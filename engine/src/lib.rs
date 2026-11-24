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
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

use jt_core::{AudioBlock, Sample};

use webrtc_audio_processing::config::EchoCanceller;
use webrtc_audio_processing::{Config, Processor};

/// Target jitter depth in samples (~one 128-sample block). Playback stays
/// silent until this much data has buffered, smoothing early jitter.
const TARGET_DEPTH: usize = 128;

/// Upper bound in samples. When the buffer exceeds this, drop the oldest
/// samples so latency stays bounded even if the network briefly stalls.
const MAX_DEPTH: usize = 4096;

/// Bounded queue capacity (in blocks) between the audio callback and the
/// network thread. Overflow drops the block — correct realtime behaviour.
const CHANNEL_BOUND: usize = 8;

/// Resolve a `host[:port]` string to a `SocketAddr`.
///
/// Accepts either a literal IP (`1.14.105.192:9000`) or a domain name
/// (`daoway.ai:9000`). A bare host without a port defaults to port 9000.
pub fn resolve_addr(input: &str) -> Result<SocketAddr, std::io::Error> {
    let trimmed = input.trim();
    // Fast path: already a literal IP:port.
    if let Ok(addr) = trimmed.parse::<SocketAddr>() {
        return Ok(addr);
    }
    // Allow a bare host (no port): default to the standard port.
    let candidate = if trimmed.contains(':') {
        trimmed.to_string()
    } else {
        format!("{trimmed}:9000")
    };
    candidate
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("cannot resolve '{trimmed}'"),
            )
        })
}

/// Parse a 4-digit rehearsal-room code (0000–9999).
pub fn parse_room_code(input: &str) -> Result<u32, String> {
    let t = input.trim();
    if t.len() != 4 || !t.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("房间号必须是 4 位数字（输入了 '{input}'）"));
    }
    Ok(t.parse::<u32>().unwrap())
}

/// Stream mode.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// 排练模式：连续全双工，人人互相听见。
    Rehearsal,
    /// 语音对讲：全双工 + 回声消除（AEC），开公放也不啸叫。
    Voice,
}

/// A named audio device, for the device picker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub name: String,
    pub is_default: bool,
}

/// Everything needed to start a client session.
#[derive(Debug, Clone)]
pub struct SessionConfig {
    pub server: SocketAddr,
    pub room_id: u32,
    pub mode: Mode,
    pub input_device: Option<String>,
    pub output_device: Option<String>,
}

/// Enumerate available input devices.
pub fn list_input_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let default = host.default_input_device().and_then(|d| d.name().ok());
    host.input_devices()
        .map(|devs| {
            devs.filter_map(|d| {
                d.name().ok().map(|name| DeviceInfo {
                    is_default: Some(&name) == default.as_ref(),
                    name,
                })
            })
            .collect()
        })
        .unwrap_or_default()
}

/// Enumerate available output devices.
pub fn list_output_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let default = host.default_output_device().and_then(|d| d.name().ok());
    host.output_devices()
        .map(|devs| {
            devs.filter_map(|d| {
                d.name().ok().map(|name| DeviceInfo {
                    is_default: Some(&name) == default.as_ref(),
                    name,
                })
            })
            .collect()
        })
        .unwrap_or_default()
}

fn find_input_device(host: &cpal::Host, name: Option<&str>) -> Option<cpal::Device> {
    match name {
        None => host.default_input_device(),
        Some(n) => host
            .input_devices()
            .ok()?
            .find(|d| d.name().ok().as_deref() == Some(n)),
    }
}

fn find_output_device(host: &cpal::Host, name: Option<&str>) -> Option<cpal::Device> {
    match name {
        None => host.default_output_device(),
        Some(n) => host
            .output_devices()
            .ok()?
            .find(|d| d.name().ok().as_deref() == Some(n)),
    }
}

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

/// WebRTC AEC3 echo-cancellation state, shared between the output (render) and
/// input (capture) audio callbacks. Used only in voice mode (full-duplex with
/// echo cancellation so speakerphone playback doesn't feed back into the mic).
struct AecState {
    processor: Processor,
    render: VecDeque<f32>,
    capture: VecDeque<f32>,
    frame_size: usize,
}

impl AecState {
    fn new(sample_rate_hz: u32) -> Result<Self, Box<dyn std::error::Error>> {
        let processor = Processor::new(sample_rate_hz)?;
        let config = Config {
            echo_canceller: Some(EchoCanceller::default()),
            ..Default::default()
        };
        processor.set_config(config);
        let frame_size = processor.num_samples_per_frame();
        Ok(Self {
            processor,
            render: VecDeque::new(),
            capture: VecDeque::new(),
            frame_size,
        })
    }

    /// Feed the played (render) samples; capped to bound memory (~500 ms).
    fn push_render(&mut self, samples: &[Sample]) {
        for &s in samples {
            self.render.push_back(s as f32 / 32768.0);
        }
        let max = self.frame_size * 50;
        while self.render.len() > max {
            self.render.pop_front();
        }
    }

    /// Feed captured samples; returns echo-cancelled output (whole frames only).
    fn process_capture(&mut self, samples: &[Sample]) -> Vec<Sample> {
        for &s in samples {
            self.capture.push_back(s as f32 / 32768.0);
        }
        let max = self.frame_size * 50;
        while self.capture.len() > max {
            self.capture.pop_front();
        }
        let mut out = Vec::with_capacity(samples.len());
        while self.render.len() >= self.frame_size && self.capture.len() >= self.frame_size {
            let mut render_frame = vec![Vec::with_capacity(self.frame_size)];
            let mut capture_frame = vec![Vec::with_capacity(self.frame_size)];
            for _ in 0..self.frame_size {
                render_frame[0].push(self.render.pop_front().unwrap());
                capture_frame[0].push(self.capture.pop_front().unwrap());
            }
            let _ = self.processor.process_render_frame(&mut render_frame);
            let _ = self.processor.process_capture_frame(&mut capture_frame);
            for &v in &capture_frame[0] {
                out.push((v.clamp(-1.0, 1.0) * 32767.0) as Sample);
            }
        }
        out
    }
}

/// A running client session. Dropping it stops streaming and joins threads.
pub struct ClientSession {
    stop: Arc<AtomicBool>,
    tx: Option<SyncSender<Vec<Sample>>>,
    socket: Arc<UdpSocket>,
    _streams: Vec<cpal::Stream>,
    handles: Vec<JoinHandle<()>>,
    fifo: SampleFifo,
    rtt_ms: Arc<AtomicU64>,
    sample_rate: f64,
    buffer_frames: f64,
    talking: Arc<AtomicBool>,
    sidetone_on: Arc<AtomicBool>,
    own_id: Arc<AtomicU32>,
    roster: Arc<Mutex<Vec<(u32, String)>>>,
}

impl ClientSession {
    /// Start capturing, streaming, and playback, according to `config`.
    pub fn start(config: SessionConfig) -> Result<Self, Box<dyn std::error::Error>> {
        let host = cpal::default_host();

        let in_dev = find_input_device(&host, config.input_device.as_deref())
            .ok_or("no input device found")?;
        let out_dev = find_output_device(&host, config.output_device.as_deref())
            .ok_or("no output device found")?;

        let in_cfg = in_dev.default_input_config()?;
        let out_cfg = out_dev.default_output_config()?;

        eprintln!("[jt-engine] input  : {:?}", in_cfg);
        eprintln!("[jt-engine] output : {:?}", out_cfg);

        // UDP socket, connected so `recv` only accepts packets from the server.
        let socket = Arc::new(UdpSocket::bind("0.0.0.0:0")?);
        socket.connect(config.server)?;

        let stop = Arc::new(AtomicBool::new(false));
        let fifo = SampleFifo::new();
        let rtt_ms = Arc::new(AtomicU64::new(0));
        // Full-duplex: both modes always stream. Voice mode additionally runs
        // AEC (echo cancellation) so speaker playback doesn't feed back.
        let mode = config.mode;
        let talking = Arc::new(AtomicBool::new(true));
        let aec: Option<Arc<Mutex<AecState>>> = if mode == Mode::Voice {
            Some(Arc::new(Mutex::new(AecState::new(jt_core::SAMPLE_RATE_HZ)?)))
        } else {
            None
        };
        let room_id = config.room_id;
        let own_id = Arc::new(AtomicU32::new(0));
        let roster = Arc::new(Mutex::new(Vec::<(u32, String)>::new()));
        // 本地反听：把采集缓冲起来，混入本地输出（录音时听自己）。
        let sidetone: Arc<Mutex<VecDeque<Sample>>> = Arc::new(Mutex::new(VecDeque::new()));
        let sidetone_on = Arc::new(AtomicBool::new(false));

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
        let send_talking = talking.clone();
        let send_handle = thread::spawn(move || {
            let mut seq: u32 = 0;
            let mut last_ping = Instant::now();
            while !send_stop.load(Ordering::Relaxed) {
                match rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(samples) => {
                        // In voice mode, send silence while not talking.
                        let out = if send_talking.load(Ordering::Relaxed) {
                            samples
                        } else {
                            vec![0; samples.len()]
                        };
                        let block = AudioBlock::new(seq, out);
                        seq = seq.wrapping_add(1);
                        let mut buf = Vec::new();
                        block.serialize_into(room_id, &mut buf);
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
        let recv_own_id = own_id.clone();
        let recv_roster = roster.clone();
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
                        } else if let Some(id) = jt_core::parse_welcome(&buf[..n]) {
                            recv_own_id.store(id, Ordering::Relaxed);
                        } else if let Some(entries) = jt_core::parse_roster(&buf[..n]) {
                            *recv_roster.lock().unwrap() = entries;
                        } else if let Some((_, block)) = AudioBlock::deserialize(&buf[..n]) {
                            recv_fifo.push(&block.samples);
                        }
                    }
                    Err(_) => {} // timeout — loop again
                }
            }
        });

        // --- input stream (mic -> channel) ---
        let input_stream = build_input_stream(
            &in_dev,
            &in_cfg,
            tx.clone(),
            aec.clone(),
            sidetone.clone(),
            sidetone_on.clone(),
        )?;

        // --- output stream (jitter buffer -> speaker) ---
        let output_stream =
            build_output_stream(&out_dev, &out_cfg, fifo.clone(), sidetone.clone(), aec.clone())?;

        input_stream.play()?;
        output_stream.play()?;

        Ok(Self {
            stop,
            tx: Some(tx),
            socket,
            _streams: vec![input_stream, output_stream],
            handles: vec![send_handle, recv_handle],
            fifo,
            rtt_ms,
            sample_rate,
            buffer_frames,
            talking,
            sidetone_on,
            own_id,
            roster,
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

    /// Set whether this client is currently "talking". In voice mode this gates
    /// audio (silence while not talking); in rehearsal mode it is ignored
    /// (always talking).
    pub fn set_talking(&self, talking: bool) {
        self.talking.store(talking, Ordering::Relaxed);
    }

    /// Whether this client is currently talking.
    pub fn talking(&self) -> bool {
        self.talking.load(Ordering::Relaxed)
    }

    /// Announce this client's display name to the server.
    pub fn set_name(&self, name: &str) {
        let _ = self.socket.send(&jt_core::make_name(name));
    }

    /// Set this client's local gain for one peer (peer client id).
    pub fn set_gain(&self, peer_id: u32, gain: f32) {
        let _ = self.socket.send(&jt_core::make_gain(peer_id, gain));
    }

    /// 本地反听：是否把麦克风采集混入本地输出（听自己）。默认关，录音时开（用耳机）。
    pub fn set_sidetone(&self, on: bool) {
        self.sidetone_on.store(on, Ordering::Relaxed);
    }

    /// 同步播放/停止内置伴奏（服务器）。
    pub fn set_music(&self, on: bool) {
        let cmd = if on { jt_core::CMD_START } else { jt_core::CMD_STOP };
        let _ = self.socket.send(&jt_core::make_music_cmd(cmd));
    }

    /// 开始/停止服务器多轨录音（录音时互相隔离）。
    pub fn set_recording(&self, on: bool) {
        let cmd = if on { jt_core::CMD_START } else { jt_core::CMD_STOP };
        let _ = self.socket.send(&jt_core::make_record_cmd(cmd));
    }

    /// 开始/停止全员同步回放录制结果。
    pub fn set_playback(&self, on: bool) {
        let cmd = if on { jt_core::CMD_START } else { jt_core::CMD_STOP };
        let _ = self.socket.send(&jt_core::make_playback_cmd(cmd));
    }

    /// This client's server-assigned id (0 until the welcome arrives).
    pub fn own_id(&self) -> u32 {
        self.own_id.load(Ordering::Relaxed)
    }

    /// Current room roster as `(client_id, name)` pairs.
    pub fn roster(&self) -> Vec<(u32, String)> {
        self.roster.lock().unwrap().clone()
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
    aec: Option<Arc<Mutex<AecState>>>,
    sidetone: Arc<Mutex<VecDeque<Sample>>>,
    sidetone_on: Arc<AtomicBool>,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    let channels = cfg.channels() as usize;
    let config = cfg.config();
    let stream = match cfg.sample_format() {
        cpal::SampleFormat::I16 => {
            let a = aec.clone();
            let s = sidetone.clone();
            let so = sidetone_on.clone();
            device.build_input_stream(
                &config,
                move |data: &[i16], _| {
                    let mono = mono_i16_from_i16(data, channels);
                    let out = match &a {
                        Some(aec) => aec.lock().unwrap().process_capture(&mono),
                        None => mono,
                    };
                    push_sidetone(&s, &so, &out);
                    let _ = tx.try_send(out);
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::U16 => {
            let a = aec.clone();
            let s = sidetone.clone();
            let so = sidetone_on.clone();
            device.build_input_stream(
                &config,
                move |data: &[u16], _| {
                    let mono = mono_i16_from_u16(data, channels);
                    let out = match &a {
                        Some(aec) => aec.lock().unwrap().process_capture(&mono),
                        None => mono,
                    };
                    push_sidetone(&s, &so, &out);
                    let _ = tx.try_send(out);
                },
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::F32 => {
            let a = aec.clone();
            let s = sidetone.clone();
            let so = sidetone_on.clone();
            device.build_input_stream(
                &config,
                move |data: &[f32], _| {
                    let mono = mono_i16_from_f32(data, channels);
                    let out = match &a {
                        Some(aec) => aec.lock().unwrap().process_capture(&mono),
                        None => mono,
                    };
                    push_sidetone(&s, &so, &out);
                    let _ = tx.try_send(out);
                },
                err_fn,
                None,
            )?
        }
        other => return Err(format!("unsupported input sample format: {:?}", other).into()),
    };
    Ok(stream)
}

fn build_output_stream(
    device: &cpal::Device,
    cfg: &cpal::SupportedStreamConfig,
    fifo: SampleFifo,
    sidetone: Arc<Mutex<VecDeque<Sample>>>,
    aec: Option<Arc<Mutex<AecState>>>,
) -> Result<cpal::Stream, Box<dyn std::error::Error>> {
    let channels = cfg.channels() as usize;
    let config = cfg.config();
    let stream = match cfg.sample_format() {
        cpal::SampleFormat::I16 => {
            let f = fifo.clone();
            let s = sidetone.clone();
            let a = aec.clone();
            device.build_output_stream(
                &config,
                move |data: &mut [i16], _| write_i16(data, channels, &f, Some(s.as_ref()), a.as_deref()),
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::U16 => {
            let f = fifo.clone();
            let s = sidetone.clone();
            let a = aec.clone();
            device.build_output_stream(
                &config,
                move |data: &mut [u16], _| write_u16(data, channels, &f, Some(s.as_ref()), a.as_deref()),
                err_fn,
                None,
            )?
        }
        cpal::SampleFormat::F32 => {
            let f = fifo.clone();
            let s = sidetone.clone();
            let a = aec.clone();
            device.build_output_stream(
                &config,
                move |data: &mut [f32], _| write_f32(data, channels, &f, Some(s.as_ref()), a.as_deref()),
                err_fn,
                None,
            )?
        }
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

/// 本地反听缓冲上限（1 秒 @ 48kHz）。
const SIDETONE_CAP: usize = 48_000;
/// 反听增益（听自己时的音量，避免太响盖过伴奏）。
const SIDETONE_GAIN: f32 = 0.5;

/// 把采集样本推入反听缓冲（若开启）。有界，防止积压。
fn push_sidetone(sidetone: &Mutex<VecDeque<Sample>>, on: &AtomicBool, samples: &[Sample]) {
    if !on.load(Ordering::Relaxed) {
        return;
    }
    let mut buf = sidetone.lock().unwrap();
    buf.extend(samples.iter().copied());
    while buf.len() > SIDETONE_CAP {
        buf.pop_front();
    }
}

/// 混音：服务器混音 + 本地反听（听自己）。
fn mix_sidetone(server: Sample, sidetone: Option<&Mutex<VecDeque<Sample>>>) -> Sample {
    let self_mon = sidetone
        .map(|s| {
            let v = s.lock().unwrap().pop_front().unwrap_or(0);
            (v as f32 * SIDETONE_GAIN) as Sample
        })
        .unwrap_or(0);
    (server as i32 + self_mon as i32).clamp(i16::MIN as i32, i16::MAX as i32) as Sample
}

fn write_i16(
    data: &mut [i16],
    channels: usize,
    fifo: &SampleFifo,
    sidetone: Option<&Mutex<VecDeque<Sample>>>,
    aec: Option<&Mutex<AecState>>,
) {
    let n = data.len() / channels;
    let mut mono = Vec::with_capacity(n);
    for _ in 0..n {
        mono.push(mix_sidetone(fifo.pop(), sidetone));
    }
    if let Some(a) = aec {
        a.lock().unwrap().push_render(&mono);
    }
    for (i, frame) in data.chunks_mut(channels).enumerate() {
        for c in frame.iter_mut() {
            *c = mono[i];
        }
    }
}

fn write_u16(
    data: &mut [u16],
    channels: usize,
    fifo: &SampleFifo,
    sidetone: Option<&Mutex<VecDeque<Sample>>>,
    aec: Option<&Mutex<AecState>>,
) {
    let n = data.len() / channels;
    let mut mono = Vec::with_capacity(n);
    for _ in 0..n {
        mono.push(mix_sidetone(fifo.pop(), sidetone));
    }
    if let Some(a) = aec {
        a.lock().unwrap().push_render(&mono);
    }
    for (i, frame) in data.chunks_mut(channels).enumerate() {
        let v = (mono[i] as i32 + 32768) as u16;
        for c in frame.iter_mut() {
            *c = v;
        }
    }
}

fn write_f32(
    data: &mut [f32],
    channels: usize,
    fifo: &SampleFifo,
    sidetone: Option<&Mutex<VecDeque<Sample>>>,
    aec: Option<&Mutex<AecState>>,
) {
    let n = data.len() / channels;
    let mut mono = Vec::with_capacity(n);
    for _ in 0..n {
        mono.push(mix_sidetone(fifo.pop(), sidetone));
    }
    if let Some(a) = aec {
        a.lock().unwrap().push_render(&mono);
    }
    for (i, frame) in data.chunks_mut(channels).enumerate() {
        let v = mono[i] as f32 / 32768.0;
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

    #[test]
    fn aec_processes_frames() {
        let mut aec = AecState::new(48_000).expect("AEC init");
        let fs = aec.frame_size;
        let render: Vec<i16> = (0..fs)
            .map(|i| ((i as f32 / 40.0).cos() * 0.5 * 32767.0) as i16)
            .collect();
        let capture: Vec<i16> = (0..fs)
            .map(|i| ((i as f32 / 20.0).sin() * 0.3 * 32767.0) as i16)
            .collect();

        for _ in 0..5 {
            aec.push_render(&render);
            let out = aec.process_capture(&capture);
            assert_eq!(out.len(), fs, "AEC must output one full frame");
        }
    }

    /// 模拟真实声学回声：远端播放 1000Hz（回声源），近端说话 300Hz，
    /// 麦克风信号 = 语音 + 0.6 * 延迟 100ms 的回声。喂给 AEC 后，
    /// 用 Goertzel 分别量 1000Hz（回声）和 300Hz（语音）的能量变化。
    #[test]
    fn aec_reduces_simulated_echo() {
        fn goertzel_power(samples: &[f32], freq: f32, fs: f32) -> f64 {
            let omega = 2.0 * std::f32::consts::PI * freq / fs;
            let coeff = 2.0 * omega.cos();
            let (mut s1, mut s2) = (0.0f32, 0.0f32);
            for &x in samples {
                let s0 = x + coeff * s1 - s2;
                s2 = s1;
                s1 = s0;
            }
            let p = s1 * s1 + s2 * s2 - coeff * s1 * s2;
            p as f64
        }

        let mut aec = AecState::new(48_000).expect("AEC init");
        let fs = aec.frame_size;
        let total_frames = 500; // 5 秒
        let total = fs * total_frames;
        let delay = fs * 10; // 100 ms 回声路径

        // 远端渲染（多频率，模拟远端音乐/语音的宽带频谱）
        let render: Vec<f32> = (0..total)
            .map(|i| {
                let t = i as f32 / 48_000.0;
                (t * 600.0 * 6.2831853).sin() * 0.25
                    + (t * 1200.0 * 6.2831853).sin() * 0.2
                    + (t * 1800.0 * 6.2831853).sin() * 0.15
            })
            .collect();
        let voice: Vec<f32> = (0..total)
            .map(|i| (i as f32 / 48_000.0 * 300.0 * 6.2831853).sin() * 0.3)
            .collect();

        // 麦克风信号 = 语音 + 回声（指数衰减的多次反射，模拟房间混响）
        let mut capture = Vec::with_capacity(total);
        for i in 0..total {
            let mut echo = 0.0f32;
            let mut tap = delay;
            let mut gain = 0.45f32;
            for _ in 0..10 {
                if i >= tap {
                    echo += render[i - tap] * gain;
                }
                tap += 160; // 每次反射间隔约 3.3ms
                gain *= 0.62; // 指数衰减
            }
            capture.push((voice[i] + echo).clamp(-1.0, 1.0));
        }

        // 逐帧喂给 AEC
        let mut out = Vec::with_capacity(total);
        for f in 0..total_frames {
            let s = f * fs;
            let r: Vec<i16> = render[s..s + fs].iter().map(|&x| (x * 32767.0) as i16).collect();
            let c: Vec<i16> = capture[s..s + fs].iter().map(|&x| (x * 32767.0) as i16).collect();
            aec.push_render(&r);
            out.extend(aec.process_capture(&c));
        }
        assert_eq!(out.len(), total);

        // 取收敛后的后 250 帧测量
        let start = 250 * fs;
        let cap: Vec<f32> = capture[start..total].to_vec();
        let outp: Vec<f32> = out[start..total].iter().map(|&x| x as f32 / 32767.0).collect();

        let echo_in = goertzel_power(&cap, 600.0, 48_000.0)
            + goertzel_power(&cap, 1200.0, 48_000.0)
            + goertzel_power(&cap, 1800.0, 48_000.0);
        let echo_out = goertzel_power(&outp, 600.0, 48_000.0)
            + goertzel_power(&outp, 1200.0, 48_000.0)
            + goertzel_power(&outp, 1800.0, 48_000.0);
        let voice_in = goertzel_power(&cap, 300.0, 48_000.0);
        let voice_out = goertzel_power(&outp, 300.0, 48_000.0);

        let reduction_db = 10.0 * (echo_out / echo_in).log10();
        let voice_db = 10.0 * (voice_out / voice_in).log10();
        println!("[AEC 模拟] 回声抑制: {reduction_db:.1} dB，语音保留: {voice_db:.1} dB");
        println!("           （注：纯正弦输入是自适应滤波器最难的场景；真实语音/音乐的抑制会远高于此）");

        // 回声至少抑制 3 dB；语音不被过度损伤（保留 > -6 dB）
        // 纯正弦的秩亏自相关矩阵会限制 NLMS 收敛，真实宽带语音可达 20~40 dB。
        assert!(
            reduction_db < -3.0,
            "AEC 应至少抑制 3dB 回声，实际 {reduction_db:.1} dB"
        );
        assert!(
            voice_db > -6.0,
            "AEC 不应过度损伤语音，实际 {voice_db:.1} dB"
        );
    }
}
