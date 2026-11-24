//! jt-server — a JackTrip-style mixing hub with rehearsal-room isolation,
//! named participants, and per-peer server-side gain mixing.
//!
//! 架构：核心循环（本文件）负责**音频热路径**（JT01 混音）+ ping 回显 +
//! 超时清理；其余"功能"（改名/花名册、独立音量、房间策略）都是**插件**，
//! 通过 `plugins::all()` 编译期组合，见 `plugin.rs` 与 `plugins/`。
//!
//! Usage:
//!   jt-server [--port N] [--echo] [--room-timeout SECS] [--max-room-clients N]

mod hub;
mod plugin;
mod plugins;

use std::collections::HashMap;
use std::net::UdpSocket;
use std::time::{Duration, Instant};

use jt_core::{parse_ping, AudioBlock, Sample};

use crate::hub::{ClientState, Hub, MusicState};

/// A client is dropped if no packet arrives for this long.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut port: u16 = 9000;
    let mut echo = false;
    let mut room_timeout_secs: u64 = 1200; // 20 minutes
    let mut max_room_clients: usize = 20;

    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--port" => {
                if let Some(p) = args.get(i + 1) {
                    port = p.parse()?;
                    i += 1;
                }
            }
            "--echo" => echo = true,
            "--room-timeout" => {
                if let Some(v) = args.get(i + 1) {
                    room_timeout_secs = v.parse()?;
                    i += 1;
                }
            }
            "--max-room-clients" => {
                if let Some(v) = args.get(i + 1) {
                    max_room_clients = v.parse()?;
                    i += 1;
                }
            }
            other => eprintln!("ignoring unknown arg: {other}"),
        }
        i += 1;
    }

    let socket = UdpSocket::bind(("0.0.0.0", port))?;
    println!(
        "jt-server listening on 0.0.0.0:{}{} (room idle timeout {}s, max {}/room)",
        port,
        if echo { " (echo mode)" } else { "" },
        room_timeout_secs,
        max_room_clients
    );

    let mut hub = Hub {
        socket,
        clients: HashMap::new(),
        rooms: HashMap::new(),
        next_client_id: 1,
        max_room_clients,
        room_timeout: Duration::from_secs(room_timeout_secs),
        echo,
        music: MusicState {
            playing: false,
            started_at: None,
        },
        recording: false,
        active_recording: None,
        recordings: Vec::new(),
        next_recording_id: 1,
        playback: None,
    };
    let mut plugins = plugins::all();
    let names: Vec<&str> = plugins.iter().map(|p| p.name()).collect();
    println!("plugins loaded: {}", names.join(", "));

    let mut buf = [0u8; 65536];
    let mut last_tick = Instant::now();

    loop {
        let (n, src) = match hub.socket.recv_from(&mut buf) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("recv error: {e}");
                continue;
            }
        };

        // 1. ping 回显（核心，供客户端测 RTT）。
        if parse_ping(&buf[..n]).is_some() {
            let _ = hub.socket.send_to(&buf[..n], src);
            continue;
        }

        // 2. 控制消息 → 插件链（改名/音量等）。
        let mut handled = false;
        for p in plugins.iter_mut() {
            if p.on_message(&mut hub, src, &buf, n) {
                handled = true;
                break;
            }
        }
        if handled {
            continue;
        }

        // 3. 音频热路径（JT01）—— 不经插件，保持 <20ms 延迟。
        let Some((room, block)) = AudioBlock::deserialize(&buf[..n]) else {
            continue; // not our packet
        };

        // 新客户端注册：先让插件决定是否允许加入。
        if !hub.clients.contains_key(&src) {
            let mut reject = None;
            for p in plugins.iter_mut() {
                if let Some(pkt) = p.on_join(&mut hub, room) {
                    reject = Some(pkt);
                    break;
                }
            }
            if let Some(pkt) = reject {
                let _ = hub.socket.send_to(&pkt, src);
                continue;
            }

            let cid = hub.next_client_id;
            hub.next_client_id += 1;
            hub.clients.insert(
                src,
                ClientState {
                    room,
                    client_id: cid,
                    name: format!("乐手{cid}"),
                    last_block: Vec::new(),
                    last_seen: Instant::now(),
                    gains: HashMap::new(),
                },
            );
            let _ = hub.socket.send_to(&jt_core::make_welcome(cid), src);
            hub.broadcast_roster(room);
        }

        // 刷新该客户端的音频块 + 时间戳（保留原始音频用于多轨录音）。
        let samples = block.samples;
        {
            let cs = hub.clients.get_mut(&src).unwrap();
            cs.last_block = samples.clone();
            cs.last_seen = Instant::now();
        }
        let room = hub.clients.get(&src).unwrap().room;

        // 录音中：把该客户端的原始音频追加到当前录制的轨道（不改变混音）。
        if hub.recording {
            let cid = hub.clients.get(&src).unwrap().client_id;
            if let Some(rec) = hub.active_recording.as_mut() {
                rec.tracks.entry(cid).or_default().extend_from_slice(&samples);
            }
        }

        let len = samples.len();

        // 输出混音：回放 > 正常（其他人 + 音乐）。录音不影响混音。
        let playback_id = hub.playback.as_ref().map(|p| p.recording_id);
        let output: Option<Vec<Sample>> = if let Some(id) = playback_id {
            Some(hub.playback_block(id, len))
        } else {
            let gains = hub.clients.get(&src).unwrap().gains.clone();
            let mut mix: Vec<i32> = Vec::new();
            for (addr, cs) in hub.clients.iter() {
                if cs.room != room || (!hub.echo && *addr == src) {
                    continue;
                }
                let gain = gains.get(&cs.client_id).copied().unwrap_or(1.0);
                for (j, &s) in cs.last_block.iter().enumerate() {
                    if j >= mix.len() {
                        mix.push(0);
                    }
                    mix[j] += ((s as f32) * gain) as i32;
                }
            }
            // 音乐混入。
            if hub.music.playing {
                let music = hub.synth_music_block(len);
                for (j, &s) in music.iter().enumerate() {
                    if j >= mix.len() {
                        mix.push(0);
                    }
                    mix[j] += s as i32;
                }
            }
            if mix.is_empty() {
                None // 房间里只有自己且无音乐 → 不发
            } else {
                Some(
                    mix.iter()
                        .map(|&v| v.clamp(i16::MIN as i32, i16::MAX as i32) as Sample)
                        .collect(),
                )
            }
        };

        if let Some(out_samples) = output {
            let resp = AudioBlock::new(0, out_samples);
            let mut out = Vec::new();
            resp.serialize_into(room, &mut out);
            let _ = hub.socket.send_to(&out, src);
        }

        // 4. 每秒 housekeeping：核心剔除超时客户端 + 插件 tick。
        if last_tick.elapsed() >= Duration::from_secs(1) {
            last_tick = Instant::now();

            // 核心：剔除超时客户端。
            let before = hub.clients.len();
            hub.clients.retain(|_, cs| cs.last_seen.elapsed() < CLIENT_TIMEOUT);

            // 插件 tick（如 RoomPlugin 刷新/释放空闲房间）。
            for p in plugins.iter_mut() {
                p.on_tick(&mut hub);
            }

            // 成员变化 → 推送更新后的花名册。
            if hub.clients.len() != before {
                let room_ids: Vec<u32> = hub.rooms.keys().copied().collect();
                for r in room_ids {
                    hub.broadcast_roster(r);
                }
            }

            println!("clients: {} | rooms: {}", hub.clients.len(), hub.rooms.len());
        }
    }
}
