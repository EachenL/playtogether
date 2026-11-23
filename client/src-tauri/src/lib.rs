//! Tauri GUI client. Thin wrapper over `jt_engine::ClientSession`.
//!
//! Threading note: `cpal::Stream` is deliberately NOT `Send`/`Sync` on some
//! platforms (notably macOS CoreAudio), so a `ClientSession` must never move
//! across threads. We therefore keep the session owned by ONE dedicated engine
//! thread for its whole lifetime, and the Tauri commands talk to it over a
//! plain `mpsc` channel (which IS `Send`). This avoids any `unsafe`.

use std::net::ToSocketAddrs;
use std::sync::mpsc::{self, Sender};
use std::sync::OnceLock;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use jt_engine::{ClientSession, Mode, SessionConfig};

/// Messages the GUI sends to the engine thread.
enum Cmd {
    Start(SessionConfig, Sender<String>),
    Stop,
    Status(Sender<String>),
    SetTalking(bool),
    SetName(String),
    SetGain(u32, f32),
    Roster(Sender<serde_json::Value>),
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
            Cmd::Start(config, resp) => {
                session = None; // stop any existing session first
                let room = config.room_id;
                let server = config.server;
                let msg = match ClientSession::start(config) {
                    Ok(s) => {
                        session = Some(s);
                        format!("已进入房间 {room:04} · streaming to {server}")
                    }
                    Err(e) => format!("failed: {e}"),
                };
                let _ = resp.send(msg);
            }
            Cmd::SetTalking(t) => {
                if let Some(s) = &session {
                    s.set_talking(t);
                }
            }
            Cmd::SetName(name) => {
                if let Some(s) = &session {
                    s.set_name(&name);
                }
            }
            Cmd::SetGain(peer, gain) => {
                if let Some(s) = &session {
                    s.set_gain(peer, gain);
                }
            }
            Cmd::Roster(resp) => {
                let v = match &session {
                    Some(s) => serde_json::json!({
                        "own_id": s.own_id(),
                        "members": s
                            .roster()
                            .iter()
                            .map(|(id, name)| serde_json::json!({ "id": id, "name": name }))
                            .collect::<Vec<_>>(),
                    }),
                    None => serde_json::json!({ "own_id": 0, "members": [] }),
                };
                let _ = resp.send(v);
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
fn start_session(
    server: String,
    room: String,
    mode: String,
    input_device: Option<String>,
    output_device: Option<String>,
) -> Result<String, String> {
    let addr = jt_engine::resolve_addr(&server).map_err(|e| format!("invalid address: {e}"))?;
    let room_id = jt_engine::parse_room_code(&room).map_err(|e| e.to_string())?;
    let mode = match mode.as_str() {
        "voice" => Mode::Voice,
        _ => Mode::Rehearsal,
    };
    let config = SessionConfig {
        server: addr,
        room_id,
        mode,
        input_device,
        output_device,
    };
    let (tx, rx) = mpsc::channel();
    cmd_tx()
        .send(Cmd::Start(config, tx))
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

/// List audio input/output devices for the local device picker.
#[tauri::command]
fn list_devices() -> Result<serde_json::Value, String> {
    let inputs = jt_engine::list_input_devices();
    let outputs = jt_engine::list_output_devices();
    Ok(serde_json::json!({
        "inputs": inputs
            .iter()
            .map(|d| serde_json::json!({ "name": d.name, "is_default": d.is_default }))
            .collect::<Vec<_>>(),
        "outputs": outputs
            .iter()
            .map(|d| serde_json::json!({ "name": d.name, "is_default": d.is_default }))
            .collect::<Vec<_>>(),
    }))
}

/// Push-to-talk / mute control for voice mode.
#[tauri::command]
fn set_talking(talking: bool) -> Result<(), String> {
    cmd_tx()
        .send(Cmd::SetTalking(talking))
        .map_err(|e| e.to_string())
}

/// Set this client's display name.
#[tauri::command]
fn set_name(name: String) -> Result<(), String> {
    cmd_tx().send(Cmd::SetName(name)).map_err(|e| e.to_string())
}

/// Set this client's local gain for one peer.
#[tauri::command]
fn set_gain(peer: u32, gain: f32) -> Result<(), String> {
    cmd_tx()
        .send(Cmd::SetGain(peer, gain))
        .map_err(|e| e.to_string())
}

/// Fetch the current room roster (own id + member id/name list).
#[tauri::command]
fn get_roster() -> Result<serde_json::Value, String> {
    let (tx, rx) = mpsc::channel();
    cmd_tx().send(Cmd::Roster(tx)).map_err(|e| e.to_string())?;
    rx.recv_timeout(Duration::from_millis(500))
        .map_err(|e| e.to_string())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            start_session,
            stop_session,
            session_status,
            ping_server,
            list_devices,
            set_talking,
            set_name,
            set_gain,
            get_roster
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
