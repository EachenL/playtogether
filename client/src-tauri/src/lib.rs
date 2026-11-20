//! Tauri GUI client. Thin wrapper over `jt_engine::ClientSession`.
//!
//! Threading note: `cpal::Stream` is deliberately NOT `Send`/`Sync` on some
//! platforms (notably macOS CoreAudio), so a `ClientSession` must never move
//! across threads. We therefore keep the session owned by ONE dedicated engine
//! thread for its whole lifetime, and the Tauri commands talk to it over a
//! plain `mpsc` channel (which IS `Send`). This avoids any `unsafe`.

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jt_engine::ClientSession;

/// Messages the GUI sends to the engine thread.
enum Cmd {
    Start(SocketAddr, Sender<String>),
    Stop,
    Status(Sender<String>),
}

static CMD_TX: OnceLock<Sender<Cmd>> = OnceLock::new();

fn cmd_tx() -> &'static Sender<Cmd> {
    CMD_TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Cmd>();
        std::thread::spawn(move || engine_loop(rx));
        tx
    })
}

/// Owns the `ClientSession` on one thread and processes control commands.
fn engine_loop(rx: mpsc::Receiver<Cmd>) {
    let mut session: Option<ClientSession> = None;
    while let Ok(cmd) = rx.recv() {
        match cmd {
            Cmd::Start(addr, resp) => {
                session = None; // stop any existing session first
                let msg = match ClientSession::start(addr) {
                    Ok(s) => {
                        session = Some(s);
                        format!("streaming to {addr}")
                    }
                    Err(e) => format!("failed: {e}"),
                };
                let _ = resp.send(msg);
            }
            Cmd::Stop => {
                session = None; // Drop stops the streams and joins threads
            }
            Cmd::Status(resp) => {
                let msg = match &session {
                    Some(s) => format!(
                        "streaming · jitter {} samples · RTT {} ms · one-way ~{:.1} ms",
                        s.depth(),
                        s.rtt_ms(),
                        s.est_one_way_ms()
                    ),
                    None => "idle".to_string(),
                };
                let _ = resp.send(msg);
            }
        }
    }
}

#[tauri::command]
fn start_session(server: String) -> Result<String, String> {
    let addr: SocketAddr = server.parse().map_err(|e| format!("invalid address: {e}"))?;
    let (tx, rx) = mpsc::channel();
    cmd_tx()
        .send(Cmd::Start(addr, tx))
        .map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_millis(1000))
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn stop_session() -> Result<(), String> {
    cmd_tx().send(Cmd::Stop).map_err(|e| e.to_string())
}

#[tauri::command]
fn session_status() -> String {
    let (tx, rx) = mpsc::channel();
    if cmd_tx().send(Cmd::Status(tx)).is_err() {
        return "engine thread unavailable".to_string();
    }
    rx.recv_timeout(Duration::from_millis(500))
        .unwrap_or_else(|_| "no response".to_string())
}

/// Ping-only latency probe: does NOT touch the audio device, so it works even
/// on machines without an input device (or without mic permission granted).
/// Sends a `JTPN` frame and reports RTT plus the one-way estimate.
#[tauri::command]
fn ping_server(server: String) -> Result<String, String> {
    let addr = server
        .to_socket_addrs()
        .map_err(|e| format!("cannot resolve {server}: {e}"))?
        .next()
        .ok_or_else(|| format!("cannot resolve {server}"))?;

    let sock = std::net::UdpSocket::bind("0.0.0.0:0").map_err(|e| e.to_string())?;
    sock.set_read_timeout(Some(Duration::from_millis(1500)))
        .map_err(|e| e.to_string())?;

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let packet = jt_core::make_ping(ts);

    let started = Instant::now();
    sock.send_to(&packet, addr).map_err(|e| e.to_string())?;

    let mut buf = [0u8; 64];
    let (n, _) = sock
        .recv_from(&mut buf)
        .map_err(|e| format!("no reply from {addr}: {e}"))?;
    let rtt_ms = started.elapsed().as_secs_f64() * 1000.0;

    match jt_core::parse_ping(&buf[..n]) {
        Some(_) => Ok(format!(
            "RTT {rtt_ms:.1} ms · 单向约 {:.1} ms",
            rtt_ms / 2.0
        )),
        None => Err("unexpected reply from server".to_string()),
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            start_session,
            stop_session,
            session_status,
            ping_server
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
