use std::sync::mpsc;
use std::sync::Arc;

pub enum Request {
    StatsRequest { resp: mpsc::Sender<FrameStats> },
}

pub fn err_fn(err: cpal::StreamError) {
    eprintln!("an error occurred on stream: {}", err);
}

pub struct DumbLooper<'a> {
    // Change to i32? 1ms is already only 48 samples?
    input_device: &'a cpal::Device,
    output_device: &'a cpal::Device,
    config: &'a cpal::StreamConfig,
    delay_ms: f32,
    input_stream: Option<cpal::Stream>,
    output_stream: Option<cpal::Stream>,
}
use cpal;
impl<'a> DumbLooper<'a> {
    pub fn new(
        input_device: &'a cpal::Device,
        output_device: &'a cpal::Device,
        config: &'a cpal::StreamConfig,
        delay_ms: f32,
    ) -> Self {
        Self {
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
        chunk_sender: mpsc::Sender<Chunk>,
        fs_sender: mpsc::Sender<FrameStats>,
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
            producer.push(0.0).unwrap();
        }

        let mut previnp_callback: Option<cpal::StreamInstant> = None;
        let mut previnp_capture: Option<cpal::StreamInstant> = None;
        // TODO: use new_uninit() for efficiency, bundling count and chunk into a
        // struct.
        let mut chunk = new_chunk();
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
            let mut fs = FrameStats::new(channels);
            let mut iter = data.iter();
            while fs.consume_frame(&mut iter) {}
            fs_sender.send(fs).unwrap();

            let mut output_fell_behind = false;
            for &sample in data {
                if producer.push(sample).is_err() {
                    output_fell_behind = true;
                }
                if sample_count >= CHUNK_SIZE {
                    chunk_sender.send(chunk.clone()).unwrap();
                    chunk = new_chunk();
                    sample_count = 0;
                }
                // Review: is get_mut(...).unwrap() cheap? Can we somehow move
                // such binding to outside the for-loop, then somehow unbind
                // when we want to send(chunk.clone())?
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
            self.config
        );

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
