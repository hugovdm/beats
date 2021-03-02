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
