//! Integration tests: spawn the real `jt-server` binary and verify the full
//! UDP round-trip (audio echo, ping echo, multi-client mixing, room isolation,
//! and room-full rejection) without needing audio hardware.

use std::net::UdpSocket;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU16, Ordering};
use std::time::Duration;

use jt_core::{make_ping, parse_ping, parse_room_full, AudioBlock};

/// Kills the server child on drop, even if a test panics.
struct ServerGuard(Child);

impl Drop for ServerGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

static NEXT_PORT: AtomicU16 = AtomicU16::new(0);

fn alloc_port() -> u16 {
    let base = 20000 + (std::process::id() as u16 % 10000);
    base + NEXT_PORT.fetch_add(1, Ordering::SeqCst)
}

fn spawn_server(echo: bool) -> (ServerGuard, u16) {
    let port = alloc_port();
    let bin = env!("CARGO_BIN_EXE_jt-server");
    let mut cmd = Command::new(bin);
    cmd.arg("--port").arg(port.to_string());
    if echo {
        cmd.arg("--echo");
    }
    let child = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn jt-server");
    wait_ready(port);
    (ServerGuard(child), port)
}

fn spawn_server_limited(max_clients: usize) -> (ServerGuard, u16) {
    let port = alloc_port();
    let bin = env!("CARGO_BIN_EXE_jt-server");
    let mut cmd = Command::new(bin);
    cmd.arg("--port").arg(port.to_string());
    cmd.arg("--max-room-clients").arg(max_clients.to_string());
    let child = cmd
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("failed to spawn jt-server");
    wait_ready(port);
    (ServerGuard(child), port)
}

fn wait_ready(port: u16) {
    let sock = UdpSocket::bind("127.0.0.1:0").expect("bind readiness probe");
    sock.connect(("127.0.0.1", port)).expect("connect readiness probe");
    sock.set_read_timeout(Some(Duration::from_millis(100)))
        .expect("set readiness timeout");
    let ping = make_ping(0);
    let mut buf = [0u8; 64];
    for _ in 0..40 {
        let _ = sock.send(&ping);
        if sock.recv(&mut buf).is_ok() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("server on port {port} did not become ready (port conflict?)");
}

fn connect(port: u16) -> UdpSocket {
    let sock = UdpSocket::bind("127.0.0.1:0").expect("bind client socket");
    sock.connect(("127.0.0.1", port)).expect("connect to server");
    sock.set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    sock
}

fn send_block(sock: &UdpSocket, room: u32, samples: &[i16]) {
    let block = AudioBlock::new(0, samples.to_vec());
    let mut buf = Vec::new();
    block.serialize_into(room, &mut buf);
    sock.send(&buf).expect("send audio block");
}

/// Receive the next audio block, skipping control packets (welcome/roster).
fn recv_block(sock: &UdpSocket) -> Vec<i16> {
    recv_audio(sock).expect("receive audio")
}

/// Receive the next audio block, skipping control packets; `None` on timeout.
fn recv_audio(sock: &UdpSocket) -> Option<Vec<i16>> {
    let mut recv = [0u8; 65536];
    loop {
        match sock.recv(&mut recv) {
            Ok(n) => {
                if let Some((_, block)) = AudioBlock::deserialize(&recv[..n]) {
                    return Some(block.samples);
                }
                // control packet (welcome/roster) — keep waiting
            }
            Err(_) => return None, // timeout
        }
    }
}

#[test]
fn server_echoes_audio_and_ping() {
    let (_guard, port) = spawn_server(true);
    let sock = connect(port);

    let samples = vec![100i16, -200, 300, -32768, 32767];
    send_block(&sock, 1234, &samples);
    assert_eq!(recv_block(&sock), samples, "echoed samples must match");

    sock.send(&make_ping(42)).expect("send ping");
    let mut recv = [0u8; 65536];
    loop {
        let n = sock.recv(&mut recv).expect("receive ping echo");
        if let Some(ts) = parse_ping(&recv[..n]) {
            assert_eq!(ts, 42, "ping must round-trip");
            break;
        }
        // else control packet
    }
}

#[test]
fn server_mixes_others_and_excludes_self() {
    let (_guard, port) = spawn_server(false);
    let a = connect(port);
    let b = connect(port);

    send_block(&a, 1234, &[100, 0, 0]);
    send_block(&b, 1234, &[0, 200, 0]);
    assert_eq!(recv_block(&b), vec![100, 0, 0], "B must hear A only");

    send_block(&a, 1234, &[100, 0, 0]);
    assert_eq!(recv_block(&a), vec![0, 200, 0], "A must hear B only");
}

#[test]
fn server_isolates_rooms() {
    let (_guard, port) = spawn_server(false);
    let a = connect(port);
    let b = connect(port);

    send_block(&a, 1111, &[100, 0, 0]);
    send_block(&b, 2222, &[0, 200, 0]);

    // A sends again in room 1111 — must get no AUDIO (B is in another room).
    send_block(&a, 1111, &[100, 0, 0]);
    assert!(
        recv_audio(&a).is_none(),
        "A must not hear B (different rooms)"
    );
}

#[test]
fn server_rejects_room_when_full() {
    let (_guard, port) = spawn_server_limited(2);
    let a = connect(port);
    let b = connect(port);
    let c = connect(port);

    send_block(&a, 1234, &[100, 0, 0]);
    send_block(&b, 1234, &[0, 200, 0]);

    send_block(&c, 1234, &[300, 0, 0]);
    let mut recv = [0u8; 65536];
    let n = c.recv(&mut recv).expect("expect room-full notification");
    assert!(parse_room_full(&recv[..n]), "must receive a room-full signal");
}
