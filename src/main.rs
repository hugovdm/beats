//! Assumes that the input and output devices can use the same stream configuration and that they
//! support the f32 sample format.

#![feature(proc_macro_hygiene, decl_macro)]
#[macro_use]
extern crate rocket;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::thread;

mod audioplumbing;
mod webui;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let mut latency_ms: f32 = 1000.0;
    {
        let mut next_is_delay = false;
        for arg in &args {
            if next_is_delay {
                latency_ms = arg.parse().unwrap();
                next_is_delay = false;
            } else if arg == "--delay" {
                next_is_delay = true;
            }
        }
    }
    let latency_ms = latency_ms;

    // Conditionally compile with jack if the feature is specified.
    #[cfg(all(
        any(target_os = "linux", target_os = "dragonfly", target_os = "freebsd"),
        feature = "jack"
    ))]
    // Manually check for flags. Can be passed through cargo with -- e.g.
    // cargo run --release --example beep --features jack -- --jack
    let host = if args.contains(&String::from("--jack")) {
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
    println!(
        "Using default input device: \"{}\", host: \"{:?}\"",
        input_device.name()?,
        host.id()
    );
    println!(
        "Using default output device: \"{}\", host: \"{:?}\"",
        output_device.name()?,
        host.id()
    );

    // We'll try and use the same configuration between streams to keep it simple.
    let config: cpal::StreamConfig = input_device.default_input_config()?.into();

    // Create a delay in case the input and output devices aren't synced.
    let latency_frames = (latency_ms / 1_000.0) * config.sample_rate.0 as f32;
    let latency_samples = latency_frames as usize * config.channels as usize;

    // The buffer to share samples
    use ringbuf::RingBuffer;
    let ring = RingBuffer::new(latency_samples * 2);
    let (mut producer, mut consumer) = ring.split();

    // Fill the samples with 0.0 equal to the length of the delay.
    for _ in 0..latency_samples {
        // The ring buffer has twice as much space as necessary to add latency here,
        // so this should never fail
        producer.push(0.0).unwrap();
    }

    use std::sync::mpsc;
    let (chunk_sender, chunk_receiver) = mpsc::channel();
    let (sender, receiver) = mpsc::channel();
    // Review: why can we pass a SyncSender to Rocket's .manage() but not a Sender?
    // let (req_sender, req_receiver) = mpsc::channel();
    let (req_sender, req_receiver) = mpsc::sync_channel(100);
    let mut coordinator = audioplumbing::Coordinator::new(chunk_receiver, req_receiver, receiver);
    thread::spawn(move || coordinator.run());

    let channels = config.channels;

    let mut previnp_callback: Option<cpal::StreamInstant> = None;
    let mut previnp_capture: Option<cpal::StreamInstant> = None;
    // TODO: use new_uninit() for efficiency, bundling count and chunk into a
    // struct.
    let mut chunk = audioplumbing::new_chunk();
    let mut sample_count: usize = 0;
    let input_data_fn = move |data: &[f32], inpinf: &cpal::InputCallbackInfo| {
        // TODO: move to stats that are available on-demand:
        // println!("input count {}    info: {:?}", data.len(), inpinf);
        if previnp_callback != None && previnp_capture != None {
            // println!(
            //     "input timestamp diffs callback: {:?}, capture: {:?}",
            //     inpinf
            //         .timestamp()
            //         .callback
            //         .duration_since(&previnp_callback.unwrap())
            //         .unwrap(),
            //     inpinf
            //         .timestamp()
            //         .capture
            //         .duration_since(&previnp_capture.unwrap())
            //         .unwrap(),
            // );
        }
        previnp_callback = Some(inpinf.timestamp().callback);
        previnp_capture = Some(inpinf.timestamp().capture);
        let mut fs = audioplumbing::FrameStats::new(channels);
        let mut iter = data.iter();
        while fs.consume_frame(&mut iter) {}
        sender.send(fs).unwrap();

        let mut output_fell_behind = false;
        for &sample in data {
            if producer.push(sample).is_err() {
                output_fell_behind = true;
            }
            if sample_count >= audioplumbing::CHUNK_SIZE {
                chunk_sender.send(chunk.clone()).unwrap();
                chunk = audioplumbing::new_chunk();
                sample_count = 0;
            }
            // Review: is get_mut(...).unwrap() cheap? Can we somehow move
            // such binding to outside the for-loop, then somehow unbind
            // when we want to send(chunk.clone())?
            use std::sync::Arc;
            let c = Arc::get_mut(&mut chunk).unwrap();
            c[sample_count] = sample;
            sample_count = sample_count + 1;
        }
        if output_fell_behind {
            eprintln!("output stream fell behind: try increasing latency");
        }
    };

    let output_data_fn = move |data: &mut [f32], outinf: &cpal::OutputCallbackInfo| {
        // println!("output count {} info {:?}", data.len(), outinf);
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
    let input_stream =
        input_device.build_input_stream(&config, input_data_fn, audioplumbing::err_fn)?;
    let output_stream =
        output_device.build_output_stream(&config, output_data_fn, audioplumbing::err_fn)?;
    println!("Successfully built streams.");

    // Play the streams.
    println!(
        "Starting the input and output streams with `{}` milliseconds of latency.",
        latency_ms
    );
    input_stream.play()?;
    output_stream.play()?;

    use std::time::Duration;
    let console_req_sender = req_sender.clone();
    thread::spawn(move || {
        let (resp_sender, resp_receiver) = mpsc::channel();
        console_req_sender
            .send(audioplumbing::Request::StatsRequest {
                resp: resp_sender.clone(),
            })
            .unwrap();
        while let Ok(fs) = resp_receiver.recv() {
            println!("f{}", fs.format_stats());
            thread::sleep(Duration::from_millis(100));
            console_req_sender
                .send(audioplumbing::Request::StatsRequest {
                    resp: resp_sender.clone(),
                })
                .unwrap();
        }
    });

    rocket::ignite()
        .mount(
            "/",
            routes![
                webui::index,
                webui::stats,
                webui::hi_world,
                webui::dart_files
            ],
        )
        .manage(req_sender)
        .launch();

    Ok(())
}
