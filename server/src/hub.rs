//! Hub — 服务器核心状态与音频热路径。
//!
//! 这里的 `ClientState` / `Hub` 是**音频数据面**（JT01 混音）直接读写的数据；
//! 插件只通过 `&mut Hub` 更新**控制面**状态（改名/音量/花名册），不会进入
//! 每 10ms 一次的音频热路径。

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use jt_core::Sample;

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

/// 服务器核心：socket + 客户端/房间状态 + 混音所需的一切。
pub struct Hub {
    pub socket: UdpSocket,
    pub clients: HashMap<SocketAddr, ClientState>,
    pub rooms: HashMap<u32, Instant>, // room code → last active
    pub next_client_id: u32,
    pub max_room_clients: usize,
    pub room_timeout: Duration,
    pub echo: bool,
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
}
