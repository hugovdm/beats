//! Assumes that the input and output devices can use the same stream configuration and that they
//! support the f32 sample format.

#![feature(proc_macro_hygiene, decl_macro)]

#[macro_use]
extern crate rocket;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use std::thread;

mod audiopipes {
    pub fn err_fn(err: cpal::StreamError) {
        eprintln!("an error occurred on stream: {}", err);
    }

    const MAX_CHANNELS: usize = 2;
    #[derive(Clone, Debug)]
    pub struct FrameStats {
        channels: cpal::ChannelCount,
        max: [f32; MAX_CHANNELS as usize],
        max_cnt: [u32; MAX_CHANNELS as usize],
        acc: [f32; MAX_CHANNELS as usize],
        cnt: [u32; MAX_CHANNELS as usize],
    }
    impl FrameStats {
        pub fn new(channels: cpal::ChannelCount) -> FrameStats {
            FrameStats {
                channels: channels,
                max: [0.0; MAX_CHANNELS],
                max_cnt: [0; MAX_CHANNELS],
                acc: [0.0; MAX_CHANNELS],
                cnt: [0; MAX_CHANNELS],
            }
        }

        /// Collects statistics from `feed`.
        //
        // Partially generic version of the function below:
        //
        // fn consume_frame<'a, I>(&mut self, feed: &mut I) -> bool
        // where
        //     I: Iterator<Item = &'a f32>,
        // { ... }
        //
        // Parameterising out f32 might allow this to function with e.g. u16 or u32
        // too. However it should be parameterised out at the FrameStats level.
        pub fn consume_frame<'a>(&mut self, feed: &mut impl Iterator<Item = &'a f32>) -> bool {
            for c in 0..self.channels as usize {
                match feed.next() {
                    Some(s) => {
                        if c > MAX_CHANNELS {
                            continue;
                        }
                        let ss = s * s;
                        self.acc[c] += ss;
                        self.cnt[c] += 1;
                        if ss > self.max[c] {
                            self.max[c] = ss;
                            self.max_cnt[c] = 1;
                        } else if ss == self.max[c] {
                            self.max_cnt[c] += 1;
                        }
                    }
                    None => return false,
                }
            }
            return true;
        }

        pub fn format_stats(&self) -> String {
            let mut s = String::with_capacity(120);
            for c in 0..self.channels as usize {
                s.push_str(" | ");
                let rms = (self.acc[c] / self.cnt[c] as f32).sqrt();
                s.push_str(&format!(
                    "max: {:.3}, max_cnt: {:3}, rms: {:.3}, cnt: {} ",
                    self.max[c].sqrt(),
                    self.max_cnt[c],
                    rms,
                    self.cnt[c]
                ));
                let bar_len = (50.0 * rms) as u32;
                for _ in 0..bar_len {
                    s.push('#');
                }
                for _ in bar_len..50 {
                    s.push(' ');
                }
            }
            s
        }
    }

    use std::sync::mpsc;
    pub enum Request {
        StatsRequest { resp: mpsc::Sender<FrameStats> },
    }

    use std::sync::Arc;
    pub const CHUNK_SIZE: usize = 8192;
    const MAX_CHUNKS: usize = 1024;
    const MAX_STATS: usize = 100;
    type Chunk = Arc<[f32; CHUNK_SIZE]>;
    pub fn new_chunk() -> Chunk {
        Arc::new([0.0; CHUNK_SIZE])
    }
    pub struct Coordinator {
        chunk_receiver: mpsc::Receiver<Chunk>,
        request_receiver: mpsc::Receiver<Request>,
        frame_stats_receiver: mpsc::Receiver<FrameStats>,
        chunks: Vec<Chunk>,
        chunks_pos: usize,
        stats: Vec<FrameStats>,
        stats_pos: usize,
    }
    impl Coordinator {
        pub fn new(
            chunk_receiver: mpsc::Receiver<Chunk>,
            request_receiver: mpsc::Receiver<Request>,
            frame_stats_receiver: mpsc::Receiver<FrameStats>,
        ) -> Coordinator {
            Coordinator {
                chunk_receiver: chunk_receiver,
                request_receiver: request_receiver,
                frame_stats_receiver: frame_stats_receiver,
                chunks: Vec::with_capacity(MAX_CHUNKS),
                chunks_pos: 0,
                stats: Vec::with_capacity(MAX_STATS),
                stats_pos: 0,
            }
        }

        pub fn run(&mut self) {
            // Is it possible to call thread::spawn here instead of in the calling
            // code?
            //
            // (Simplicity of current code's approach: the thread owns all of &self.)

            // FIXME: Is it possible to select between multiple channels: block
            // until any of them have received data? The loop is currently very
            // hacky: we do a blocking read for frame stats, relying on them being
            // sent regularly (unlike requests). We also expect they're sent more
            // often than chunks.
            loop {
                while let Ok(chunk) = self.chunk_receiver.try_recv() {
                    self.chunks_pos = self.chunks_pos + 1;
                    if self.chunks_pos >= self.chunks.len() {
                        self.chunks.push(chunk);
                    } else {
                        self.chunks[self.chunks_pos] = chunk;
                    }
                }
                while let Ok(req) = self.request_receiver.try_recv() {
                    match req {
                        Request::StatsRequest { resp } => {
                            resp.send(self.stats[self.stats_pos - 1].clone()).unwrap()
                        }
                    }
                }
                match self.frame_stats_receiver.recv() {
                    Ok(fs) => {
                        self.stats_pos = self.stats_pos + 1;
                        if self.stats_pos >= self.stats.len() {
                            self.stats.push(fs);
                        } else {
                            self.stats[self.stats_pos] = fs;
                        }
                    }
                    Err(why) => panic!("{:?}", why),
                }
            }
        }
    }
}

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
    let mut coordinator = audiopipes::Coordinator::new(chunk_receiver, req_receiver, receiver);
    thread::spawn(move || coordinator.run());

    let channels = config.channels;

    let mut previnp_callback: Option<cpal::StreamInstant> = None;
    let mut previnp_capture: Option<cpal::StreamInstant> = None;
    // TODO: use new_uninit() for efficiency, bundling count and chunk into a
    // struct.
    let mut chunk = audiopipes::new_chunk();
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
        let mut fs = audiopipes::FrameStats::new(channels);
        let mut iter = data.iter();
        while fs.consume_frame(&mut iter) {}
        sender.send(fs).unwrap();

        let mut output_fell_behind = false;
        for &sample in data {
            if producer.push(sample).is_err() {
                output_fell_behind = true;
            }
            if sample_count >= audiopipes::CHUNK_SIZE {
                chunk_sender.send(chunk.clone()).unwrap();
                chunk = audiopipes::new_chunk();
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
        input_device.build_input_stream(&config, input_data_fn, audiopipes::err_fn)?;
    let output_stream =
        output_device.build_output_stream(&config, output_data_fn, audiopipes::err_fn)?;
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
            .send(audiopipes::Request::StatsRequest {
                resp: resp_sender.clone(),
            })
            .unwrap();
        while let Ok(fs) = resp_receiver.recv() {
            println!("f{}", fs.format_stats());
            thread::sleep(Duration::from_millis(100));
            console_req_sender
                .send(audiopipes::Request::StatsRequest {
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

mod webui {
    use super::audiopipes;
    use rocket::{
        response::{content, NamedFile},
        State,
    };
    use std::path::{Path, PathBuf};
    use std::sync::mpsc::{channel, SyncSender};

    #[get("/")]
    pub fn index() -> content::Html<String> {
        content::Html(format!(
            "Hello, world!\
            <ul>\
            <li><a href=\"dart/index.html\">dart index.html</a></li>\
            <li><a href=\"stats\">stats</a>\
            </ul>"
        ))
    }

    #[get("/stats")]
    pub fn stats(state: State<SyncSender<audiopipes::Request>>) -> String {
        let (resp_sender, resp_receiver) = channel();
        state
            .send(audiopipes::Request::StatsRequest {
                resp: resp_sender.clone(),
            })
            .unwrap();
        match resp_receiver.recv() {
            Ok(fs) => fs.format_stats(),
            Err(why) => format!("error receiving stats: {:?}", why),
        }
    }

    #[get("/hello_world")] // <- route attribute
    pub fn hi_world() -> &'static str {
        "hello, world!"
    }

    #[get("/dart/<file..>")]
    pub fn dart_files(file: PathBuf) -> Option<NamedFile> {
        NamedFile::open(Path::new("../dart/build/").join(file)).ok()
    }
}
