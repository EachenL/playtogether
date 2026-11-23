//! jt-cli — a headless client for quickly testing the audio path without the
//! Tauri GUI.
//!
//! Usage:
//!   jt-cli <server_host[:port]> <room_code>

use jt_engine::{ClientSession, Mode, SessionConfig};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = std::env::args().collect();
    let server = args
        .get(1)
        .ok_or("usage: jt-cli <server_host[:port]> <room_code>")?;
    let room_str = args
        .get(2)
        .ok_or("usage: jt-cli <server_host[:port]> <room_code>")?;

    let addr = jt_engine::resolve_addr(server)?;
    let room = jt_engine::parse_room_code(room_str)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;
    let mode = match args.get(3).map(|s| s.as_str()) {
        Some("voice") => Mode::Voice,
        _ => Mode::Rehearsal,
    };

    let config = SessionConfig {
        server: addr,
        room_id: room,
        mode,
        input_device: None,
        output_device: None,
    };

    println!("connecting to {addr} in room {room:04} ({mode:?}) ...");
    let session = ClientSession::start(config)?;
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
