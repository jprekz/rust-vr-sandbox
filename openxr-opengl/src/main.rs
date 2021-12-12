use glow::HasContext;
use glutin::event::{Event, WindowEvent};
use glutin::event_loop::ControlFlow;
use glutin::platform::{
    windows::{RawHandle, WindowExtWindows},
    ContextTraitExt,
};
use openxr as xr;
use winapi::{shared::windef::HWND, um::winuser::GetDC};

fn main() {
    let el = glutin::event_loop::EventLoop::new();
    let wb = glutin::window::WindowBuilder::new()
        .with_title("Hello world!")
        .with_inner_size(glutin::dpi::LogicalSize::new(800.0, 800.0));
    let windowed_context = glutin::ContextBuilder::new()
        .build_windowed(wb, &el)
        .unwrap();
    let windowed_context = unsafe { windowed_context.make_current().unwrap() };
    let gl = unsafe {
        glow::Context::from_loader_function(|s| windowed_context.get_proc_address(s) as *const _)
    };

    // XR
    let entry = xr::Entry::linked();
    let extensions = entry
        .enumerate_extensions()
        .expect("Cannot enumerate extensions");
    if !extensions.khr_opengl_enable {
        panic!("XR: OpenGL extension unsupported");
    }
    let app_info = xr::ApplicationInfo {
        application_name: "hello openxrs",
        ..Default::default()
    };
    let mut extension_set = xr::ExtensionSet::default();
    // IMPORTANT?
    extension_set.khr_opengl_enable = true;
    let instance = entry
        .create_instance(&app_info, &extension_set, &[])
        .unwrap();

    let instance_props = instance.properties().unwrap();
    println!(
        "loaded instance: {} v{}",
        instance_props.runtime_name, instance_props.runtime_version
    );

    let system = instance
        .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
        .unwrap();
    let environment_blend_mode = instance
        .enumerate_environment_blend_modes(system, VIEW_TYPE)
        .unwrap()[0];
    let system_props = instance.system_properties(system).unwrap();
    println!(
        "selected system {}: {}",
        system_props.system_id.into_raw(),
        if system_props.system_name.is_empty() {
            "<unnamed>"
        } else {
            &system_props.system_name
        }
    );

    // IMPORTANT?
    let _reqs = <xr::OpenGL as xr::Graphics>::requirements(&instance, system).unwrap();

    let session_create_info = unsafe {
        let hwnd = windowed_context.window().hwnd();
        let h_dc = GetDC(hwnd as HWND);
        let handle = windowed_context.raw_handle();
        let h_glrc = match handle {
            RawHandle::Egl(_) => panic!(),
            RawHandle::Wgl(h_glrc) => h_glrc,
        };
        xr::opengl::SessionCreateInfo::Windows { h_dc, h_glrc }
    };
    let (session, mut frame_wait, mut frame_stream) =
        unsafe { instance.create_session::<xr::OpenGL>(system, &session_create_info) }.unwrap();

    let stage = session
        .create_reference_space(xr::ReferenceSpaceType::STAGE, xr::Posef::IDENTITY)
        .unwrap();

    // graphic initialize and event loop
    let mut swapchains = None;
    let swapchain_framebuffer = unsafe { gl.create_framebuffer() }.unwrap();

    let mut event_storage = xr::EventDataBuffer::new();
    let mut session_running = false;
    let mut frame_count = 0;
    el.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Poll;
        match event {
            Event::LoopDestroyed => {
                return;
            }
            Event::MainEventsCleared => {
                windowed_context.window().request_redraw();
            }
            Event::RedrawRequested(_) => {
                while let Some(event) = instance.poll_event(&mut event_storage).unwrap() {
                    use xr::Event::*;
                    match event {
                        SessionStateChanged(e) => {
                            println!("entered state {:?}", e.state());
                            match e.state() {
                                xr::SessionState::READY => {
                                    session.begin(VIEW_TYPE).unwrap();
                                    session_running = true;
                                }
                                xr::SessionState::STOPPING => {
                                    session.end().unwrap();
                                    session_running = false;
                                }
                                xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => {
                                    *control_flow = ControlFlow::Exit;
                                }
                                _ => {}
                            };
                        }
                        InstanceLossPending(_) => {
                            *control_flow = ControlFlow::Exit;
                        }
                        EventsLost(e) => {
                            println!("lost {} events", e.lost_event_count());
                        }
                        _ => {}
                    };
                }

                if session_running {
                    let xr_frame_state = frame_wait.wait().unwrap();
                    frame_stream.begin().unwrap();

                    if !xr_frame_state.should_render {
                        frame_stream
                            .end(
                                xr_frame_state.predicted_display_time,
                                environment_blend_mode,
                                &[],
                            )
                            .unwrap();
                        return;
                    }

                    let swapchains = swapchains.get_or_insert_with(|| {
                        let view_configuration_views = instance
                            .enumerate_view_configuration_views(system, VIEW_TYPE)
                            .unwrap();
                        view_configuration_views
                            .into_iter()
                            .map(|vp| {
                                let width = vp.recommended_image_rect_width;
                                let height = vp.recommended_image_rect_height;
                                let rect = xr::Rect2Di {
                                    offset: xr::Offset2Di { x: 0, y: 0 },
                                    extent: xr::Extent2Di {
                                        width: width as _,
                                        height: height as _,
                                    },
                                };

                                let sample_count = vp.recommended_swapchain_sample_count;

                                let swapchain_formats =
                                    session.enumerate_swapchain_formats().unwrap();
                                if !swapchain_formats.contains(&glow::SRGB8_ALPHA8) {
                                    panic!(
                                        "XR: Cannot use OpenGL GL_SRGB8_ALPHA8 swapchain format"
                                    );
                                }

                                let handle = session
                                    .create_swapchain(&xr::SwapchainCreateInfo {
                                        create_flags: xr::SwapchainCreateFlags::EMPTY,
                                        usage_flags: xr::SwapchainUsageFlags::COLOR_ATTACHMENT
                                            | xr::SwapchainUsageFlags::SAMPLED,
                                        format: glow::SRGB8_ALPHA8,
                                        sample_count,
                                        width,
                                        height,
                                        face_count: 1,
                                        array_size: 1,
                                        mip_count: 1,
                                    })
                                    .unwrap();

                                Swapchain { handle, rect }
                            })
                            .collect::<Vec<_>>()
                    });

                    let (_, views) = session
                        .locate_views(VIEW_TYPE, xr_frame_state.predicted_display_time, &stage)
                        .unwrap();

                    // draw
                    for (i, swapchain) in swapchains.iter_mut().enumerate() {
                        unsafe { draw_view(&gl, swapchain, swapchain_framebuffer, i, frame_count) };
                    }

                    let layerviews = views
                        .iter()
                        .zip(swapchains)
                        .map(|(view, swapchain)| {
                            xr::CompositionLayerProjectionView::new()
                                .pose(view.pose)
                                .fov(view.fov)
                                .sub_image(
                                    xr::SwapchainSubImage::new()
                                        .swapchain(&swapchain.handle)
                                        .image_rect(swapchain.rect),
                                )
                        })
                        .collect::<Vec<_>>();
                    frame_stream
                        .end(
                            xr_frame_state.predicted_display_time,
                            environment_blend_mode,
                            &[&xr::CompositionLayerProjection::new()
                                .space(&stage)
                                .views(&layerviews)],
                        )
                        .unwrap();
                } else {
                    std::thread::sleep(std::time::Duration::from_millis(1000 / 60));
                }

                unsafe {
                    draw_window(&gl, frame_count);
                }
                windowed_context.swap_buffers().unwrap();
                frame_count += 1;
            }
            Event::WindowEvent { ref event, .. } => match event {
                WindowEvent::Resized(physical_size) => {
                    windowed_context.resize(*physical_size);
                }
                WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                _ => (),
            },
            _ => (),
        }
    });
}

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

struct Swapchain {
    handle: xr::Swapchain<xr::OpenGL>,
    rect: xr::Rect2Di,
}

unsafe fn draw_window(gl: &glow::Context, frame_count: u32) {
    gl.bind_framebuffer(glow::FRAMEBUFFER, None);

    gl.clear_color(0.01 * (frame_count % 100) as f32, 0.2, 0.3, 1.0);
    gl.clear(glow::COLOR_BUFFER_BIT);
}

unsafe fn draw_view(
    gl: &glow::Context,
    swapchain: &mut Swapchain,
    swapchain_framebuffer: glow::Framebuffer,
    i: usize,
    frame_count: u32,
) {
    gl.bind_framebuffer(glow::FRAMEBUFFER, Some(swapchain_framebuffer));

    let images = swapchain.handle.enumerate_images().unwrap();
    let image_id = swapchain.handle.acquire_image().unwrap();
    swapchain.handle.wait_image(xr::Duration::INFINITE).unwrap();
    let image = images[image_id as usize];
    let color_texture: glow::Texture = std::mem::transmute(image);
    let rect = swapchain.rect;
    gl.viewport(
        rect.offset.x,
        rect.offset.y,
        rect.extent.width,
        rect.extent.height,
    );
    gl.framebuffer_texture_2d(
        glow::FRAMEBUFFER,
        glow::COLOR_ATTACHMENT0,
        glow::TEXTURE_2D,
        Some(color_texture),
        0,
    );
    if i == 0 {
        gl.clear_color(0.2, 0.01 * (frame_count % 100) as f32, 0.3, 1.0);
    } else {
        gl.clear_color(0.2, 0.3, 0.01 * (frame_count % 100) as f32, 1.0);
    }
    gl.clear(glow::COLOR_BUFFER_BIT);

    swapchain.handle.release_image().unwrap();
}
