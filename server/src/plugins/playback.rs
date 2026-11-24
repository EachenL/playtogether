//! Playback 插件 —— 处理 JTPB 回放指定历史录制。
//!
//! 任何人可指定某条录制（by id）触发全员同步回放，或停止。

use std::net::SocketAddr;
use std::time::Instant;

use jt_core::{parse_playback_cmd, CMD_START};

use crate::hub::{Hub, PlaybackState};
use crate::plugin::Plugin;

pub struct PlaybackPlugin;

impl Plugin for PlaybackPlugin {
    fn name(&self) -> &'static str {
        "playback"
    }

    fn on_message(&mut self, hub: &mut Hub, _src: SocketAddr, buf: &[u8], n: usize) -> bool {
        let Some((recording_id, cmd)) = parse_playback_cmd(&buf[..n]) else {
            return false;
        };
        if cmd == CMD_START {
            hub.playback = Some(PlaybackState {
                recording_id,
                started_at: Instant::now(),
            });
        } else {
            hub.playback = None;
        }
        true
    }
}
