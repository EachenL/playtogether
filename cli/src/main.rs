//! jt-cli — a headless client for quickly testing the audio path without the
//! Tauri GUI. Run `jt-server --echo` in one terminal and `jt-cli
//! 127.0.0.1:9000` in another to hear your microphone looped back.
//!
//! Usage:
//!   jt-cli <server_ip:port>

use std::net::SocketAddr;

use jt_engine::ClientSession;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let addr: SocketAddr = args
        .get(1)
        .ok_or("usage: jt-cli <server_ip:port>")?
        .parse()?;

    println!("connecting to {addr} ...");
    let session = ClientSession::start(addr)?;
    println!("streaming. press Ctrl-C to stop.");

    loop {
        std::thread::sleep(std::time::Duration::from_millis(500));
        println!(
            "jitter depth: {} samples | RTT: {} ms | est. one-way: {:.1} ms",
            session.depth(),
            session.rtt_ms(),
            session.est_one_way_ms()
        );
    }
}
