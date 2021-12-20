use std::cell::RefCell;
use std::rc::Rc;

use futures_executor::LocalPool;
use futures_util::task::LocalSpawnExt;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use wasm_bindgen_futures::JsFuture;
use web_sys::*;

pub struct WebXR {
    pool: LocalPool,
    running: bool,
    session: Rc<RefCell<Option<XrSession>>>,
    ref_space: Rc<RefCell<Option<XrReferenceSpace>>>,
}
impl WebXR {
    pub fn new() -> WebXR {
        WebXR {
            pool: LocalPool::new(),
            running: false,
            session: Rc::new(RefCell::new(None)),
            ref_space: Rc::new(RefCell::new(None)),
        }
    }

    pub fn start(
        &mut self,
        webgl2_context: WebGl2RenderingContext,
        mut frame_fn: impl FnMut(XrSession, Vec<XrView>, XrWebGlLayer, XrFrame, &XrReferenceSpace)
            + 'static,
    ) {
        if self.running {
            return;
        }
        self.running = true;

        let session = self.session.clone();
        let ref_space = self.ref_space.clone();

        self.pool
            .spawner()
            .spawn_local(async move {
                let navigator = window().unwrap().navigator();
                let xr = navigator.xr();

                let session_mode = XrSessionMode::ImmersiveVr;
                let supports_session = JsFuture::from(xr.is_session_supported(session_mode))
                    .await
                    .expect("neeee");
                if supports_session == false {
                    panic!();
                }

                let mut session_init = XrSessionInit::new();
                session_init.optional_features(&JsValue::from_serde(&["bounded-floor"]).unwrap());
                session.borrow_mut().replace(
                    JsFuture::from(xr.request_session_with_options(session_mode, &session_init))
                        .await
                        .unwrap()
                        .into(),
                );

                let borrowed_session = session.borrow();
                let session = borrowed_session.as_ref().unwrap();

                let gl_layer =
                    XrWebGlLayer::new_with_web_gl2_rendering_context(session, &webgl2_context)
                        .unwrap();
                let mut render_state_init = XrRenderStateInit::new();
                render_state_init.base_layer(Some(&gl_layer));
                session.update_render_state_with_state(&render_state_init);

                let space_type = XrReferenceSpaceType::BoundedFloor;
                ref_space.borrow_mut().replace(
                    JsFuture::from(session.request_reference_space(space_type))
                        .await
                        .unwrap()
                        .into(),
                );

                let f: Rc<RefCell<Option<Closure<dyn FnMut(f64, XrFrame)>>>> =
                    Rc::new(RefCell::new(None));
                let g = f.clone();
                let callback = Closure::wrap(Box::new(move |_time: f64, frame: XrFrame| {
                    let session = frame.session();
                    let gl = webgl2_context.clone();

                    let gl_layer = session.render_state().base_layer().unwrap();
                    let swapchain_framebuffer = &gl_layer.framebuffer();
                    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(swapchain_framebuffer));

                    let ref_space = ref_space.borrow();
                    let ref_space = ref_space.as_ref().unwrap();
                    let pose = frame.get_viewer_pose(ref_space).unwrap();
                    let views = pose.views().iter().map(|v| v.into()).collect();

                    frame_fn(session.clone(), views, gl_layer, frame, ref_space);

                    session.request_animation_frame(
                        f.borrow().as_ref().unwrap().as_ref().unchecked_ref(),
                    );
                }) as Box<dyn FnMut(f64, XrFrame)>);

                *g.borrow_mut() = Some(callback);
                session
                    .request_animation_frame(g.borrow().as_ref().unwrap().as_ref().unchecked_ref());
            })
            .expect("Failed to start application");
    }

    pub fn stop(&mut self) {
        if !self.running {
            return;
        }
        if let Some(session) = self.session.borrow().as_ref() {
            let _p = session.end();
            self.running = false;
        }
    }

    pub fn process_events(&mut self, _control_flow: &mut winit::event_loop::ControlFlow) {
        self.pool.try_run_one();
    }
}
