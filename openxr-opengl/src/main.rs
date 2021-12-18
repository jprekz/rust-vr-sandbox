mod openxr;

use ::openxr as xr;
use glam::f32::{vec3, Mat4};
use glow::HasContext;
use glutin::event::{Event, VirtualKeyCode, WindowEvent};
use glutin::event_loop::ControlFlow;
use glutin::platform::{
    windows::{RawHandle, WindowExtWindows},
    ContextTraitExt,
};
use winapi::{shared::windef::HWND, um::winuser::GetDC};

use crate::openxr::OpenXR;

struct Backend {
    event_loop: glutin::event_loop::EventLoop<()>,
    windowed_context: glutin::ContextWrapper<glutin::PossiblyCurrent, glutin::window::Window>,
}
impl Backend {
    fn new() -> Backend {
        let el = glutin::event_loop::EventLoop::new();
        let wb = glutin::window::WindowBuilder::new()
            .with_title("Hello world!")
            .with_inner_size(glutin::dpi::LogicalSize::new(800.0, 800.0));
        let windowed_context = glutin::ContextBuilder::new()
            .build_windowed(wb, &el)
            .unwrap();
        let windowed_context = unsafe { windowed_context.make_current().unwrap() };
        Backend {
            event_loop: el,
            windowed_context,
        }
    }

    fn get_gl_context(&self) -> glow::Context {
        unsafe {
            glow::Context::from_loader_function(|s| {
                self.windowed_context.get_proc_address(s) as *const _
            })
        }
    }

    fn get_xr_session_create_info(&self) -> xr::opengl::SessionCreateInfo {
        let hwnd = self.windowed_context.window().hwnd();
        let h_dc = unsafe { GetDC(hwnd as HWND) };
        let handle = unsafe { self.windowed_context.raw_handle() };
        let h_glrc = match handle {
            RawHandle::Egl(_) => panic!(),
            RawHandle::Wgl(h_glrc) => h_glrc,
        };
        xr::opengl::SessionCreateInfo::Windows { h_dc, h_glrc }
    }
}

fn main() {
    let backend = Backend::new();
    let gl = backend.get_gl_context();

    let session_create_info = backend.get_xr_session_create_info();
    let mut xr = OpenXR::new(session_create_info);

    let mut scene = Scene::new(&gl);

    let swapchain_framebuffer = unsafe { gl.create_framebuffer() }.unwrap();

    backend.event_loop.run(move |event, _, control_flow| {
        *control_flow = ControlFlow::Poll;
        match event {
            Event::WindowEvent { ref event, .. } => match event {
                WindowEvent::Resized(physical_size) => {
                    backend.windowed_context.resize(*physical_size);
                }
                WindowEvent::CloseRequested => *control_flow = ControlFlow::Exit,
                WindowEvent::KeyboardInput { input, .. }
                    if input.virtual_keycode == Some(VirtualKeyCode::Escape) =>
                {
                    *control_flow = ControlFlow::Exit
                }
                _ => (),
            },
            Event::LoopDestroyed => {
                return;
            }
            Event::MainEventsCleared => {
                xr.process_events(control_flow);
                backend.windowed_context.window().request_redraw();
            }
            Event::RedrawRequested(_) => {
                xr.wait_frame(|session, views, interaction, xr_frame_state, swapchains| {
                    scene.update(session, interaction, xr_frame_state);

                    for (i, swapchain) in swapchains.iter_mut().enumerate() {
                        let view = views[i];
                        unsafe {
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

                            scene.v_mat = openxr::pose_transform_matrix(view.pose).inverse();
                            scene.p_mat =
                                openxr::fov_perspective_projection_matrix(view.fov, 0.1, 100.0);

                            scene.render(&gl);

                            swapchain.handle.release_image().unwrap();
                        }
                    }
                });

                unsafe {
                    gl.bind_framebuffer(glow::FRAMEBUFFER, None);
                    let size = backend.windowed_context.window().inner_size();
                    gl.viewport(0, 0, size.width as _, size.height as _);

                    scene.render(&gl);
                }

                backend.windowed_context.swap_buffers().unwrap();
            }
            _ => (),
        }
    });
}

struct Scene {
    program: glow::NativeProgram,
    vbo: glow::NativeBuffer,
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
                    r#"#version 410
                    uniform mat4 mvp;
                    in vec3 Position;
                    void main() {
                        gl_Position = mvp * vec4(Position, 1);
                    }"#,
                ),
                (
                    glow::FRAGMENT_SHADER,
                    r#"#version 410
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
        session: &xr::Session<xr::OpenGL>,
        interaction: &openxr::Interaction,
        xr_frame_state: &xr::FrameState,
    ) {
        let left_location = interaction
            .left_space
            .locate(&interaction.stage, xr_frame_state.predicted_display_time)
            .unwrap();
        if interaction
            .left_action
            .is_active(session, xr::Path::NULL)
            .unwrap()
        {
            self.left_m_mat = Some(
                openxr::pose_transform_matrix(left_location.pose)
                    * Mat4::from_scale(vec3(0.1, 0.1, 0.1))
                    * Mat4::from_rotation_x(-std::f32::consts::PI / 2.0),
            );
        }

        let right_location = interaction
            .right_space
            .locate(&interaction.stage, xr_frame_state.predicted_display_time)
            .unwrap();
        if interaction
            .right_action
            .is_active(session, xr::Path::NULL)
            .unwrap()
        {
            self.right_m_mat = Some(
                openxr::pose_transform_matrix(right_location.pose)
                    * Mat4::from_scale(vec3(0.1, 0.1, 0.1))
                    * Mat4::from_rotation_x(-std::f32::consts::PI / 2.0),
            );
        }
    }

    unsafe fn render(&self, gl: &glow::Context) {
        gl.use_program(Some(self.program));
        gl.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));

        gl.clear_color(0.0, 0.0, 0.0, 1.0);
        gl.clear(glow::COLOR_BUFFER_BIT);

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
