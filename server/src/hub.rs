//! Hub — 服务器核心状态与音频热路径。
//!
//! 这里的 `ClientState` / `Hub` 是**音频数据面**（JT01 混音）直接读写的数据；
//! 插件只通过 `&mut Hub` 更新**控制面**状态（改名/音量/花名册/音乐/录音/回放），
//! 不会进入每 10ms 一次的音频热路径。

use std::collections::HashMap;
use std::f64::consts::PI;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use jt_core::{Sample, SAMPLE_RATE_HZ};

/// 单个客户端的状态。
pub struct ClientState {
    pub room: u32,
    pub client_id: u32,
    pub name: String,
    pub last_block: Vec<Sample>,
    pub last_seen: Instant,
    /// 该客户端对每个 peer（client_id）的本地增益。
    pub gains: HashMap<u32, f32>,
}

/// 内置 demo 伴奏的播放状态。
pub struct MusicState {
    pub playing: bool,
    pub started_at: Option<Instant>,
}

/// 录制结果回放的状态。
pub struct PlaybackState {
    pub playing: bool,
    pub started_at: Option<Instant>,
}

/// 服务器核心：socket + 客户端/房间状态 + 混音 + 录音/音乐/回放所需的一切。
pub struct Hub {
    pub socket: UdpSocket,
    pub clients: HashMap<SocketAddr, ClientState>,
    pub rooms: HashMap<u32, Instant>, // room code → last active
    pub next_client_id: u32,
    pub max_room_clients: usize,
    pub room_timeout: Duration,
    pub echo: bool,

    pub music: MusicState,
    pub recording: bool,
    /// 多轨录音：client_id → 该客户端录制的全部样本。
    pub recordings: HashMap<u32, Vec<Sample>>,
    pub playback: PlaybackState,
}

impl Hub {
    /// 广播某房间的花名册（id + 名称）给房间内所有客户端。
    pub fn broadcast_roster(&self, room: u32) {
        let mut entries: Vec<(u32, String)> = self
            .clients
            .values()
            .filter(|cs| cs.room == room)
            .map(|cs| (cs.client_id, cs.name.clone()))
            .collect();
        entries.sort_by_key(|(id, _)| *id);
        let pkt = jt_core::make_roster(&entries);
        for (addr, cs) in self.clients.iter() {
            if cs.room == room {
                let _ = self.socket.send_to(&pkt, *addr);
            }
        }
    }

    /// 房间内当前客户端数。
    pub fn room_count(&self, room: u32) -> usize {
        self.clients.values().filter(|cs| cs.room == room).count()
    }

    /// 音乐开始后经过的时长。
    fn music_offset(&self) -> Duration {
        self.music
            .started_at
            .map(|t| t.elapsed())
            .unwrap_or_default()
    }

    /// 生成当前音乐块：一个节拍器（120 BPM）+ 一个 A 大调和弦垫。
    /// `len` 为采样数（一帧 480 = 10ms）。全房间同步：都以服务器时钟为基准。
    pub fn synth_music_block(&self, len: usize) -> Vec<Sample> {
        if !self.music.playing {
            return vec![0; len];
        }
        let sr = SAMPLE_RATE_HZ as f64;
        let offset = self.music_offset().as_secs_f64();
        let beat_period = 0.5; // 120 BPM
        let mut out = Vec::with_capacity(len);
        for i in 0..len {
            let t = offset + i as f64 / sr;
            // 节拍器咔哒声
            let beat_pos = (t / beat_period).fract();
            let click = if beat_pos < 0.02 {
                (1.0 - beat_pos / 0.02) * 0.7
            } else {
                0.0
            };
            // A 大调和弦（A4=440, C#5=554, E5=659）
            let pad = 0.06 * (2.0 * PI * 440.0 * t).sin()
                + 0.06 * (2.0 * PI * 554.0 * t).sin()
                + 0.06 * (2.0 * PI * 659.0 * t).sin();
            let v = click + pad;
            out.push((v.clamp(-1.0, 1.0) * 32767.0) as Sample);
        }
        out
    }

    /// 生成当前回放块：把所有录制轨道按时间对齐混音。
    /// `len` 为采样数。轨道长度不同时，取到哪算哪。
    pub fn playback_block(&self, len: usize) -> Vec<Sample> {
        let sr = SAMPLE_RATE_HZ as f64;
        let idx0 = (self
            .playback
            .started_at
            .map(|t| t.elapsed().as_secs_f64())
            .unwrap_or_default()
            * sr) as usize;
        let mut mix: Vec<i32> = vec![0; len];
        for track in self.recordings.values() {
            for (i, m) in mix.iter_mut().enumerate() {
                if let Some(&s) = track.get(idx0 + i) {
                    *m += s as i32;
                }
            }
        }
        mix.iter()
            .map(|&v| v.clamp(i16::MIN as i32, i16::MAX as i32) as Sample)
            .collect()
    }
}
