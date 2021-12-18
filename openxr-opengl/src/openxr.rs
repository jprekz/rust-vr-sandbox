use openxr as xr;

const VIEW_TYPE: xr::ViewConfigurationType = xr::ViewConfigurationType::PRIMARY_STEREO;

pub struct Swapchain {
    pub handle: xr::Swapchain<xr::OpenGL>,
    pub rect: xr::Rect2Di,
}

pub struct Interaction {
    pub action_set: xr::ActionSet,
    pub right_action: xr::Action<xr::Posef>,
    pub left_action: xr::Action<xr::Posef>,
    pub right_space: xr::Space,
    pub left_space: xr::Space,
    pub stage: xr::Space,
}

pub struct OpenXR {
    system: xr::SystemId,
    instance: xr::Instance,
    session: xr::Session<xr::OpenGL>,
    session_running: bool,
    frame_wait: xr::FrameWaiter,
    frame_stream: xr::FrameStream<xr::OpenGL>,
    environment_blend_mode: xr::EnvironmentBlendMode,
    interaction: Interaction,
    event_storage: xr::EventDataBuffer,
    swapchains: Option<Vec<Swapchain>>,
}
impl OpenXR {
    pub fn new(session_create_info: xr::opengl::SessionCreateInfo) -> OpenXR {
        let entry = xr::Entry::linked();

        let instance = {
            let app_info = xr::ApplicationInfo {
                application_name: "hello openxrs",
                ..Default::default()
            };

            let extensions = entry
                .enumerate_extensions()
                .expect("Cannot enumerate extensions");
            if !extensions.khr_opengl_enable {
                panic!("XR: OpenGL extension unsupported");
            }

            let mut extension_set = xr::ExtensionSet::default();
            extension_set.khr_opengl_enable = true;

            entry
                .create_instance(&app_info, &extension_set, &[])
                .unwrap()
        };

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

        let _reqs = <xr::OpenGL as xr::Graphics>::requirements(&instance, system).unwrap();

        let (session, frame_wait, frame_stream) =
            unsafe { instance.create_session::<xr::OpenGL>(system, &session_create_info) }.unwrap();

        let action_set = instance
            .create_action_set("input", "input pose information", 0)
            .unwrap();

        let right_action = action_set
            .create_action::<xr::Posef>("right_hand", "Right Hand Controller", &[])
            .unwrap();
        let left_action = action_set
            .create_action::<xr::Posef>("left_hand", "Left Hand Controller", &[])
            .unwrap();

        instance
            .suggest_interaction_profile_bindings(
                instance
                    .string_to_path("/interaction_profiles/khr/simple_controller")
                    .unwrap(),
                &[
                    xr::Binding::new(
                        &right_action,
                        instance
                            .string_to_path("/user/hand/right/input/grip/pose")
                            .unwrap(),
                    ),
                    xr::Binding::new(
                        &left_action,
                        instance
                            .string_to_path("/user/hand/left/input/grip/pose")
                            .unwrap(),
                    ),
                ],
            )
            .unwrap();

        session.attach_action_sets(&[&action_set]).unwrap();

        let right_space = right_action
            .create_space(session.clone(), xr::Path::NULL, xr::Posef::IDENTITY)
            .unwrap();
        let left_space = left_action
            .create_space(session.clone(), xr::Path::NULL, xr::Posef::IDENTITY)
            .unwrap();

        let stage = session
            .create_reference_space(xr::ReferenceSpaceType::STAGE, xr::Posef::IDENTITY)
            .unwrap();

        let event_storage = xr::EventDataBuffer::new();

        OpenXR {
            system,
            instance,
            session,
            session_running: false,
            frame_wait,
            frame_stream,
            environment_blend_mode,
            interaction: Interaction {
                action_set,
                right_action,
                left_action,
                right_space,
                left_space,
                stage,
            },
            event_storage,
            swapchains: None,
        }
    }

    pub fn process_events(&mut self, control_flow: &mut glutin::event_loop::ControlFlow) {
        while let Some(event) = self.instance.poll_event(&mut self.event_storage).unwrap() {
            use xr::Event::*;
            match event {
                SessionStateChanged(e) => {
                    println!("entered state {:?}", e.state());
                    match e.state() {
                        xr::SessionState::READY => {
                            self.session.begin(VIEW_TYPE).unwrap();
                            self.session_running = true;
                        }
                        xr::SessionState::STOPPING => {
                            self.session.end().unwrap();
                            self.session_running = false;
                        }
                        xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => {
                            *control_flow = glutin::event_loop::ControlFlow::Exit;
                        }
                        _ => {}
                    };
                }
                InstanceLossPending(_) => {
                    *control_flow = glutin::event_loop::ControlFlow::Exit;
                }
                EventsLost(e) => {
                    println!("lost {} events", e.lost_event_count());
                }
                _ => {}
            };
        }
    }

    pub fn wait_frame(
        &mut self,
        mut frame_fn: impl FnMut(
            &xr::Session<xr::OpenGL>,
            &Vec<xr::View>,
            &Interaction,
            &xr::FrameState,
            &mut Vec<Swapchain>,
        ),
    ) {
        if !self.session_running {
            std::thread::sleep(std::time::Duration::from_millis(1000 / 60));
            return;
        }

        let xr_frame_state = self.frame_wait.wait().unwrap();

        self.frame_stream.begin().unwrap();

        if !xr_frame_state.should_render {
            self.frame_stream
                .end(
                    xr_frame_state.predicted_display_time,
                    self.environment_blend_mode,
                    &[],
                )
                .unwrap();
            return;
        }

        let swapchains = self.swapchains.get_or_insert_with(|| {
            let view_configuration_views = self
                .instance
                .enumerate_view_configuration_views(self.system, VIEW_TYPE)
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

                    let swapchain_formats = self.session.enumerate_swapchain_formats().unwrap();
                    if !swapchain_formats.contains(&glow::SRGB8_ALPHA8) {
                        panic!("XR: Cannot use OpenGL GL_SRGB8_ALPHA8 swapchain format");
                    }

                    let handle = self
                        .session
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

        self.session
            .sync_actions(&[(&self.interaction.action_set).into()])
            .unwrap();

        let (_flags, views) = self
            .session
            .locate_views(
                VIEW_TYPE,
                xr_frame_state.predicted_display_time,
                &self.interaction.stage,
            )
            .unwrap();

        frame_fn(
            &self.session,
            &views,
            &self.interaction,
            &xr_frame_state,
            swapchains,
        );

        self.frame_stream
            .end(
                xr_frame_state.predicted_display_time,
                self.environment_blend_mode,
                &[&xr::CompositionLayerProjection::new()
                    .space(&self.interaction.stage)
                    .views(&[
                        xr::CompositionLayerProjectionView::new()
                            .pose(views[0].pose)
                            .fov(views[0].fov)
                            .sub_image(
                                xr::SwapchainSubImage::new()
                                    .swapchain(&swapchains[0].handle)
                                    .image_rect(swapchains[0].rect),
                            ),
                        xr::CompositionLayerProjectionView::new()
                            .pose(views[1].pose)
                            .fov(views[1].fov)
                            .sub_image(
                                xr::SwapchainSubImage::new()
                                    .swapchain(&swapchains[1].handle)
                                    .image_rect(swapchains[1].rect),
                            ),
                    ])],
            )
            .unwrap();
    }
}

pub fn pose_transform_matrix(pose: xr::Posef) -> glam::f32::Mat4 {
    let rotation = glam::f32::Quat::from_xyzw(
        pose.orientation.x,
        pose.orientation.y,
        pose.orientation.z,
        pose.orientation.w,
    );
    let translation = glam::f32::vec3(pose.position.x, pose.position.y, pose.position.z);
    glam::f32::Mat4::from_rotation_translation(rotation, translation)
}

pub fn fov_perspective_projection_matrix(
    fov: xr::Fovf,
    z_near: f32,
    z_far: f32,
) -> glam::f32::Mat4 {
    let tan_left = fov.angle_left.tan();
    let tan_right = fov.angle_right.tan();
    let tan_down = fov.angle_down.tan();
    let tan_up = fov.angle_up.tan();

    let tan_width = tan_right - tan_left;
    let tan_height = tan_up - tan_down;

    glam::f32::Mat4::from_cols_array(&[
        2.0 / tan_width,
        0.0,
        0.0,
        0.0,
        0.0,
        2.0 / tan_height,
        0.0,
        0.0,
        (tan_right + tan_left) / tan_width,
        (tan_up + tan_down) / tan_height,
        -(z_far + z_near) / (z_far - z_near),
        -1.0,
        0.0,
        0.0,
        -(z_far * (z_near + z_near)) / (z_far - z_near),
        0.0,
    ])
}
