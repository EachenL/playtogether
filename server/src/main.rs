//! jt-server — a JackTrip-style mixing hub with rehearsal-room isolation,
//! named participants, and per-peer server-side gain mixing.
//!
//! Each client sends one mono audio stream (uncompressed PCM over UDP) tagged
//! with a 4-digit room code. The server keeps the latest block from every
//! client, and on each received packet mixes every *other* same-room client's
//! audio — applying the *receiver's* per-peer gains — and sends the result back
//! to the sender. It also assigns each client an id, tracks their display name,
//! and broadcasts the room roster (id + name) so peers can see who joined and
//! set per-peer volume.
//!
//! Usage:
//!   jt-server [--port N] [--echo] [--room-timeout SECS] [--max-room-clients N]

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use jt_core::{parse_gain, parse_name, parse_ping, AudioBlock, Sample};

/// A client is dropped if no packet arrives for this long.
const CLIENT_TIMEOUT: Duration = Duration::from_secs(10);

struct ClientState {
    room: u32,
    client_id: u32,
    name: String,
    last_block: Vec<Sample>,
    last_seen: Instant,
    /// This client's per-peer gains: peer client_id → gain multiplier.
    gains: HashMap<u32, f32>,
}

fn broadcast_roster(socket: &UdpSocket, clients: &HashMap<SocketAddr, ClientState>, room: u32) {
    let mut entries: Vec<(u32, String)> = clients
        .values()
        .filter(|cs| cs.room == room)
        .map(|cs| (cs.client_id, cs.name.clone()))
        .collect();
    entries.sort_by_key(|(id, _)| *id);
    let pkt = jt_core::make_roster(&entries);
    for (addr, cs) in clients.iter() {
        if cs.room == room {
            let _ = socket.send_to(&pkt, *addr);
        }
    }
}

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

    let mut clients: HashMap<SocketAddr, ClientState> = HashMap::new();
    let mut rooms: HashMap<u32, Instant> = HashMap::new(); // room code → last active
    let mut next_client_id: u32 = 1;
    let mut buf = [0u8; 65536];
    let mut last_prune = Instant::now();

    loop {
        let (n, src) = match socket.recv_from(&mut buf) {
            Ok(v) => v,
            Err(e) => {
                eprintln!("recv error: {e}");
                continue;
            }
        };

        // Echo ping packets back so clients can measure RTT.
        if let Some(_ts) = parse_ping(&buf[..n]) {
            let _ = socket.send_to(&buf[..n], src);
            continue;
        }

        // Display-name announcement.
        if let Some(name) = parse_name(&buf[..n]) {
            if let Some(cs) = clients.get_mut(&src) {
                cs.name = name;
                let room = cs.room;
                broadcast_roster(&socket, &clients, room);
            }
            continue;
        }

        // Per-peer gain update.
        if let Some((peer, gain)) = parse_gain(&buf[..n]) {
            if let Some(cs) = clients.get_mut(&src) {
                cs.gains.insert(peer, gain);
            }
            continue;
        }

        let Some((room, block)) = AudioBlock::deserialize(&buf[..n]) else {
            continue; // not our packet
        };

        // New client registration.
        if !clients.contains_key(&src) {
            let in_room = clients.values().filter(|cs| cs.room == room).count();
            if in_room >= max_room_clients {
                let _ = socket.send_to(&jt_core::make_room_full(), src);
                continue; // room full — reject
            }
            let cid = next_client_id;
            next_client_id += 1;
            clients.insert(
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
            let _ = socket.send_to(&jt_core::make_welcome(cid), src);
            broadcast_roster(&socket, &clients, room);
        }

        // Existing client: refresh block + timestamp.
        {
            let cs = clients.get_mut(&src).unwrap();
            cs.last_block = block.samples;
            cs.last_seen = Instant::now();
        }

        // Mix for the sender, applying the sender's per-peer gains.
        let (room, gains) = {
            let cs = clients.get(&src).unwrap();
            (cs.room, cs.gains.clone())
        };
        let mut mix: Vec<i32> = Vec::new();
        for (addr, cs) in clients.iter() {
            if cs.room != room || (!echo && *addr == src) {
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

        if !mix.is_empty() {
            let clamped: Vec<Sample> = mix
                .iter()
                .map(|&v| v.clamp(i16::MIN as i32, i16::MAX as i32) as Sample)
                .collect();
            let resp = AudioBlock::new(0, clamped);
            let mut out = Vec::new();
            resp.serialize_into(room, &mut out);
            let _ = socket.send_to(&out, src);
        }

        // Periodic housekeeping (every second).
        if last_prune.elapsed() >= Duration::from_secs(1) {
            last_prune = Instant::now();

            let before = clients.len();
            clients.retain(|_, cs| cs.last_seen.elapsed() < CLIENT_TIMEOUT);

            // Refresh each active room's last-active timestamp.
            for cs in clients.values() {
                rooms.insert(cs.room, Instant::now());
            }

            // Release rooms idle longer than the timeout.
            let timeout = Duration::from_secs(room_timeout_secs);
            rooms.retain(|room, last_active| {
                if last_active.elapsed() >= timeout {
                    println!("room {room:04} released (idle > {room_timeout_secs}s)");
                    false
                } else {
                    true
                }
            });

            // If membership changed, push an updated roster to every live room.
            if clients.len() != before {
                let room_ids: Vec<u32> = rooms.keys().copied().collect();
                for r in room_ids {
                    broadcast_roster(&socket, &clients, r);
                }
            }

            println!("clients: {} | rooms: {}", clients.len(), rooms.len());
        }
    }
}
