//! jt-server — a JackTrip-style mixing hub.
//!
//! Each client sends one mono audio stream (uncompressed PCM over UDP). The
//! server keeps the latest block from every client and, on each received
//! packet, mixes every *other* client's audio and sends the result back to the
//! sender. This is the classic "hear everyone but yourself" hub topology.
//!
//! Usage:
//!   jt-server [--port N] [--echo]
//!
//!   --echo  Also mix the sender's own audio back (useful for single-machine
//!           loopback testing, where there is no second client).

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, Instant};

use jt_core::{parse_ping, AudioBlock, Sample};

struct ClientState {
    last_block: Vec<Sample>,
    last_seen: Instant,
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut port: u16 = 9000;
    let mut echo = false;

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
            other => eprintln!("ignoring unknown arg: {other}"),
        }
        i += 1;
    }

    let socket = UdpSocket::bind(("0.0.0.0", port))?;
    println!(
        "jt-server listening on 0.0.0.0:{}{}",
        port,
        if echo { " (echo mode)" } else { "" }
    );

    let mut clients: HashMap<SocketAddr, ClientState> = HashMap::new();
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

        // Echo ping packets back to the sender so clients can measure RTT.
        if let Some(_ts) = parse_ping(&buf[..n]) {
            let _ = socket.send_to(&buf[..n], src);
            continue;
        }

        let Some((_client_id, block)) = AudioBlock::deserialize(&buf[..n]) else {
            continue; // not our packet
        };

        let state = clients
            .entry(src)
            .or_insert(ClientState {
                last_block: Vec::new(),
                last_seen: Instant::now(),
            });
        state.last_block = block.samples;
        state.last_seen = Instant::now();

        // Mix every client except the sender (or including it in echo mode).
        let mut blocks: Vec<&[Sample]> = Vec::new();
        for (addr, cs) in clients.iter() {
            if !echo && *addr == src {
                continue;
            }
            blocks.push(cs.last_block.as_slice());
        }

        if !blocks.is_empty() {
            let resp = AudioBlock::new(0, jt_core::mix_blocks(&blocks));
            let mut out = Vec::new();
            resp.serialize_into(0, &mut out);
            let _ = socket.send_to(&out, src);
        }

        // Prune clients we haven't heard from in a while.
        if last_prune.elapsed() >= Duration::from_secs(1) {
            clients.retain(|_, cs| cs.last_seen.elapsed() < Duration::from_secs(10));
            last_prune = Instant::now();
            println!("clients: {}", clients.len());
        }
    }
}
