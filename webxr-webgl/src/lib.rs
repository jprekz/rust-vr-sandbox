mod webxr;

use std::cell::RefCell;
use std::rc::Rc;

use glam::f32::{vec3, Mat4};
use glow::HasContext;
use wasm_bindgen::prelude::*;
use wasm_bindgen::JsCast;
use winit::{
    dpi::LogicalSize,
    event::{Event, VirtualKeyCode, WindowEvent},
    event_loop::{ControlFlow, EventLoop},
    window::WindowBuilder,
};

#[wasm_bindgen(start)]
pub fn main() {
    std::panic::set_hook(Box::new(console_error_panic_hook::hook));

    let event_loop = EventLoop::new();
    let window = WindowBuilder::new()
        .with_title("Hello world!")
        .with_inner_size(LogicalSize::new(800.0, 800.0))
        .build(&event_loop)
        .unwrap();

    use winit::platform::web::WindowExtWebSys;

    let canvas = window.canvas();

    web_sys::window()
        .unwrap()
        .document()
        .unwrap()
        .body()
        .unwrap()
        .append_child(&canvas)
        .unwrap();

    let mut gl_attribs = std::collections::HashMap::new();
    gl_attribs.insert(String::from("xrCompatible"), true);
    let js_gl_attribs = JsValue::from_serde(&gl_attribs).unwrap();

    let webgl2_context = canvas
        .get_context_with_context_options("webgl2", &js_gl_attribs)
        .unwrap()
        .unwrap()
        .dyn_into::<web_sys::WebGl2RenderingContext>()
        .unwrap();

    let gl = glow::Context::from_webgl2_context(webgl2_context.clone());
    let gl = Rc::new(RefCell::new(gl));
    unsafe {
        gl.borrow().enable(glow::DEPTH_TEST);
    }

    let scene = Rc::new(RefCell::new(Scene::new(&gl.borrow())));

    let mut xr = webxr::WebXR::new();

    event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Poll;
        match event {
            Event::WindowEvent { ref event, .. } => match event {
                WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                WindowEvent::KeyboardInput { input, .. }
                    if input.virtual_keycode == Some(VirtualKeyCode::Escape) =>
                {
                    xr.stop();
                }
                WindowEvent::KeyboardInput { input, .. }
                    if input.virtual_keycode == Some(VirtualKeyCode::Return) =>
                {
                    let gl = gl.clone();
                    let scene = scene.clone();
                    xr.start(
                        webgl2_context.clone(),
                        move |session, views, gl_layer, frame, ref_space| unsafe {
                            let gl = gl.borrow();

                            scene.borrow_mut().update(&session, &frame, &ref_space);

                            gl.clear_color(0.0, 0.0, 0.0, 1.0);
                            gl.clear(glow::COLOR_BUFFER_BIT);

                            for view in views {
                                let viewport = gl_layer.get_viewport(&view).unwrap();
                                gl.viewport(
                                    viewport.x(),
                                    viewport.y(),
                                    viewport.width(),
                                    viewport.height(),
                                );

                                scene.borrow_mut().v_mat = glam::Mat4::from_cols_slice(
                                    &view.transform().inverse().matrix(),
                                );
                                scene.borrow_mut().p_mat =
                                    glam::Mat4::from_cols_slice(&view.projection_matrix());

                                scene.borrow().render(&gl);
                            }
                        },
                    );
                }
                _ => (),
            },
            Event::LoopDestroyed => {
                return;
            }
            Event::MainEventsCleared => {
                xr.process_events(control_flow);
                window.request_redraw();
            }
            Event::RedrawRequested(_) => unsafe {
                let gl = gl.borrow();

                gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                gl.clear_color(0.0, 0.0, 0.0, 1.0);
                gl.clear(glow::COLOR_BUFFER_BIT);
                let size = window.inner_size();
                gl.viewport(0, 0, size.width as _, size.height as _);

                scene.borrow().render(&gl);
            },
            _ => (),
        }
    });
}

struct Scene {
    program: glow::Program,
    vbo: glow::Buffer,
    p_mat: Mat4,
    v_mat: Mat4,
    mid_m_mat: Mat4,
    left_m_mat: Option<Mat4>,
    right_m_mat: Option<Mat4>,
}
impl Scene {
    fn new(gl: &glow::Context) -> Scene {
        let program = unsafe {
            let program = gl.create_program().expect("Cannot create program");

            let shader_sources = [
                (
                    glow::VERTEX_SHADER,
                    r#"#version 300 es
                    precision highp float;
                    uniform mat4 mvp;
                    in vec3 Position;
                    void main() {
                        gl_Position = mvp * vec4(Position, 1);
                    }"#,
                ),
                (
                    glow::FRAGMENT_SHADER,
                    r#"#version 300 es
                    precision mediump float;
                    uniform vec3 color;
                    out vec4 FragColor;
                    void main() {
                        FragColor = vec4(color, 1);
                    } "#,
                ),
            ];

            let mut shaders = Vec::with_capacity(shader_sources.len());

            for (shader_type, shader_source) in shader_sources.iter() {
                let shader = gl
                    .create_shader(*shader_type)
                    .expect("Cannot create shader");
                gl.shader_source(shader, shader_source);
                gl.compile_shader(shader);
                if !gl.get_shader_compile_status(shader) {
                    panic!("{}", gl.get_shader_info_log(shader));
                }
                gl.attach_shader(program, shader);
                shaders.push(shader);
            }

            gl.link_program(program);
            if !gl.get_program_link_status(program) {
                panic!("{}", gl.get_program_info_log(program));
            }

            for shader in shaders {
                gl.detach_shader(program, shader);
                gl.delete_shader(shader);
            }

            program
        };

        let vbo = unsafe {
            let vertices = [0.0f32, 1.0, 0.0, 1.0, 0.0, 0.0, -1.0, 0.0, 0.0];
            let vertices_u8: &[u8] = core::slice::from_raw_parts(
                vertices.as_ptr() as *const u8,
                vertices.len() * core::mem::size_of::<f32>(),
            );

            let vbo = gl.create_buffer().unwrap();
            gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, vertices_u8, glow::STATIC_DRAW);

            let vao = gl.create_vertex_array().unwrap();
            gl.bind_vertex_array(Some(vao));
            gl.enable_vertex_attrib_array(0);
            gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, 12, 0);

            vbo
        };

        Scene {
            program,
            vbo,
            p_mat: Mat4::perspective_rh_gl(std::f32::consts::PI / 2.0, 1.0, 0.1, 100.0),
            v_mat: Mat4::look_at_rh(
                vec3(0.0, 1.0, 3.0),
                vec3(0.0, 0.0, 0.0),
                vec3(0.0, 1.0, 0.0),
            ),
            mid_m_mat: Mat4::from_translation(vec3(0.0, 0.0, -3.0)),
            left_m_mat: None,
            right_m_mat: None,
        }
    }

    fn update(
        &mut self,
        session: &web_sys::XrSession,
        frame: &web_sys::XrFrame,
        ref_pose: &web_sys::XrReferenceSpace,
    ) {
        let sources = session.input_sources();
        for i in 0..sources.length() {
            let source = sources.get(i).unwrap();
            match (source.handedness(), source.grip_space()) {
                (web_sys::XrHandedness::Left, Some(space)) => {
                    if let Some(pose) = frame.get_pose(&space, ref_pose) {
                        self.left_m_mat = Some(
                            Mat4::from_cols_slice(&pose.transform().matrix())
                                * Mat4::from_scale(vec3(0.1, 0.1, 0.1))
                                * Mat4::from_rotation_x(-std::f32::consts::PI / 2.0),
                        );
                    }
                }
                (web_sys::XrHandedness::Right, Some(space)) => {
                    if let Some(pose) = frame.get_pose(&space, ref_pose) {
                        self.right_m_mat = Some(
                            Mat4::from_cols_slice(&pose.transform().matrix())
                                * Mat4::from_scale(vec3(0.1, 0.1, 0.1))
                                * Mat4::from_rotation_x(-std::f32::consts::PI / 2.0),
                        );
                    }
                }
                _ => {}
            }
        }
    }

    unsafe fn render(&self, gl: &glow::Context) {
        gl.use_program(Some(self.program));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));

        // mid triangle
        let mvp_mat = self.p_mat * self.v_mat * self.mid_m_mat;
        let uniform_location = gl.get_uniform_location(self.program, "mvp");
        gl.uniform_matrix_4_f32_slice(uniform_location.as_ref(), false, &mvp_mat.to_cols_array());
        let uniform_location = gl.get_uniform_location(self.program, "color");
        gl.uniform_3_f32(uniform_location.as_ref(), 1.0, 1.0, 1.0);
        gl.draw_arrays(glow::TRIANGLES, 0, 3);

        // left triangle
        if let Some(left_m_mat) = self.left_m_mat {
            let mvp_mat: Mat4 = self.p_mat * self.v_mat * left_m_mat;
            let uniform_location = gl.get_uniform_location(self.program, "mvp");
            gl.uniform_matrix_4_f32_slice(
                uniform_location.as_ref(),
                false,
                &mvp_mat.to_cols_array(),
            );
            let uniform_location = gl.get_uniform_location(self.program, "color");
            gl.uniform_3_f32(uniform_location.as_ref(), 1.0, 0.0, 0.0);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }

        // right triangle
        if let Some(right_m_mat) = self.right_m_mat {
            let mvp_mat: Mat4 = self.p_mat * self.v_mat * right_m_mat;
            let uniform_location = gl.get_uniform_location(self.program, "mvp");
            gl.uniform_matrix_4_f32_slice(
                uniform_location.as_ref(),
                false,
                &mvp_mat.to_cols_array(),
            );
            let uniform_location = gl.get_uniform_location(self.program, "color");
            gl.uniform_3_f32(uniform_location.as_ref(), 0.0, 1.0, 0.0);
            gl.draw_arrays(glow::TRIANGLES, 0, 3);
        }
    }
}
