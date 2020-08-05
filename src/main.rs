use std::error::Error;
use wgpu_glyph::{ab_glyph, GlyphBrushBuilder, Section, Text};

fn main() -> Result<(), Box<dyn Error>> {
    use cpal::traits::{DeviceTrait, HostTrait};
    // use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

    let host = cpal::default_host();
    let input_device = host
        .default_input_device()
        .expect("No input device available.");
    let output_device = host
        .default_output_device()
        .expect("No output device available.");

    let mut supported_input_configs_range = input_device
        .supported_input_configs()
        .expect("error while querying input configs");
    let supported_input_config = supported_input_configs_range
        .next()
        .expect("no supported input config?!")
        .with_max_sample_rate();

    let mut extra_config_opt = supported_input_configs_range.next();
    while extra_config_opt != None {
        let extra_config = extra_config_opt
            .expect("error while iterating over supported configs")
            .with_max_sample_rate();
        println!("{:?}", extra_config);
        extra_config_opt = supported_input_configs_range.next();
    }
    println!();

    let mut supported_configs_range = output_device
        .supported_output_configs()
        .expect("error while querying configs");
    let supported_config = supported_configs_range
        .next()
        .expect("no supported config?!")
        .with_max_sample_rate();

    let mut extra_config_opt = supported_configs_range.next();
    while extra_config_opt != None {
        let extra_config = extra_config_opt
            .expect("error while iterating over supported configs")
            .with_max_sample_rate();
        println!("{:?}", extra_config);
        extra_config_opt = supported_configs_range.next();
    }
    println!();

    println!("supported_input_config was: {:?}", supported_input_config);
    println!("supported_config was: {:?}", supported_config);

    // use cpal::{Data, Sample, SampleFormat};
    use cpal::{Sample, SampleFormat};
    let err_fn = |err| eprintln!("an error occurred on the output audio stream: {}", err);
    let sample_format = supported_config.sample_format();
    // TODO: check against supported_input_config.sample_format();

    let mut input_config: cpal::StreamConfig = supported_input_config.into();
    // Overriding values anyway:
    input_config.channels = 1;
    input_config.sample_rate = cpal::SampleRate(44100);
    println!("using input_config: {:?}", input_config);

    let mut config: cpal::StreamConfig = supported_config.into();
    // Overriding values anyway:
    config.channels = 1;
    config.sample_rate = cpal::SampleRate(44100);
    println!("using config: {:?}", config);

    let _stream = match sample_format {
        SampleFormat::F32 => {
            // input_device.build_input_stream(&config, read_samples::<f32>, err_fn)
            output_device.build_output_stream(&config, write_silence::<f32>, err_fn)
        }
        SampleFormat::I16 => {
            // input_device.build_input_stream(&config, read_samples::<i16>, err_fn)
            output_device.build_output_stream(&config, write_silence::<i16>, err_fn)
        }
        SampleFormat::U16 => {
            // input_device.build_input_stream(&config, read_samples::<u16>, err_fn)
            output_device.build_output_stream(&config, write_silence::<u16>, err_fn)
        }
    };
    if let Err(e) = _stream {
        println!("Error from build_output_stream: {}", e);
        // TODO: find a good way to terminate.
        // return ();
        panic!();
    }
    let _stream = _stream.unwrap();

    fn write_silence<T: Sample>(data: &mut [T], _: &cpal::OutputCallbackInfo) {
        for sample in data.iter_mut() {
            *sample = Sample::from(&0.0);
        }
    }

    env_logger::init();

    // Open window and create a surface
    let event_loop = winit::event_loop::EventLoop::new();

    let window = winit::window::WindowBuilder::new()
        .with_resizable(false)
        .build(&event_loop)
        .unwrap();

    let surface = wgpu::Surface::create(&window);

    // Initialize GPU
    let (device, queue) = futures::executor::block_on(async {
        let adapter = wgpu::Adapter::request(
            &wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
            },
            wgpu::BackendBit::all(),
        )
        .await
        .expect("Request adapter");

        adapter
            .request_device(&wgpu::DeviceDescriptor {
                extensions: wgpu::Extensions {
                    anisotropic_filtering: false,
                },
                limits: wgpu::Limits { max_bind_groups: 1 },
            })
            .await
    });

    // Prepare swap chain
    let render_format = wgpu::TextureFormat::Bgra8UnormSrgb;
    let mut size = window.inner_size();

    let mut swap_chain = device.create_swap_chain(
        &surface,
        &wgpu::SwapChainDescriptor {
            usage: wgpu::TextureUsage::OUTPUT_ATTACHMENT,
            format: render_format,
            width: size.width,
            height: size.height,
            present_mode: wgpu::PresentMode::Mailbox,
        },
    );

    // Prepare glyph_brush
    let inconsolata = ab_glyph::FontArc::try_from_slice(include_bytes!(
        "Inconsolata-Regular.ttf"
    ))?;

    let mut glyph_brush = GlyphBrushBuilder::using_font(inconsolata)
        .build(&device, render_format);

    // Render loop
    window.request_redraw();

    event_loop.run(move |event, _, control_flow| {
        match event {
            winit::event::Event::WindowEvent {
                event: winit::event::WindowEvent::CloseRequested,
                ..
            } => *control_flow = winit::event_loop::ControlFlow::Exit,
            winit::event::Event::WindowEvent {
                event: winit::event::WindowEvent::Resized(new_size),
                ..
            } => {
                size = new_size;

                swap_chain = device.create_swap_chain(
                    &surface,
                    &wgpu::SwapChainDescriptor {
                        usage: wgpu::TextureUsage::OUTPUT_ATTACHMENT,
                        format: render_format,
                        width: size.width,
                        height: size.height,
                        present_mode: wgpu::PresentMode::Mailbox,
                    },
                );
            }
            winit::event::Event::RedrawRequested { .. } => {
                // Get a command encoder for the current frame
                let mut encoder = device.create_command_encoder(
                    &wgpu::CommandEncoderDescriptor {
                        label: Some("Redraw"),
                    },
                );

                // Get the next frame
                let frame =
                    swap_chain.get_next_texture().expect("Get next frame");

                // Clear frame
                {
                    let _ = encoder.begin_render_pass(
                        &wgpu::RenderPassDescriptor {
                            color_attachments: &[
                                wgpu::RenderPassColorAttachmentDescriptor {
                                    attachment: &frame.view,
                                    resolve_target: None,
                                    load_op: wgpu::LoadOp::Clear,
                                    store_op: wgpu::StoreOp::Store,
                                    clear_color: wgpu::Color {
                                        r: 0.4,
                                        g: 0.4,
                                        b: 0.4,
                                        a: 1.0,
                                    },
                                },
                            ],
                            depth_stencil_attachment: None,
                        },
                    );
                }

                glyph_brush.queue(Section {
                    screen_position: (30.0, 30.0),
                    bounds: (size.width as f32, size.height as f32),
                    text: vec![Text::new("Hello wgpu_glyph!")
                        .with_color([0.0, 0.0, 0.0, 1.0])
                        .with_scale(40.0)],
                    ..Section::default()
                });

                glyph_brush.queue(Section {
                    screen_position: (30.0, 90.0),
                    bounds: (size.width as f32, size.height as f32),
                    text: vec![Text::new("Hello wgpu_glyph!")
                        .with_color([1.0, 1.0, 1.0, 1.0])
                        .with_scale(40.0)],
                    ..Section::default()
                });

                // Draw the text!
                glyph_brush
                    .draw_queued(
                        &device,
                        &mut encoder,
                        &frame.view,
                        size.width,
                        size.height,
                    )
                    .expect("Draw queued");

                queue.submit(&[encoder.finish()]);
            }
            _ => {
                *control_flow = winit::event_loop::ControlFlow::Wait;
            }
        }
    });
}
