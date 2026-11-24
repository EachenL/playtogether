//! Recording 插件 —— 处理 JTRC 开始/停止服务器多轨录音。
//!
//! 录音时：① 音频热路径把每个客户端的原始音频各自存一轨（`Hub::recordings`）；
//! ② 输出混音进入"隔离"模式，只发音乐、不发其他人的声音。

use std::net::SocketAddr;

use jt_core::{parse_record_cmd, CMD_START};

use crate::hub::Hub;
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
        hub.recording = cmd == CMD_START;
        if hub.recording {
            hub.recordings.clear(); // 重新录制，丢弃上一版
        }
        true
    }
}
