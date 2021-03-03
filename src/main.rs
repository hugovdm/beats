//! Assumes that the input and output devices can use the same stream configuration and that they
//! support the f32 sample format.

#![feature(proc_macro_hygiene, decl_macro)]
#[macro_use]
extern crate rocket;

use cpal::traits::{DeviceTrait, HostTrait};
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

    use std::sync::mpsc;
    let (chunk_sender, chunk_receiver) = mpsc::channel();
    let (fs_sender, fs_receiver) = mpsc::channel();
    // Review: why can we pass a SyncSender to Rocket's .manage() but not a Sender?
    // let (req_sender, req_receiver) = mpsc::channel();
    let (req_sender, req_receiver) = mpsc::sync_channel(100);
    let mut coordinator: audioplumbing::Coordinator<f32> =
        audioplumbing::Coordinator::new(chunk_receiver, req_receiver, fs_receiver);
    thread::spawn(move || coordinator.run());

    let mut looper =
        audioplumbing::DumbLooper::new(&input_device, &output_device, &config, latency_ms);
    looper.play(chunk_sender.clone(), fs_sender.clone())?;

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
