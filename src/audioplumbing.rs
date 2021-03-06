use std::sync::mpsc;
use std::sync::Arc;

pub trait Sample: cpal::Sample + PartialEq + PartialOrd + Send + std::fmt::Display {}

impl Sample for f32 {}

pub enum CoordinatorRequest<T: Sample> {
    StatsRequest { resp: mpsc::Sender<FrameStats<T>> },
}

#[derive(Debug)]
pub enum ControllerError<T: Sample> {
    SendError {
        e: mpsc::SendError<CoordinatorRequest<T>>,
    },
    RecvError {
        e: mpsc::RecvError,
    },
}

pub type ControllerResult<T, U> = Result<T, ControllerError<U>>;

#[derive(Clone)]
pub struct Controller<T: Sample> {
    req_sender: mpsc::SyncSender<CoordinatorRequest<T>>,
}

/// Controls the Coordinator
impl<T: Sample> Controller<T> {
    pub fn new(req_sender: mpsc::SyncSender<CoordinatorRequest<T>>) -> Self {
        Self { req_sender }
    }

    pub fn get_frame_stats(&self) -> ControllerResult<FrameStats<T>, T> {
        let (resp_sender, resp_receiver) = mpsc::channel();
        let req = CoordinatorRequest::StatsRequest {
            resp: resp_sender.clone(),
        };
        match self.req_sender.send(req) {
            Err(why) => Err(ControllerError::SendError { e: why }),
            Ok(()) => match resp_receiver.recv() {
                Err(why) => Err(ControllerError::RecvError { e: why }),
                Ok(result) => Ok(result),
            },
        }
    }
}

fn err_fn(err: cpal::StreamError) {
    eprintln!("an error occurred on stream: {}", err);
}

pub struct AudioPlumbing<'a> {
    // Change to i32? 1ms is already only 48 samples?
    input_device: &'a cpal::Device,
    output_device: &'a cpal::Device,
    config: &'a cpal::StreamConfig,
    delay_ms: f32,
    input_stream: Option<cpal::Stream>,
    output_stream: Option<cpal::Stream>,
}

impl<'a> AudioPlumbing<'a> {
    pub fn new(
        input_device: &'a cpal::Device,
        output_device: &'a cpal::Device,
        config: &'a cpal::StreamConfig,
        delay_ms: f32,
    ) -> Self {
        AudioPlumbing {
            input_device,
            output_device,
            config,
            delay_ms,
            input_stream: None,
            output_stream: None,
        }
    }

    pub fn play(
        &mut self,
        chunk_sender: mpsc::Sender<Chunk<f32>>,
        fs_sender: mpsc::Sender<FrameStats<f32>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        // Create a delay in case the input and output devices aren't synced.
        let channels = self.config.channels;
        let latency_frames = (self.delay_ms / 1_000.0) * self.config.sample_rate.0 as f32;
        let latency_samples = latency_frames as usize * channels as usize;

        // The buffer to share samples
        use ringbuf::RingBuffer;
        let ring = RingBuffer::new(latency_samples * 2);
        let (mut producer, mut consumer) = ring.split();

        // Fill the samples with 0.0 equal to the length of the delay.
        for _ in 0..latency_samples {
            // The ring buffer has twice as much space as necessary to add latency here,
            // so this should never fail
            producer.push(cpal::Sample::from::<f32>(&0.0)).unwrap();
        }

        let mut inp_diagnostics = CpalDiagnostics::empty("input");
        let mut out_diagnostics = CpalDiagnostics::empty("output");

        // TODO: use new_uninit() for efficiency, bundling count and chunk into a
        // struct.
        let mut chunk = new_chunk();
        let mut sample_count: usize = 0;
        let input_data_fn = move |data: &[f32], inpinf: &cpal::InputCallbackInfo| {
            // TODO: move to stats that are available on-demand:
            // println!("input count {}    info: {:?}", data.len(), inpinf);
            inp_diagnostics
                .diagnose_capture(inpinf.timestamp().callback, inpinf.timestamp().capture);
            // TODO: don't reset fs every period.
            let mut fs = FrameStats::new(channels);
            let mut output_fell_behind = false;
            assert_eq!(data.len() % channels as usize, 0);
            for frame in data.chunks_exact(channels.into()) {
                {
                    fs.consume_frame(&mut frame.iter());
                }
                for &sample in frame {
                    if producer.push(sample).is_err() {
                        // TODO: check that this only happens on frame boundaries
                        output_fell_behind = true;
                    }

                    if sample_count >= CHUNK_SIZE {
                        chunk_sender.send(chunk.clone()).unwrap();
                        chunk = new_chunk();
                        sample_count = 0;
                    }

                    // Review: is get_mut(...).unwrap() cheap? Can we somehow
                    // move such binding to outside the for-loop, then somehow
                    // unbind when we want to send(chunk.clone())? drop()?
                    let c = Arc::get_mut(&mut chunk).unwrap();
                    c[sample_count] = sample;
                    sample_count = sample_count + 1;
                }
            }
            // Sending fs: this is a move. We can't use it again. Different
            // lifetime conditions than chunk.
            fs_sender.send(fs).unwrap();
            if output_fell_behind {
                eprintln!("output stream fell behind: try increasing latency");
            }
        };

        let output_data_fn = move |data: &mut [f32], outinf: &cpal::OutputCallbackInfo| {
            // println!("output count {} info {:?}", data.len(), outinf);
            let mut input_fell_behind = false;
            out_diagnostics
                .diagnose_playback(outinf.timestamp().callback, outinf.timestamp().playback);
            assert_eq!(data.len() % channels as usize, 0);
            for frame in data.chunks_exact_mut(channels.into()) {
                for sample in frame {
                    *sample = match consumer.pop() {
                        Some(s) => s,
                        None => {
                            input_fell_behind = true;
                            cpal::Sample::from::<f32>(&0.0)
                        }
                    };
                }
            }
            if input_fell_behind {
                eprintln!("input stream fell behind: try increasing latency");
            }
        };

        // Build streams.
        println!(
            "Attempting to build both streams with f32 samples and `{:?}`.",
            self.config
        );

        // These streams can live as long as &self, so the input_data_fn and
        // output_data_fn closures, and thus the data they capture, need to
        // also live that long. (Function parameters in input_data_fn and
        // output_data_fn need live only as long as the function invocation.)
        use cpal::traits::DeviceTrait;
        self.input_stream = Some(self.input_device.build_input_stream(
            self.config,
            input_data_fn,
            err_fn,
        )?);
        self.output_stream = Some(self.output_device.build_output_stream(
            self.config,
            output_data_fn,
            err_fn,
        )?);
        println!("Successfully built streams.");

        // Play the streams.
        println!(
            "Starting the input and output streams with `{}` milliseconds of latency.",
            self.delay_ms,
        );
        use cpal::traits::StreamTrait;
        self.input_stream.as_ref().unwrap().play()?;
        self.output_stream.as_ref().unwrap().play()?;

        Ok(())
    }
}

struct CpalDiagnostics {
    ident: &'static str,
    callback: Option<cpal::StreamInstant>,
    capture: Option<cpal::StreamInstant>,
    playback: Option<cpal::StreamInstant>,
    first: Option<cpal::StreamInstant>,
}

impl CpalDiagnostics {
    fn empty(ident: &'static str) -> Self {
        Self {
            ident,
            callback: None,
            capture: None,
            playback: None,
            first: None,
        }
    }

    fn diagnose_capture(&mut self, callback: cpal::StreamInstant, capture: cpal::StreamInstant) {
        if self.first == None {
            if callback.duration_since(&capture).is_some() {
                self.first = Some(capture);
            } else {
                assert_eq!(capture.duration_since(&callback).is_some(), true);
                self.first = Some(callback);
            }
        }
        print!(
            "{} timestamps: callback: {:?}, capture: {:?}",
            self.ident,
            callback.duration_since(&self.first.unwrap()).unwrap(),
            capture.duration_since(&self.first.unwrap()).unwrap(),
        );
        if self.callback != None && self.capture != None {
            println!(
                " - diffs callback: {:?}, capture: {:?}",
                callback.duration_since(&self.callback.unwrap()).unwrap(),
                capture.duration_since(&self.capture.unwrap()).unwrap(),
            );
        } else {
            println!();
        }
        self.callback = Some(callback);
        self.capture = Some(capture);
    }

    fn diagnose_playback(&mut self, callback: cpal::StreamInstant, playback: cpal::StreamInstant) {
        if self.first == None {
            if callback.duration_since(&playback).is_some() {
                self.first = Some(playback);
            } else {
                assert_eq!(playback.duration_since(&callback).is_some(), true);
                self.first = Some(callback);
            }
        }
        print!(
            "{} timestamps: callback: {:?}, playback: {:?}",
            self.ident,
            callback.duration_since(&self.first.unwrap()).unwrap(),
            playback.duration_since(&self.first.unwrap()).unwrap(),
        );
        if self.callback != None && self.playback != None {
            println!(
                " - diffs callback: {:?}, playback: {:?}",
                callback.duration_since(&self.callback.unwrap()).unwrap(),
                playback.duration_since(&self.playback.unwrap()).unwrap(),
            );
        } else {
            println!();
        }
        self.callback = Some(callback);
        self.playback = Some(playback);
    }
}

const MAX_CHANNELS: usize = 2;
const SAMPLE_SCALE_FACTOR: i16 = 256;
#[derive(Clone, Debug)]
pub struct FrameStats<T: Sample> {
    channels: cpal::ChannelCount,
    max: [T; MAX_CHANNELS as usize],
    max_cnt: [u32; MAX_CHANNELS as usize],
    min: [T; MAX_CHANNELS as usize],
    min_cnt: [u32; MAX_CHANNELS as usize],
    acc: [u32; MAX_CHANNELS as usize],
    cnt: [u32; MAX_CHANNELS as usize],
}

impl<T: Sample> FrameStats<T> {
    fn new(channels: cpal::ChannelCount) -> FrameStats<T> {
        FrameStats {
            channels: channels,
            // Review: is there a better way to implement "0 as T"?
            max: [T::from::<i16>(&0); MAX_CHANNELS],
            max_cnt: [0; MAX_CHANNELS],
            // Review: is there a better way to implement "0 as T"?
            min: [T::from(&(0 as i16)); MAX_CHANNELS],
            min_cnt: [0; MAX_CHANNELS],
            acc: [0; MAX_CHANNELS],
            cnt: [0; MAX_CHANNELS],
        }
    }
}

impl<'a, T: 'a + Sample> FrameStats<T> {
    fn consume_frame(&mut self, feed: &mut impl Iterator<Item = &'a T>) -> bool {
        for c in 0..self.channels as usize {
            match feed.next() {
                Some(s) => {
                    if c > MAX_CHANNELS {
                        continue;
                    }
                    if *s > self.max[c] {
                        self.max[c] = *s;
                        self.max_cnt[c] = 1;
                    } else if *s < self.min[c] {
                        self.min[c] = *s;
                        self.min_cnt[c] = 1;
                    } else {
                        if *s == self.max[c] {
                            self.max_cnt[c] += 1;
                        }
                        if *s == self.min[c] {
                            self.min_cnt[c] += 1;
                        }
                    }
                    let s: i16 = cpal::Sample::from::<T>(s);
                    let s = s / SAMPLE_SCALE_FACTOR;
                    let ss = (s * s) as u32;
                    self.acc[c] += ss; // TODO: deal with overflow!
                    self.cnt[c] += 1;
                }
                None => return false,
            }
        }
        return true;
    }
}

// TODO: we need only number_traits::Sqrt;
impl<T: Sample> FrameStats<T> {
    pub fn format_stats(&self) -> String {
        const BAR_SIZE: i32 = 40;
        let mut s = String::with_capacity(120);
        for c in 0..self.channels as usize {
            s.push_str(" | ");
            let rms = ((self.acc[c] as f32 / self.cnt[c] as f32).sqrt()
                * SAMPLE_SCALE_FACTOR as f32) as i16;
            let bar_len = rms as i32 * BAR_SIZE / ::std::i16::MAX as i32;
            let rms: T = cpal::Sample::from(&rms);
            s.push_str(&format!(
                "min-max: {:.3}-{:.3}; cnts: {:3},{:3}; rms: {:.3}, cnt: {} ",
                self.min[c], self.max[c], self.min_cnt[c], self.max_cnt[c], rms, self.cnt[c]
            ));
            for _ in 0..bar_len {
                s.push('#');
            }
            for _ in bar_len..BAR_SIZE {
                s.push(' ');
            }
        }
        s
    }
}

const CHUNK_SIZE: usize = 8192;
const MAX_CHUNKS: usize = 1024;
const MAX_STATS: usize = 100;
type Chunk<T> = Arc<[T; CHUNK_SIZE]>;
fn new_chunk<T: Sample>() -> Chunk<T> {
    Arc::new([T::from::<i16>(&0); CHUNK_SIZE])
}
pub struct Coordinator<T: Sample> {
    chunk_receiver: mpsc::Receiver<Chunk<T>>,
    request_receiver: mpsc::Receiver<CoordinatorRequest<T>>,
    frame_stats_receiver: mpsc::Receiver<FrameStats<T>>,
    chunks: Vec<Chunk<T>>,
    chunks_pos: usize,
    stats: Vec<FrameStats<T>>,
    stats_pos: usize,
}
impl<T: Sample> Coordinator<T> {
    pub fn new(
        chunk_receiver: mpsc::Receiver<Chunk<T>>,
        request_receiver: mpsc::Receiver<CoordinatorRequest<T>>,
        frame_stats_receiver: mpsc::Receiver<FrameStats<T>>,
    ) -> Coordinator<T> {
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
                    CoordinatorRequest::StatsRequest { resp } => {
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
