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

/// 历史录制最多保留的条数。
pub const MAX_RECORDINGS: usize = 20;

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

/// 回放某条历史录制的状态。
pub struct PlaybackState {
    pub recording_id: u32,
    pub started_at: Instant,
}

/// 一条历史录制（多轨：每个客户端一轨）。
pub struct Recording {
    pub id: u32,
    pub duration_samples: usize,
    pub tracks: HashMap<u32, Vec<Sample>>,
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
    /// 是否正在录音（独立的开关，与音乐播放无关）。
    pub recording: bool,
    /// 正在录制的会话（录音中才有值）。
    pub active_recording: Option<Recording>,
    /// 历史录制（最多 [`MAX_RECORDINGS`] 条）。
    pub recordings: Vec<Recording>,
    pub next_recording_id: u32,
    /// 当前回放（None = 未在回放）。
    pub playback: Option<PlaybackState>,
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

    /// 广播历史录制列表（id + 时长）给所有客户端。
    pub fn broadcast_record_list(&self) {
        let entries: Vec<(u32, u32)> = self
            .recordings
            .iter()
            .map(|r| {
                let dur_ms = (r.duration_samples as f64 / SAMPLE_RATE_HZ as f64 * 1000.0) as u32;
                (r.id, dur_ms)
            })
            .collect();
        let pkt = jt_core::make_record_list(&entries);
        for addr in self.clients.keys() {
            let _ = self.socket.send_to(&pkt, *addr);
        }
    }

    /// 音乐开始后经过的时长。
    fn music_offset(&self) -> Duration {
        self.music
            .started_at
            .map(|t| t.elapsed())
            .unwrap_or_default()
    }

    /// 生成当前音乐块：一个节拍器（120 BPM）+ 一个 A 大调和弦垫。
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
            let beat_pos = (t / beat_period).fract();
            let click = if beat_pos < 0.02 {
                (1.0 - beat_pos / 0.02) * 0.7
            } else {
                0.0
            };
            let pad = 0.06 * (2.0 * PI * 440.0 * t).sin()
                + 0.06 * (2.0 * PI * 554.0 * t).sin()
                + 0.06 * (2.0 * PI * 659.0 * t).sin();
            let v = click + pad;
            out.push((v.clamp(-1.0, 1.0) * 32767.0) as Sample);
        }
        out
    }

    /// 生成某条历史录制的当前回放块（混音所有轨道，按时间对齐）。
    pub fn playback_block(&self, recording_id: u32, len: usize) -> Vec<Sample> {
        let Some(rec) = self.recordings.iter().find(|r| r.id == recording_id) else {
            return vec![0; len];
        };
        let sr = SAMPLE_RATE_HZ as f64;
        let idx0 = (self
            .playback
            .as_ref()
            .map(|p| p.started_at.elapsed().as_secs_f64())
            .unwrap_or_default()
            * sr) as usize;
        let mut mix: Vec<i32> = vec![0; len];
        for track in rec.tracks.values() {
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
