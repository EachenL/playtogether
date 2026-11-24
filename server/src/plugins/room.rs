//! Room 插件 —— 房间人数上限 + 空闲房间释放策略。

use std::time::Instant;

use jt_core::make_room_full;

use crate::hub::Hub;
use crate::plugin::Plugin;

pub struct RoomPlugin;

impl Plugin for RoomPlugin {
    fn name(&self) -> &'static str {
        "room"
    }

    fn on_join(&mut self, hub: &mut Hub, room: u32) -> Option<Vec<u8>> {
        if hub.room_count(room) >= hub.max_room_clients {
            return Some(make_room_full());
        }
        None
    }

    fn on_tick(&mut self, hub: &mut Hub) {
        // 刷新每个活跃房间的最后活跃时间（基于存活客户端）。
        let ids: Vec<u32> = hub.clients.values().map(|cs| cs.room).collect();
        for r in ids {
            hub.rooms.insert(r, Instant::now());
        }

        // 释放空闲超时的房间。
        let timeout = hub.room_timeout;
        hub.rooms.retain(|room, last_active| {
            if last_active.elapsed() >= timeout {
                println!("room {room:04} released (idle > {}s)", timeout.as_secs());
                false
            } else {
                true
            }
        });
    }
}
