//! jt-engine — a reusable, headless client session.
//!
//! Pipeline:
//! ```text
//! mic ──▶ cpal input stream ──▶ mpsc channel ──▶ UDP sender thread ──▶ server
//!                                                                        │
//! speaker ◀── cpal output stream ◀── SampleFifo (jitter) ◀── UDP recv ◀──┘
//! ```
//!
//! Threading note (important for low latency): the cpal audio callbacks run on
//! a high-priority audio thread and must never block on the network. We copy
//! samples into a bounded `mpsc` channel (dropping on overflow, which is the
//! correct behaviour for realtime audio) and let dedicated threads do the I/O.

use std::collections::VecDeque;
use std::net::{SocketAddr, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use jt_core::{AudioBlock, Sample};

/// Target jitter depth in samples (~one 128-sample block). Playback stays
/// silent until this much data has buffered, smoothing early jitter.
const TARGET_DEPTH: usize = 128;

/// Upper bound in samples. When the buffer exceeds this, drop the oldest
/// samples so latency stays bounded even if the network briefly stalls.
const MAX_DEPTH: usize = 4096;

/// Bounded queue capacity (in blocks) between the audio callback and the
/// network thread. Overflow drops the block — correct realtime behaviour.
const CHANNEL_BOUND: usize = 8;

/// Sample-level FIFO shared between the network receive thread and the output
/// audio callback.
#[derive(Clone)]
struct SampleFifo {
    inner: Arc<Mutex<VecDeque<Sample>>>,
}

impl SampleFifo {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(VecDeque::new())),
        }
    }

    fn push(&self, samples: &[Sample]) {
        let mut buf = self.inner.lock().unwrap();
        buf.extend(samples.iter().copied());
        while buf.len() > MAX_DEPTH {
            buf.pop_front();
        }
    }

    fn pop(&self) -> Sample {
        let mut buf = self.inner.lock().unwrap();
        if buf.len() < TARGET_DEPTH {
            0 // silence while under target depth
        } else {
            buf.pop_front().unwrap_or(0)
        }
    }

    fn depth(&self) -> usize {
        self.inner.lock().unwrap().len()
    }
}

/// A running client session. Dropping it stops streaming and joins threads.
pub struct ClientSession {
    stop: Arc<AtomicBool>,
    tx: Option<SyncSender<Vec<Sample>>>,
    _socket: Arc<UdpSocket>,
    _streams: Vec<cpal::Stream>,
    handles: Vec<JoinHandle<()>>,
    fifo: SampleFifo,
    rtt_ms: Arc<AtomicU64>,
    sample_rate: f64,
    buffer_frames: f64,
}

impl ClientSession {
    /// Start capturing, streaming to `server`, and playback.
    pub fn start(server: SocketAddr) -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();

        let in_dev = host
            .default_input_device()
            .ok_or("no input device found")?;
        let out_dev = host
            .default_output_device()
            .ok_or("no output device found")?;

        let in_cfg = in_dev.default_input_config()?;
        let out_cfg = out_dev.default_output_config()?;

        eprintln!("[jt-engine] input  : {:?}", in_cfg);
        eprintln!("[jt-engine] output : {:?}", out_cfg);

        // UDP socket, connected so `recv` only accepts packets from the server.
        let socket = Arc::new(UdpSocket::bind("0.0.0.0:0")?);
        socket.connect(server)?;

        let stop = Arc::new(AtomicBool::new(false));
        let fifo = SampleFifo::new();
        let rtt_ms = Arc::new(AtomicU64::new(0));

        // Latency model inputs: sample rate and per-stream buffer size.
        let sample_rate = in_cfg.sample_rate().0 as f64;
        let buffer_frames = match in_cfg.buffer_size() {
            cpal::SupportedBufferSize::Range { min, .. } => *min as f64,
            cpal::SupportedBufferSize::Unknown => 256.0, // conservative assumption
        };

        // Input callback -> network thread.
        let (tx, rx) = mpsc::sync_channel::<Vec<Sample>>(CHANNEL_BOUND);

        // --- network sender thread ---
        let send_stop = stop.clone();
        let send_socket = socket.clone();
        let send_handle = thread::spawn(move || {
            let mut seq: u32 = 0;
            let mut last_ping = Instant::now();
            while !send_stop.load(Ordering::Relaxed) {
                match rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(samples) => {
                        let block = AudioBlock::new(seq, samples);
                        seq = seq.wrapping_add(1);
                        let mut buf = Vec::new();
                        block.serialize_into(0, &mut buf);
                        let _ = send_socket.send(&buf);
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => break,
                }
                // Periodically measure RTT with a ping.
                if last_ping.elapsed() >= Duration::from_secs(1) {
                    last_ping = Instant::now();
                    let now_ms = SystemTime::now()
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as u64;
                    let _ = send_socket.send(&jt_core::make_ping(now_ms));
                }
            }
        });

        // --- network receiver thread ---
        let recv_stop = stop.clone();
        let recv_socket = socket.clone();
        let recv_fifo = fifo.clone();
        let recv_rtt = rtt_ms.clone();
        let recv_handle = thread::spawn(move || {
            let mut buf = [0u8; 65536];
            let _ = recv_socket.set_read_timeout(Some(Duration::from_millis(20)));
            while !recv_stop.load(Ordering::Relaxed) {
                match recv_socket.recv(&mut buf) {
                    Ok(n) => {
                        if let Some(ts) = jt_core::parse_ping(&buf[..n]) {
                            let now_ms = SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap()
                                .as_millis() as u64;
                            recv_rtt.store(now_ms.saturating_sub(ts), Ordering::Relaxed);
                        } else if let Some((_, block)) = AudioBlock::deserialize(&buf[..n]) {
                            recv_fifo.push(&block.samples);
                        }
                    }
                    Err(_) => {} // timeout — loop again
                }
            }
        });

        // --- input stream (mic -> channel) ---
        let input_stream = build_input_stream(&in_dev, &in_cfg, tx.clone())?;

        // --- output stream (jitter buffer -> speaker) ---
        let output_stream = build_output_stream(&out_dev, &out_cfg, fifo.clone())?;

        input_stream.play()?;
        output_stream.play()?;

        Ok(Self {
            stop,
            tx: Some(tx),
            _socket: socket,
            _streams: vec![input_stream, output_stream],
            handles: vec![send_handle, recv_handle],
            fifo,
            rtt_ms,
            sample_rate,
            buffer_frames,
        })
    }

    /// Current jitter buffer depth in samples (useful for CLI/UI status).
    pub fn depth(&self) -> usize {
        self.fifo.depth()
    }

    /// Latest measured round-trip time to the server, in milliseconds.
    /// Returns `0` until the first ping response arrives.
    pub fn rtt_ms(&self) -> u64 {
        self.rtt_ms.load(Ordering::Relaxed)
    }

    /// Estimated one-way end-to-end latency in milliseconds:
    /// `rtt/2 (network) + 2×block (capture+playback) + jitter target`.
    /// This is a model estimate, not a hardware measurement; use it to judge
    /// whether you are under the 20 ms rehearsal budget.
    pub fn est_one_way_ms(&self) -> f64 {
        let network = self.rtt_ms.load(Ordering::Relaxed) as f64 / 2.0;
        let block = self.buffer_frames / self.sample_rate * 1000.0;
        let jitter = TARGET_DEPTH as f64 / self.sample_rate * 1000.0;
        network + 2.0 * block + jitter
    }
}

impl Drop for ClientSession {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        // Drop our sender so a fresh disconnect is observable.
        self.tx.take();
        for h in self.handles.drain(..) {
            let _ = h.join();
        }
    }
}

fn err_fn(err: cpal::StreamError) {
    eprintln!("[jt-engine] stream error: {}", err);
}

fn build_input_stream(
    device: &cpal::Device,
    cfg: &cpal::SupportedStreamConfig,
    tx: SyncSender<Vec<Sample>>,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    let channels = cfg.channels() as usize;
    let config = cfg.config();
    let stream = match cfg.sample_format() {
        cpal::SampleFormat::I16 => device.build_input_stream(
            &config,
            move |data: &[i16], _| {
                let _ = tx.try_send(mono_i16_from_i16(data, channels));
            },
            err_fn,
            None,
        )?,
        cpal::SampleFormat::U16 => device.build_input_stream(
            &config,
            move |data: &[u16], _| {
                let _ = tx.try_send(mono_i16_from_u16(data, channels));
            },
            err_fn,
            None,
        )?,
        cpal::SampleFormat::F32 => device.build_input_stream(
            &config,
            move |data: &[f32], _| {
                let _ = tx.try_send(mono_i16_from_f32(data, channels));
            },
            err_fn,
            None,
        )?,
        other => return Err(format!("unsupported input sample format: {:?}", other).into()),
    };
    Ok(stream)
}

fn build_output_stream(
    device: &cpal::Device,
    cfg: &cpal::SupportedStreamConfig,
    fifo: SampleFifo,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    let channels = cfg.channels() as usize;
    let config = cfg.config();
    let stream = match cfg.sample_format() {
        cpal::SampleFormat::I16 => device.build_output_stream(
            &config,
            move |data: &mut [i16], _| write_i16(data, channels, &fifo),
            err_fn,
            None,
        )?,
        cpal::SampleFormat::U16 => device.build_output_stream(
            &config,
            move |data: &mut [u16], _| write_u16(data, channels, &fifo),
            err_fn,
            None,
        )?,
        cpal::SampleFormat::F32 => device.build_output_stream(
            &config,
            move |data: &mut [f32], _| write_f32(data, channels, &fifo),
            err_fn,
            None,
        )?,
        other => return Err(format!("unsupported output sample format: {:?}", other).into()),
    };
    Ok(stream)
}

// --- format conversion helpers: everything collapses to mono i16 ---

fn mono_i16_from_i16(data: &[i16], channels: usize) -> Vec<Sample> {
    if channels == 1 {
        return data.to_vec();
    }
    data.chunks(channels)
        .map(|f| {
            let sum: i32 = f.iter().map(|&s| s as i32).sum();
            (sum / channels as i32) as Sample
        })
        .collect()
}

fn mono_i16_from_u16(data: &[u16], channels: usize) -> Vec<Sample> {
    // cpal U16: silence == 32768.
    data.chunks(channels)
        .map(|f| {
            let sum: i64 = f.iter().map(|&s| s as i64 - 32768).sum();
            (sum / channels as i64) as Sample
        })
        .collect()
}

fn mono_i16_from_f32(data: &[f32], channels: usize) -> Vec<Sample> {
    data.chunks(channels)
        .map(|f| {
            let avg = f.iter().sum::<f32>() / channels as f32;
            (avg.clamp(-1.0, 1.0) * 32767.0) as Sample
        })
        .collect()
}

fn write_i16(data: &mut [i16], channels: usize, fifo: &SampleFifo) {
    for frame in data.chunks_mut(channels) {
        let s = fifo.pop();
        for c in frame.iter_mut() {
            *c = s;
        }
    }
}

fn write_u16(data: &mut [u16], channels: usize, fifo: &SampleFifo) {
    for frame in data.chunks_mut(channels) {
        let v = (fifo.pop() as i32 + 32768) as u16;
        for c in frame.iter_mut() {
            *c = v;
        }
    }
}

fn write_f32(data: &mut [f32], channels: usize, fifo: &SampleFifo) {
    for frame in data.chunks_mut(channels) {
        let v = fifo.pop() as f32 / 32768.0;
        for c in frame.iter_mut() {
            *c = v;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fifo_silence_below_target() {
        let f = SampleFifo::new();
        assert_eq!(f.pop(), 0); // empty → silence
        f.push(&[1, 2, 3, 4, 5, 6, 7, 8]); // 8 < TARGET_DEPTH
        assert_eq!(f.pop(), 0); // still under target → silence
        assert_eq!(f.depth(), 8);
    }

    #[test]
    fn fifo_pops_oldest_after_target() {
        let f = SampleFifo::new();
        let mut v = vec![0i16; TARGET_DEPTH + 5];
        for (i, x) in v.iter_mut().enumerate() {
            *x = (i as i16) + 1;
        }
        f.push(&v);
        assert_eq!(f.depth(), TARGET_DEPTH + 5);
        assert_eq!(f.pop(), 1); // oldest sample
    }

    #[test]
    fn fifo_bounds_depth() {
        let f = SampleFifo::new();
        let v = vec![0i16; MAX_DEPTH + 100];
        f.push(&v);
        assert!(f.depth() <= MAX_DEPTH);
    }
}
