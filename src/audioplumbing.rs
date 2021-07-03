use std::ops::AddAssign;
use std::sync::mpsc;
use std::sync::Arc;

const BUCKETS: [f64; 14] = [
    100., 200., 400., 700., 1000., 1300., 1600., 1900., 2200., 2500., 2800., 3100., 3500., 4000.,
];
lazy_static! {
    static ref AUDIO_INPUT_BLOCK_SAMPLES_HISTOGRAM: prometheus::Histogram = register_histogram!(
        "audio_input_block_samples",
        "Number of input audio samples in each input block",
        BUCKETS.to_vec()
    )
    .unwrap();
    static ref AUDIO_OUTPUT_BLOCK_SAMPLES_HISTOGRAM: prometheus::Histogram = register_histogram!(
        "audio_output_block_samples",
        "Number of output audio samples in each output block",
        BUCKETS.to_vec()
    )
    .unwrap();
}

pub trait Sample:
    cpal::Sample + PartialEq + PartialOrd + AddAssign + Send + std::fmt::Display + std::fmt::Debug
{
}

impl Sample for f32 {}
impl Sample for i16 {}
impl Sample for u16 {}

/// Requests for the Coordinator.
pub enum CoordinatorRequest<T: Sample> {
    StatsRequest { resp: mpsc::Sender<FrameStats<T>> },
    StatusInfoRequest { resp: mpsc::Sender<StatusInfo<T>> },
}

/// Errors produced by the Controller.
#[derive(Debug)]
pub enum ControllerError<T: Sample> {
    SendError {
        e: mpsc::SendError<CoordinatorRequest<T>>,
    },
    RecvError {
        e: mpsc::RecvError,
    },
}

/// Results produced by the Controller.
pub type ControllerResult<T, U> = Result<T, ControllerError<U>>;

/// Controls the Coordinator
#[derive(Clone)]
pub struct Controller<T: Sample> {
    req_sender: mpsc::SyncSender<CoordinatorRequest<T>>,
}

/// FIXME: does this show up somewhere in rustdoc?
impl<T: Sample> Controller<T> {
    pub fn new(req_sender: mpsc::SyncSender<CoordinatorRequest<T>>) -> Self {
        Self { req_sender }
    }

    /// Makes a StatsRequest for the Coordinator.
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

    pub fn get_status(&self) -> ControllerResult<StatusInfo<T>, T> {
        let (resp_sender, resp_receiver) = mpsc::channel();
        let req = CoordinatorRequest::StatusInfoRequest {
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

use ringbuf::RingBuffer;

struct SimpleLooper<T: Sample> {
    channels: cpal::ChannelCount,
    producer: ringbuf::Producer<T>,
    consumer: ringbuf::Consumer<T>,
}

impl<'b, T: Sample + 'b> SimpleLooper<T> {
    fn new(delay_ms: f32, config: &cpal::StreamConfig) -> Self {
        let latency_frames = (delay_ms / 1_000.0) * config.sample_rate.0 as f32;
        let latency_samples = latency_frames as usize * config.channels as usize;

        // The buffer to share samples
        let ring = RingBuffer::<T>::new(latency_samples * 2);
        let (mut producer, consumer) = ring.split();

        // Fill the samples with 0.0 equal to the length of the delay.
        for _ in 0..latency_samples {
            // The ring buffer has twice as much space as necessary to add latency here,
            // so this should never fail
            producer.push(cpal::Sample::from::<f32>(&0.0)).unwrap();
        }

        SimpleLooper {
            channels: config.channels,
            producer,
            consumer,
        }
    }

    fn get_data_callbacks(
        self,
    ) -> (
        impl FnMut(&[T], &cpal::InputCallbackInfo) + 'b,
        impl FnMut(&mut [T], &cpal::OutputCallbackInfo) + 'b,
    ) {
        let channels = self.channels;
        let mut producer = self.producer;
        let mut consumer = self.consumer;

        let inp_fn = move |data: &[T], _: &cpal::InputCallbackInfo| {
            SimpleLooper::<T>::input_fn(channels, &mut producer, data)
        };

        let outp_fn = move |data: &mut [T], _: &cpal::OutputCallbackInfo| {
            SimpleLooper::<T>::output_fn(channels, &mut consumer, data)
        };

        (inp_fn, outp_fn)
    }

    /// Processes input data
    ///
    /// For efficiency purposes, accepts a whole slice of data rather than only
    /// one frame at a time. TODO: performance testing a version of this which
    /// calls input_fn for each frame?
    fn input_fn(channels: cpal::ChannelCount, producer: &'b mut ringbuf::Producer<T>, data: &[T]) {
        assert_eq!(data.len() % channels as usize, 0);
        let mut output_fell_behind = false;
        // TODO: chunking not necessary. Check benchmark results?
        for frame in data.chunks_exact(channels.into()) {
            for &sample in frame {
                if producer.push(sample).is_err() {
                    // TODO: check that this only happens on frame boundaries
                    output_fell_behind = true;
                }
            }
        }
        if output_fell_behind {
            eprintln!(
                "thread {}: SimpleLooper output stream fell behind: try increasing latency",
                thread_id::get()
            );
        }
    }

    fn output_fn(
        channels: cpal::ChannelCount,
        consumer: &'b mut ringbuf::Consumer<T>,
        data: &mut [T],
    ) {
        assert_eq!(data.len() % channels as usize, 0);
        let mut input_fell_behind = false;
        // TODO: chunking not necessary. Check benchmark results?
        for frame in data.chunks_exact_mut(channels.into()) {
            for sample in frame {
                *sample += match consumer.pop() {
                    Some(s) => s,
                    None => {
                        // TODO: check that this only happens on frame
                        // boundaries?
                        input_fell_behind = true;
                        cpal::Sample::from::<f32>(&0.0)
                    }
                };
            }
        }
        if input_fell_behind {
            eprintln!(
                "thread {}: SimpleLooper input stream fell behind: try increasing latency",
                thread_id::get()
            );
        }
    }
}

fn get_data_callback<'a>(
    channels: cpal::ChannelCount,
    chunk_sender: mpsc::Sender<Arc<ChunkData<f32>>>,
) -> impl FnMut(&[f32]) + 'a {
    let mut chunk = new_chunk(channels);
    let mut sample_count: usize = 0;
    let mut env_idx: usize = 0;
    // TODO: use new_unint() for efficiency:
    let mut env_acc: [u64; MAX_CHANNELS] = [0; MAX_CHANNELS];
    let mut env_count: usize = 0;
    let inp_fn = move |data: &[f32]| {
        input_fn(
            channels,
            &chunk_sender,
            &mut chunk,
            &mut sample_count,
            &mut env_idx,
            &mut env_acc,
            &mut env_count,
            data,
        )
    };
    inp_fn
}

fn input_fn<'a>(
    channels: cpal::ChannelCount,
    chunk_sender: &'a mpsc::Sender<Arc<ChunkData<f32>>>,
    chunk: &'a mut Arc<ChunkData<f32>>,
    sample_count: &'a mut usize,
    env_idx: &'a mut usize,
    env_acc: &'a mut [u64; MAX_CHANNELS],
    env_count: &'a mut usize,
    data: &[f32],
) {
    // eprintln!("input_fn with data len {}", data.len());
    if data.len() == 0 {
        println!("Dropping chunk_sender!");
        drop(chunk_sender);
        return;
    }
    assert_eq!(data.len() % channels as usize, 0);
    for frame in data.chunks_exact(channels.into()) {
        // Review: is get_mut(...).unwrap() cheap? Can we somehow
        // move such binding to outside the for-loop, then somehow
        // unbind when we want to send(chunk.clone())? drop()?
        //
        // TODO: Error message for calling Arc::get_mut(&mut chunk) here is
        // confusing:
        let c = Arc::get_mut(chunk).unwrap();

        // TODO: benchmark! Float ops vs integer ops, on PC and on ARM
        // (Raspberry Pi). Currently we're calculating RMS via u64: samples
        // converted from f32 to i16, squaring in i32, summing in u64. then sqrt
        // via f32 again, back to i16, and converting back to f32.
        let mut chan = 0; // TODO: figure out enumerate() for (i, &sample)?
        for &sample in frame {
            c.get_audio()[*sample_count] = sample;
            *sample_count += 1;
            let sample_i16: i16 = cpal::Sample::from::<f32>(&sample);
            let s = sample_i16 as i32;
            let ss = s * s;
            env_acc[chan] += ss as u64;
            chan += 1;
            // TODO: when doing a benchmark with f32, check impact on
            // calculation accuracy too. This println shows the int calculation:
            //
            // println!(
            //     "sample {} -> {}; s {}, ss {}, back: {}, back back: {}",
            //     sample,
            //     sample_i16,
            //     s,
            //     ss,
            //     ((ss as f32).sqrt()) as i16,
            //     <i16 as cpal::Sample>::to_f32(&(((ss as f32).sqrt()) as i16))
            // );
        }
        *env_count += 1;

        if *env_count == DOWNSAMPLE_WINDOW {
            for chan in 0..channels.into() {
                let rms = ((env_acc[chan] as f32 / DOWNSAMPLE_WINDOW as f32).sqrt()) as i16;
                c.get_envelope()[*env_idx] = cpal::Sample::from::<i16>(&rms);
                env_acc[chan] = 0;
                *env_idx += 1;
            }
            *env_count = 0;
        }

        if *sample_count >= CHUNK_SIZE {
            assert_eq!(*env_count, 0);
            assert_eq!(*env_idx, ENV_SIZE);
            chunk_sender.send(chunk.clone()).unwrap();
            *chunk = new_chunk(channels);
            *sample_count = 0;
            *env_idx = 0;
        }
    }
}

/// Handles CPAL stream errrors.
fn err_fn(err: cpal::StreamError) {
    eprintln!(
        "thread {}: an error occurred on stream: {}",
        thread_id::get(),
        err
    );
}

/// Owns CPAL-related instances and data.
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

    /// Creates closures that run in cpal-managed threads.
    ///
    /// Sends clones of the chunks via chunk_sender (perhaps to a Coordinator?).
    /// Sends FrameStats via fs_sender.
    pub fn play(
        &mut self,
        chunk_sender: mpsc::Sender<Arc<ChunkData<f32>>>,
        fs_sender: mpsc::Sender<FrameStats<f32>>,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let channels = self.config.channels;

        // let mut inp_diagnostics = CpalDiagnostics::empty("input");
        // let mut out_diagnostics = CpalDiagnostics::empty("output");

        let looper1 = SimpleLooper::<f32>::new(self.delay_ms, self.config);
        let (mut looper1_input_fn, mut looper1_output_fn) = looper1.get_data_callbacks();

        let mut chunker_input_fn = get_data_callback(channels, chunk_sender);

        // let mut chunk = new_chunk(channels);
        // let mut sample_count: usize = 0;
        // let mut env_idx: usize = 0;
        // // TODO: use new_unint() for efficiency:
        // let mut env_acc: [u32; MAX_CHANNELS] = [0; MAX_CHANNELS];
        // let mut env_count: usize = 0;
        let input_data_fn = move |data: &[f32], inpinf: &cpal::InputCallbackInfo| {
            AUDIO_INPUT_BLOCK_SAMPLES_HISTOGRAM.observe(data.len() as f64);

            // // TODO: move to stats that are available on-demand:
            // inp_diagnostics.diagnose_capture(
            //     inpinf.timestamp().callback,
            //     inpinf.timestamp().capture,
            //     data.len(),
            // );

            // TODO: don't reset fs every period.
            let mut fs = FrameStats::new(channels);
            assert_eq!(data.len() % channels as usize, 0);
            for frame in data.chunks_exact(channels.into()) {
                fs.consume_frame(&mut frame.iter());
            }
            // Sending fs: this is a move. We can't use it again. Different
            // lifetime conditions than chunk.
            fs_sender.send(fs).unwrap();

            chunker_input_fn(data);
            looper1_input_fn(data, inpinf);
        };

        let output_data_fn = move |data: &mut [f32], outinf: &cpal::OutputCallbackInfo| {
            // Performance thoughts: we don't want to call a bunch of functions
            // for every sample: function call overhead. We also don't want to
            // invalidate the fastest cache for every function call. Thus the
            // number of frames we want to process with each function call
            // probably depends on the L1 cache size on the processor we're
            // running on. Find good ways to benchmark and select "number of
            // frames per function call"?

            AUDIO_OUTPUT_BLOCK_SAMPLES_HISTOGRAM.observe(data.len() as f64);

            // out_diagnostics.diagnose_playback(
            //     outinf.timestamp().callback,
            //     outinf.timestamp().playback,
            //     data.len(),
            // );

            assert_eq!(data.len() % channels as usize, 0);
            // First we initialize the buffer, such that other function calls
            // have initialized values. (The buffer doesn't start out empty -
            // presumably past output values? Experiment with not initializing,
            // out of curiosity!)
            for frame in data.chunks_exact_mut(channels.into()) {
                for sample in frame {
                    *sample = cpal::Sample::from::<f32>(&0.0)
                }
            }
            looper1_output_fn(data, outinf);
        };

        // Build streams.
        println!(
            "thread {}: Attempting to build both streams with f32 samples and `{:?}`.",
            thread_id::get(),
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
        println!("thread {}: Successfully built streams.", thread_id::get());

        // Play the streams.
        println!(
            "thread {}: Starting the input and output streams with `{}` milliseconds of latency.",
            thread_id::get(),
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

    fn diagnose_capture(
        &mut self,
        callback: cpal::StreamInstant,
        capture: cpal::StreamInstant,
        samples: usize,
    ) {
        if self.first == None {
            if callback.duration_since(&capture).is_some() {
                self.first = Some(capture);
            } else {
                assert_eq!(capture.duration_since(&callback).is_some(), true);
                self.first = Some(callback);
            }
        }
        print!(
            "thread {}: {} count {} - timestamps: callback: {:?}, capture: {:?}",
            thread_id::get(),
            self.ident,
            samples,
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

    fn diagnose_playback(
        &mut self,
        callback: cpal::StreamInstant,
        playback: cpal::StreamInstant,
        samples: usize,
    ) {
        if self.first == None {
            if callback.duration_since(&playback).is_some() {
                self.first = Some(playback);
            } else {
                assert_eq!(playback.duration_since(&callback).is_some(), true);
                self.first = Some(callback);
            }
        }
        print!(
            "thread {}: {} count {} - timestamps: callback: {:?}, playback: {:?}",
            thread_id::get(),
            self.ident,
            samples,
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

pub struct StatusInfo<T> {
    pub saved_blocks: usize,
    pub last_chunk: Arc<ChunkData<T>>,
}

const MAX_CHANNELS: usize = 2;
// TODO: TO TEST: if SAMPLE_SCALE_FACTOR is too small, "(s * s) as u32" below
// may overflow. Demonstrate this in a test and fix the casting logic.
const SAMPLE_SCALE_FACTOR: i16 = 256;
#[derive(Clone, Debug)]
pub struct FrameStats<T: Sample> {
    channels: cpal::ChannelCount,
    max: [T; MAX_CHANNELS],
    max_cnt: [u32; MAX_CHANNELS],
    min: [T; MAX_CHANNELS],
    min_cnt: [u32; MAX_CHANNELS],
    acc: [u32; MAX_CHANNELS],
    cnt: [u32; MAX_CHANNELS],
}

impl<T: Sample> FrameStats<T> {
    fn new(channels: cpal::ChannelCount) -> FrameStats<T> {
        FrameStats {
            channels,
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
                    // See SAMPLE_SCALE_FACTOR TODO above.
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

pub trait ChunkTrait<'a, T: Sample> {
    /// Produces a new chunk
    fn new(channels: cpal::ChannelCount) -> Self;
    fn get_audio(&'a mut self) -> &'a mut [T];
    fn get_ro_audio(&'a self) -> &'a [T];
    fn get_envelope(&'a mut self) -> &'a mut [T];
    fn get_ro_envelope(&'a self) -> &'a [T];
    fn get_channels(&self) -> cpal::ChannelCount;
}

/// Number of samples to combine to produce comparatively low sampling rate
/// envelope, and for other calculations where lower sampling rate makes sense
/// for efficiency.
const DOWNSAMPLE_WINDOW: usize = 360;
/// Number of samples in the envelope in each Chunk.
const ENV_SIZE: usize = 32;
/// Total number of audio samples in a Chunk. This must be a multiple of the
/// number of channels (number of samples in a frame).
const CHUNK_SIZE: usize = DOWNSAMPLE_WINDOW * ENV_SIZE;
/// Maximum number of chunks a Coordinator will keep track of. (More chunks will
/// remain in memory as long as other references to them remain.)
const MAX_CHUNKS: usize = 1024;
/// Number of historical FrameStats a Coordinator holds onto.
const MAX_STATS: usize = 100;
/// Data associated with each chunk
pub struct ChunkData<T> {
    audio: [T; CHUNK_SIZE],
    envelope: [T; ENV_SIZE],
    channels: cpal::ChannelCount,
}

impl<'a, T: Sample> ChunkTrait<'a, T> for ChunkData<T> {
    fn new(channels: cpal::ChannelCount) -> Self {
        Self {
            // TODO: use new_uninit() for efficiency
            audio: [T::from::<i16>(&0); CHUNK_SIZE],
            envelope: [T::from::<i16>(&0); ENV_SIZE],
            channels,
        }
    }

    fn get_audio(&'a mut self) -> &'a mut [T] {
        &mut self.audio
    }

    fn get_ro_audio(&'a self) -> &'a [T] {
        &self.audio
    }

    fn get_envelope(&'a mut self) -> &'a mut [T] {
        &mut self.envelope
    }

    fn get_ro_envelope(&'a self) -> &'a [T] {
        &self.envelope
    }

    fn get_channels(&self) -> cpal::ChannelCount {
        self.channels
    }
}

/// A chunk of audio data who's lifetime is managed via reference counting.
// type Chunk<T> = Arc<ChunkData<T>>;
/// Allocates and returns a new Chunk.
fn new_chunk<T: Sample>(channels: cpal::ChannelCount) -> Arc<ChunkData<T>> {
    Arc::new(ChunkData {
        // TODO: use new_uninit() for efficiency
        audio: [T::from::<i16>(&0); CHUNK_SIZE],
        envelope: [T::from::<i16>(&0); ENV_SIZE],
        channels,
    })
}

/// Coordinates the flow of audio data and statistics through use of multiple
/// channels. Takes care of storing the audio data too (FIXME: I'll likely
/// eventually rename this again?).
pub struct Coordinator<T: Sample> {
    chunk_receiver: mpsc::Receiver<Arc<ChunkData<T>>>,
    request_receiver: mpsc::Receiver<CoordinatorRequest<T>>,
    frame_stats_receiver: mpsc::Receiver<FrameStats<T>>,
    chunks: Vec<Arc<ChunkData<T>>>,
    chunks_pos: usize,
    stats: Vec<FrameStats<T>>,
    stats_pos: usize,
}
impl<T: Sample> Coordinator<T> {
    pub fn new(
        chunk_receiver: mpsc::Receiver<Arc<ChunkData<T>>>,
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

        // This suggests using crossbeam instead:
        // https://users.rust-lang.org/t/how-to-select-channels/26096/14 - I
        // think I should switch to using a single channel in the coordinator
        // (perhaps pass the Coordinator to classes' constructor, such that they
        // can ask the Coordinator for a Sender).
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
                    CoordinatorRequest::StatusInfoRequest { resp } => {
                        let response = StatusInfo {
                            saved_blocks: self.chunks.len(),
                            last_chunk: self.chunks[self.chunks_pos - 1].clone(),
                        };
                        resp.send(response).unwrap()
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

#[cfg(test)]
mod tests {
    #[test]
    fn multi_channel_chunks() {
        let chunk = super::new_chunk::<i16>(1);
        assert_eq!(chunk.channels, 1);

        let chunk = super::new_chunk::<f32>(2);
        assert_eq!(chunk.channels, 2);

        let chunk = super::new_chunk::<u16>(3);
        assert_eq!(chunk.channels, 3);

        let chunk = super::new_chunk::<i16>(4);
        assert_eq!(chunk.channels, 4);
    }

    macro_rules! assert_delta {
        ($x:expr, $y:expr, $d:expr) => {
            assert!(
                ($x - $y).abs() < $d,
                "{} != {} despite permissible delta {}",
                $x,
                $y,
                $d
            );
        };
    }

    #[test]
    fn chunker() {
        use std::sync::mpsc;

        let (sender, receiver) = mpsc::channel();
        let mut chunker_input_fn = super::get_data_callback(2, sender);

        assert_eq!(super::DOWNSAMPLE_WINDOW, 360);
        chunker_input_fn(&[0.3; 360 * 3]);
        // // TODO: Why does this not work?
        // chunker_input_fn(&[]);
        chunker_input_fn(&[0.5; 360 * 3]);
        chunker_input_fn(&[0.1; 360 * 2]);
        let mut stereo = [0.1; 360 * 2];
        for i in 0..360 {
            stereo[i * 2] = 0.3;
        }
        chunker_input_fn(&stereo);
        chunker_input_fn(&[-0.2; 4000]);
        chunker_input_fn(&[0.9; 4000]);
        // // Useful when working with debug output inside input_fn:
        // chunker_input_fn(&[0.1, 0.3, 0.5, 0.9, -0.1, -0.9]);

        let chunk: super::Arc<super::ChunkData<f32>> = receiver.recv().unwrap();
        let mut expected1 = [0.3; super::CHUNK_SIZE];
        for i in 360 * 3..360 * 6 {
            expected1[i] = 0.5;
        }
        for i in 360 * 6..360 * 8 {
            expected1[i] = 0.1;
        }
        for i in 360 * 8..360 * 10 {
            if i % 2 == 0 {
                expected1[i] = 0.3;
            } else {
                expected1[i] = 0.1;
            }
        }
        for i in 360 * 10..7600 {
            expected1[i] = -0.2;
        }
        for i in 7600..super::CHUNK_SIZE {
            expected1[i] = 0.9;
        }
        println!("envelope: {:?}", chunk.envelope);
        // // Fail in order to ensure output is printed:
        // assert!(false);
        assert_eq!(chunk.audio, expected1);
        // Explicit envelope values, for comparison with other platforms or
        // implementations:
        assert_eq!(
            chunk.envelope,
            [
                0.29999694, 0.29999694, 0.41227454, 0.41227454, 0.49998474, 0.49998474, 0.09997864,
                0.09997864, 0.29999694, 0.09997864, 0.1999878, 0.1999878, 0.1999878, 0.1999878,
                0.1999878, 0.1999878, 0.1999878, 0.1999878, 0.1999878, 0.1999878, 0.6182135,
                0.6182135, 0.89999086, 0.89999086, 0.89999086, 0.89999086, 0.89999086, 0.89999086,
                0.89999086, 0.89999086, 0.89999086, 0.89999086
            ]
        );
        assert_delta!(chunk.envelope[0], 0.3, 0.00001);
        assert_delta!(chunk.envelope[1], 0.3, 0.00001);
        assert_delta!(
            chunk.envelope[2],
            (((0.3 as f32 * 0.3) + (0.5 * 0.5)) / 2.0).sqrt(),
            0.0001
        );
        assert_delta!(
            chunk.envelope[3],
            (((0.3 as f32 * 0.3) + (0.5 * 0.5)) / 2.0).sqrt(),
            0.0001
        );
        assert_delta!(chunk.envelope[4], 0.5, 0.00002);
        assert_delta!(chunk.envelope[5], 0.5, 0.00002);
        assert_delta!(chunk.envelope[6], 0.1, 0.00003);
        assert_delta!(chunk.envelope[7], 0.1, 0.00003);
        assert_delta!(chunk.envelope[8], 0.3, 0.00001);
        assert_delta!(chunk.envelope[9], 0.1, 0.00003);
        assert_delta!(chunk.envelope[10], 0.2, 0.00002);
        assert_delta!(chunk.envelope[11], 0.2, 0.00002);
    }
}
