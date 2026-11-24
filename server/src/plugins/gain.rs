//! Gain 插件 —— 处理 JTGN 独立音量消息。

use std::net::SocketAddr;

use jt_core::parse_gain;

use crate::hub::Hub;
use crate::plugin::Plugin;

pub struct GainPlugin;

impl Plugin for GainPlugin {
    fn name(&self) -> &'static str {
        "gain"
    }

    fn on_message(&mut self, hub: &mut Hub, src: SocketAddr, buf: &[u8], n: usize) -> bool {
        let Some((peer, gain)) = parse_gain(&buf[..n]) else {
            return false;
        };
        if let Some(cs) = hub.clients.get_mut(&src) {
            cs.gains.insert(peer, gain);
        }
        true
    }
}
