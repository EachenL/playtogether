//! Integration tests: spawn the real `jt-server` binary and verify the full
//! UDP round-trip (audio echo, ping echo, and multi-client mixing) without
//! needing audio hardware.

use std::net::UdpSocket;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use jt_core::{make_ping, parse_ping, AudioBlock};

/// Spawn `jt-server` on a free UDP port, wait for it to bind, and return the
/// child handle plus the chosen port. `echo` enables single-client loopback.
fn spawn_server(echo: bool) -> (Child, u16) {
    let probe = UdpSocket::bind("127.0.0.1:0").expect("bind probe socket");
    let port = probe.local_addr().unwrap().port();
    drop(probe);

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

    // Let the server bind its socket before we talk to it.
    std::thread::sleep(Duration::from_millis(300));
    (child, port)
}

fn connect(port: u16) -> UdpSocket {
    let sock = UdpSocket::bind("127.0.0.1:0").expect("bind client socket");
    sock.connect(("127.0.0.1", port)).expect("connect to server");
    sock.set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    sock
}

fn send_block(sock: &UdpSocket, samples: &[i16]) {
    let block = AudioBlock::new(0, samples.to_vec());
    let mut buf = Vec::new();
    block.serialize_into(0, &mut buf);
    sock.send(&buf).expect("send audio block");
}

fn recv_block(sock: &UdpSocket) -> Vec<i16> {
    let mut recv = [0u8; 65536];
    let n = sock.recv(&mut recv).expect("receive audio");
    AudioBlock::deserialize(&recv[..n])
        .expect("deserialize audio")
        .1
        .samples
}

#[test]
fn server_echoes_audio_and_ping() {
    let (mut child, port) = spawn_server(true);
    let sock = connect(port);

    // --- audio echo: sent samples must come back unchanged ---
    let samples = vec![100i16, -200, 300, -32768, 32767];
    send_block(&sock, &samples);
    assert_eq!(recv_block(&sock), samples, "echoed samples must match");

    // --- ping echo: timestamp must round-trip ---
    sock.send(&make_ping(42)).expect("send ping");
    let mut recv = [0u8; 65536];
    let n = sock.recv(&mut recv).expect("receive ping echo");
    assert_eq!(parse_ping(&recv[..n]), Some(42), "ping must round-trip");

    child.kill().expect("kill server");
    child.wait().expect("wait for server exit");
}

#[test]
fn server_mixes_others_and_excludes_self() {
    let (mut child, port) = spawn_server(false);
    let a = connect(port);
    let b = connect(port);

    // A registers first; no response yet (no other clients).
    send_block(&a, &[100, 0, 0]);

    // B registers; server should send B the mix of A only.
    send_block(&b, &[0, 200, 0]);
    assert_eq!(recv_block(&b), vec![100, 0, 0], "B must hear A only");

    // A sends again; server should send A the mix of B only.
    send_block(&a, &[100, 0, 0]);
    assert_eq!(recv_block(&a), vec![0, 200, 0], "A must hear B only");

    child.kill().expect("kill server");
    child.wait().expect("wait for server exit");
}
