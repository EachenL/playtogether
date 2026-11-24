//! Music 插件 —— 处理 JTMU 同步播放内置 demo 伴奏。
//!
//! 音频热路径会调用 `Hub::synth_music_block` 把伴奏混进每个客户端的输出；
//! 本插件只负责在收到 JTMU 时切换 `Hub::music` 状态。

use std::net::SocketAddr;
use std::time::Instant;

use jt_core::{parse_music_cmd, CMD_START};

use crate::hub::Hub;
use crate::plugin::Plugin;

pub struct MusicPlugin;

impl Plugin for MusicPlugin {
    fn name(&self) -> &'static str {
        "music"
    }

    fn on_message(&mut self, hub: &mut Hub, _src: SocketAddr, buf: &[u8], n: usize) -> bool {
        let Some(cmd) = parse_music_cmd(&buf[..n]) else {
            return false;
        };
        if cmd == CMD_START {
            hub.music.playing = true;
            hub.music.started_at = Some(Instant::now());
        } else {
            hub.music.playing = false;
            hub.music.started_at = None;
        }
        true
    }
}
