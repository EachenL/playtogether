//! Playback 插件 —— 处理 JTPB 全员同步回放录制结果。
//!
//! 音频热路径会调用 `Hub::playback_block` 把多轨录音按时间对齐混音，
//! 本插件只负责在收到 JTPB 时切换 `Hub::playback` 状态。
//! 任何人都能触发，全房间同步回放。

use std::net::SocketAddr;
use std::time::Instant;

use jt_core::{parse_playback_cmd, CMD_START};

use crate::hub::Hub;
use crate::plugin::Plugin;

pub struct PlaybackPlugin;

impl Plugin for PlaybackPlugin {
    fn name(&self) -> &'static str {
        "playback"
    }

    fn on_message(&mut self, hub: &mut Hub, _src: SocketAddr, buf: &[u8], n: usize) -> bool {
        let Some(cmd) = parse_playback_cmd(&buf[..n]) else {
            return false;
        };
        if cmd == CMD_START {
            hub.playback.playing = true;
            hub.playback.started_at = Some(Instant::now());
        } else {
            hub.playback.playing = false;
            hub.playback.started_at = None;
        }
        true
    }
}
