//! Assumes that the input and output devices can use the same stream configuration and that they
//! support the f32 sample format.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use ringbuf::RingBuffer;

const LATENCY_MS: f32 = 150.0;

use wgpu_glyph::{ab_glyph, GlyphBrushBuilder, Section, Text};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Conditionally compile with jack if the feature is specified.
    #[cfg(all(
        any(target_os = "linux", target_os = "dragonfly", target_os = "freebsd"),
        feature = "jack"
    ))]
    // Manually check for flags. Can be passed through cargo with -- e.g.
    // cargo run --release --example beep --features jack -- --jack
    let host = if std::env::args()
        .collect::<String>()
        .contains(&String::from("--jack"))
    {
        cpal::host_from_id(cpal::available_hosts()
            .into_iter()
            .find(|id| *id == cpal::HostId::Jack)
            .expect(
                "make sure --features jack is specified. only works on OSes where jack is available",
            )).expect("jack host unavailable")
    } else {
        cpal::default_host()
    };

    #[cfg(any(
        not(any(target_os = "linux", target_os = "dragonfly", target_os = "freebsd")),
        not(feature = "jack")
    ))]
    let host = cpal::default_host();

    // Default devices.
    let input_device = host
        .default_input_device()
        .expect("failed to get default input device");
    let output_device = host
        .default_output_device()
        .expect("failed to get default output device");
    println!("Using default input device: \"{}\"", input_device.name()?);
    println!("Using default output device: \"{}\"", output_device.name()?);

    // We'll try and use the same configuration between streams to keep it simple.
    let config: cpal::StreamConfig = input_device.default_input_config()?.into();

    // Create a delay in case the input and output devices aren't synced.
    let latency_frames = (LATENCY_MS / 1_000.0) * config.sample_rate.0 as f32;
    let latency_samples = latency_frames as usize * config.channels as usize;

    // The buffer to share samples
    let ring = RingBuffer::new(latency_samples * 2);
    let (mut producer, mut consumer) = ring.split();

    // Fill the samples with 0.0 equal to the length of the delay.
    for _ in 0..latency_samples {
        // The ring buffer has twice as much space as necessary to add latency here,
        // so this should never fail
        producer.push(0.0).unwrap();
    }

    let input_data_fn = move |data: &[f32], _: &cpal::InputCallbackInfo| {
        let mut output_fell_behind = false;
        for &sample in data {
            if producer.push(sample).is_err() {
                output_fell_behind = true;
            }
        }
        if output_fell_behind {
            eprintln!("output stream fell behind: try increasing latency");
        }
    };

    let output_data_fn = move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
        let mut input_fell_behind = false;
        for sample in data {
            *sample = match consumer.pop() {
                Some(s) => s,
                None => {
                    input_fell_behind = true;
                    0.0
                }
            };
        }
        if input_fell_behind {
            eprintln!("input stream fell behind: try increasing latency");
        }
    };

    // Build streams.
    println!(
        "Attempting to build both streams with f32 samples and `{:?}`.",
        config
    );
    let input_stream = input_device.build_input_stream(&config, input_data_fn, err_fn)?;
    let output_stream = output_device.build_output_stream(&config, output_data_fn, err_fn)?;
    println!("Successfully built streams.");

    // Play the streams.
    println!(
        "Starting the input and output streams with `{}` milliseconds of latency.",
        LATENCY_MS
    );
    input_stream.play()?;
    output_stream.play()?;

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

fn err_fn(err: cpal::StreamError) {
    eprintln!("an error occurred on stream: {}", err);
}
