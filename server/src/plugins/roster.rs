//! Roster 插件 —— 处理 JTNM 改名消息，维护房间花名册。

use std::net::SocketAddr;

use jt_core::parse_name;

use crate::hub::Hub;
use crate::plugin::Plugin;

pub struct RosterPlugin;

impl Plugin for RosterPlugin {
    fn name(&self) -> &'static str {
        "roster"
    }

    fn on_message(&mut self, hub: &mut Hub, src: SocketAddr, buf: &[u8], n: usize) -> bool {
        let Some(name) = parse_name(&buf[..n]) else {
            return false;
        };
        if let Some(cs) = hub.clients.get_mut(&src) {
            cs.name = name;
            let room = cs.room;
            hub.broadcast_roster(room);
        }
        true
    }
}
