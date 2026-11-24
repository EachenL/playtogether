//! Recording 插件 —— 处理 JTRC 开始/停止服务器多轨录音。
//!
//! 录音是**独立的开关**，与音乐播放无关：录音时正常混音不变，
//! 音频热路径只是把每个客户端的原始音频各自存一轨。
//! 停止后存入历史（最多 [`MAX_RECORDINGS`] 条），并广播录制列表。

use std::collections::HashMap;
use std::net::SocketAddr;

use jt_core::{parse_record_cmd, CMD_START};

use crate::hub::{Hub, Recording, MAX_RECORDINGS};
use crate::plugin::Plugin;

pub struct RecordingPlugin;

impl Plugin for RecordingPlugin {
    fn name(&self) -> &'static str {
        "recording"
    }

    fn on_message(&mut self, hub: &mut Hub, _src: SocketAddr, buf: &[u8], n: usize) -> bool {
        let Some(cmd) = parse_record_cmd(&buf[..n]) else {
            return false;
        };
        if cmd == CMD_START {
            if !hub.recording {
                let id = hub.next_recording_id;
                hub.next_recording_id += 1;
                hub.active_recording = Some(Recording {
                    id,
                    duration_samples: 0,
                    tracks: HashMap::new(),
                });
                hub.recording = true;
                println!("recording {id} started");
            }
        } else if hub.recording {
            if let Some(mut rec) = hub.active_recording.take() {
                rec.duration_samples = rec.tracks.values().map(|t| t.len()).max().unwrap_or(0);
                hub.recordings.push(rec);
                while hub.recordings.len() > MAX_RECORDINGS {
                    hub.recordings.remove(0);
                }
            }
            hub.recording = false;
            println!("recording stopped (history {})", hub.recordings.len());
            hub.broadcast_record_list();
        }
        true
    }
}
