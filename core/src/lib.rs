//! jt-core — shared types and wire protocol for the JackTrip-style demo.
//!
//! This crate is deliberately dependency-free (std only) so both the server
//! and the client can use it without pulling in any audio or GUI machinery.
//!
//! Design notes (mirroring JackTrip / Jamulus):
//! * Audio travels over UDP as *uncompressed* 16-bit PCM — the JackTrip
//!   approach, trading bandwidth for zero codec latency and full fidelity.
//! * A per-packet sequence number lets a real jitter buffer reorder and drop
//!   stale blocks. This demo keeps the buffer simple (see `jt-engine`), but the
//!   sequence number is on the wire so you can upgrade without a protocol
//!   change.

/// Canonical sample rate the demo targets (informational).
pub const SAMPLE_RATE_HZ: u32 = 48_000;

/// Canonical audio sample: 16-bit signed PCM.
pub type Sample = i16;

/// Packet magic bytes, "JT01".
pub const MAGIC: [u8; 4] = *b"JT01";

/// Wire header size in bytes: magic(4) + room_id(4) + seq(4) + sample_count(4).
pub const HEADER_SIZE: usize = 16;

/// One block of mono audio as it travels over the wire.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioBlock {
    /// Sequence number, wraps around (u32).
    pub seq: u32,
    /// Mono interleaved 16-bit samples.
    pub samples: Vec<Sample>,
}

impl AudioBlock {
    pub fn new(seq: u32, samples: Vec<Sample>) -> Self {
        Self { seq, samples }
    }

    /// Serialize into `out` (clears it first). `room_id` is the 4-digit
    /// rehearsal-room code (0000–9999) that scopes this stream to a room.
    ///
    /// Layout (little-endian):
    /// ```text
    /// [0..4)   MAGIC "JT01"
    /// [4..8)   room_id    u32
    /// [8..12)  seq        u32
    /// [12..16) num_samples u32
    /// [16..)   samples    i16 x num_samples
    /// ```
    pub fn serialize_into(&self, room_id: u32, out: &mut Vec<u8>) {
        out.clear();
        out.reserve(HEADER_SIZE + self.samples.len() * 2);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&room_id.to_le_bytes());
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&(self.samples.len() as u32).to_le_bytes());
        for s in &self.samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
    }

    /// Parse a packet. Returns `(room_id, block)` on success.
    pub fn deserialize(buf: &[u8]) -> Option<(u32, AudioBlock)> {
        if buf.len() < HEADER_SIZE || buf[..4] != MAGIC {
            return None;
        }
        let room_id = u32::from_le_bytes(buf[4..8].try_into().ok()?);
        let seq = u32::from_le_bytes(buf[8..12].try_into().ok()?);
        let n = u32::from_le_bytes(buf[12..16].try_into().ok()?) as usize;
        if buf.len() != HEADER_SIZE + n * 2 {
            return None;
        }
        let mut samples = Vec::with_capacity(n);
        for i in 0..n {
            let lo = buf[HEADER_SIZE + i * 2];
            let hi = buf[HEADER_SIZE + i * 2 + 1];
            samples.push(Sample::from_le_bytes([lo, hi]));
        }
        Some((room_id, AudioBlock { seq, samples }))
    }
}

/// Ping magic, "JTPN". Used to measure round-trip time (RTT).
pub const MAGIC_PING: [u8; 4] = *b"JTPN";

/// Build a ping packet carrying a millisecond timestamp.
/// Layout: `MAGIC_PING` (4 bytes) + `timestamp_ms` u64 little-endian = 12 bytes.
pub fn make_ping(timestamp_ms: u64) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    out.extend_from_slice(&MAGIC_PING);
    out.extend_from_slice(&timestamp_ms.to_le_bytes());
    out
}

/// Parse a ping packet, returning the echoed millisecond timestamp.
pub fn parse_ping(buf: &[u8]) -> Option<u64> {
    if buf.len() == 12 && buf[..4] == MAGIC_PING {
        Some(u64::from_le_bytes(buf[4..12].try_into().ok()?))
    } else {
        None
    }
}

/// "Room full" notification magic, "JTFU".
pub const MAGIC_FULL: [u8; 4] = *b"JTFU";

/// Build a "room full" notification packet (sent by the server to a client
/// that tried to join a room that has reached its capacity).
pub fn make_room_full() -> Vec<u8> {
    MAGIC_FULL.to_vec()
}

/// Return true if `buf` is a "room full" notification.
pub fn parse_room_full(buf: &[u8]) -> bool {
    buf == MAGIC_FULL
}

/// "Welcome" magic, "JTWC": server tells a client its assigned id.
pub const MAGIC_WELCOME: [u8; 4] = *b"JTWC";
/// "Name" magic, "JTNM": client sets its display name.
pub const MAGIC_NAME: [u8; 4] = *b"JTNM";
/// "Roster" magic, "JTRS": server broadcasts the room roster (id + name list).
pub const MAGIC_ROSTER: [u8; 4] = *b"JTRS";
/// "Gain" magic, "JTGN": client sets its local gain for one peer.
pub const MAGIC_GAIN: [u8; 4] = *b"JTGN";

pub fn make_welcome(client_id: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(8);
    out.extend_from_slice(&MAGIC_WELCOME);
    out.extend_from_slice(&client_id.to_le_bytes());
    out
}

pub fn parse_welcome(buf: &[u8]) -> Option<u32> {
    if buf.len() == 8 && buf[..4] == MAGIC_WELCOME {
        Some(u32::from_le_bytes(buf[4..8].try_into().ok()?))
    } else {
        None
    }
}

pub fn make_name(name: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + name.len());
    out.extend_from_slice(&MAGIC_NAME);
    out.extend_from_slice(name.as_bytes());
    out
}

pub fn parse_name(buf: &[u8]) -> Option<String> {
    if buf.len() >= 4 && buf[..4] == MAGIC_NAME {
        String::from_utf8(buf[4..].to_vec()).ok()
    } else {
        None
    }
}

pub fn make_roster(entries: &[(u32, String)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC_ROSTER);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (id, name) in entries {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&(name.len() as u16).to_le_bytes());
        out.extend_from_slice(name.as_bytes());
    }
    out
}

pub fn parse_roster(buf: &[u8]) -> Option<Vec<(u32, String)>> {
    if buf.len() < 6 || buf[..4] != MAGIC_ROSTER {
        return None;
    }
    let count = u16::from_le_bytes(buf[4..6].try_into().ok()?) as usize;
    let mut entries = Vec::with_capacity(count);
    let mut pos = 6;
    for _ in 0..count {
        if pos + 6 > buf.len() {
            return None;
        }
        let id = u32::from_le_bytes(buf[pos..pos + 4].try_into().ok()?);
        let nlen = u16::from_le_bytes(buf[pos + 4..pos + 6].try_into().ok()?) as usize;
        pos += 6;
        if pos + nlen > buf.len() {
            return None;
        }
        let name = String::from_utf8(buf[pos..pos + nlen].to_vec()).ok()?;
        pos += nlen;
        entries.push((id, name));
    }
    Some(entries)
}

pub fn make_gain(peer_id: u32, gain: f32) -> Vec<u8> {
    let mut out = Vec::with_capacity(12);
    out.extend_from_slice(&MAGIC_GAIN);
    out.extend_from_slice(&peer_id.to_le_bytes());
    out.extend_from_slice(&gain.to_le_bytes());
    out
}

pub fn parse_gain(buf: &[u8]) -> Option<(u32, f32)> {
    if buf.len() == 12 && buf[..4] == MAGIC_GAIN {
        let peer = u32::from_le_bytes(buf[4..8].try_into().ok()?);
        let gain = f32::from_le_bytes(buf[8..12].try_into().ok()?);
        Some((peer, gain))
    } else {
        None
    }
}

/// 音乐控制 magic, "JTMU"：同步播放内置 demo 伴奏。
pub const MAGIC_MUSIC: [u8; 4] = *b"JTMU";
/// 录音控制 magic, "JTRC"：开始/停止服务器多轨录音（录音时互相隔离）。
pub const MAGIC_RECORD: [u8; 4] = *b"JTRC";
/// 回放控制 magic, "JTPB"：开始/停止全员同步播放录制结果。
pub const MAGIC_PLAYBACK: [u8; 4] = *b"JTPB";

/// 控制命令：停止。
pub const CMD_STOP: u8 = 0;
/// 控制命令：开始。
pub const CMD_START: u8 = 1;

fn make_cmd(magic: [u8; 4], cmd: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(5);
    out.extend_from_slice(&magic);
    out.push(cmd);
    out
}

fn parse_cmd(buf: &[u8], magic: [u8; 4]) -> Option<u8> {
    if buf.len() == 5 && buf[..4] == magic {
        Some(buf[4])
    } else {
        None
    }
}

pub fn make_music_cmd(cmd: u8) -> Vec<u8> {
    make_cmd(MAGIC_MUSIC, cmd)
}

pub fn parse_music_cmd(buf: &[u8]) -> Option<u8> {
    parse_cmd(buf, MAGIC_MUSIC)
}

pub fn make_record_cmd(cmd: u8) -> Vec<u8> {
    make_cmd(MAGIC_RECORD, cmd)
}

pub fn parse_record_cmd(buf: &[u8]) -> Option<u8> {
    parse_cmd(buf, MAGIC_RECORD)
}

pub fn make_playback_cmd(recording_id: u32, cmd: u8) -> Vec<u8> {
    let mut out = Vec::with_capacity(9);
    out.extend_from_slice(&MAGIC_PLAYBACK);
    out.extend_from_slice(&recording_id.to_le_bytes());
    out.push(cmd);
    out
}

pub fn parse_playback_cmd(buf: &[u8]) -> Option<(u32, u8)> {
    if buf.len() == 9 && buf[..4] == MAGIC_PLAYBACK {
        let id = u32::from_le_bytes(buf[4..8].try_into().ok()?);
        Some((id, buf[8]))
    } else {
        None
    }
}

/// 录制列表 magic, "JTRL"：服务器广播历史录制（id + 时长毫秒）。
pub const MAGIC_RECORD_LIST: [u8; 4] = *b"JTRL";

pub fn make_record_list(entries: &[(u32, u32)]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&MAGIC_RECORD_LIST);
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (id, dur_ms) in entries {
        out.extend_from_slice(&id.to_le_bytes());
        out.extend_from_slice(&dur_ms.to_le_bytes());
    }
    out
}

pub fn parse_record_list(buf: &[u8]) -> Option<Vec<(u32, u32)>> {
    if buf.len() < 6 || buf[..4] != MAGIC_RECORD_LIST {
        return None;
    }
    let count = u16::from_le_bytes(buf[4..6].try_into().ok()?) as usize;
    let mut entries = Vec::with_capacity(count);
    let mut pos = 6;
    for _ in 0..count {
        if pos + 8 > buf.len() {
            return None;
        }
        let id = u32::from_le_bytes(buf[pos..pos + 4].try_into().ok()?);
        let dur_ms = u32::from_le_bytes(buf[pos + 4..pos + 8].try_into().ok()?);
        pos += 8;
        entries.push((id, dur_ms));
    }
    Some(entries)
}

/// Mix (saturating sum) multiple mono sample blocks into one.
/// Blocks may differ in length; the output is as long as the longest block.
/// This is the server-side mixing kernel.
pub fn mix_blocks(blocks: &[&[Sample]]) -> Vec<Sample> {
    let mut mix: Vec<i32> = Vec::new();
    for b in blocks {
        for (j, &s) in b.iter().enumerate() {
            if j >= mix.len() {
                mix.push(0);
            }
            mix[j] += s as i32;
        }
    }
    mix.iter()
        .map(|&v| v.clamp(i16::MIN as i32, i16::MAX as i32) as Sample)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let block = AudioBlock::new(42, vec![-1, 0, 1, 32767, -32768]);
        let mut buf = Vec::new();
        block.serialize_into(7, &mut buf);
        let (room, parsed) = AudioBlock::deserialize(&buf).unwrap();
        assert_eq!(room, 7);
        assert_eq!(parsed, block);
    }

    #[test]
    fn rejects_bad_magic() {
        assert!(AudioBlock::deserialize(b"XXXX....").is_none());
        assert!(AudioBlock::deserialize(b"").is_none());
    }

    #[test]
    fn ping_roundtrip() {
        let pkt = make_ping(123_456_789);
        assert_eq!(parse_ping(&pkt), Some(123_456_789));
        assert!(parse_ping(b"JTPNxxxx").is_none()); // wrong length
        assert!(parse_ping(b"XXXX00000000").is_none()); // wrong magic
    }

    #[test]
    fn mix_blocks_sums_and_saturates() {
        let a = [100i16, 200, 300];
        let b = [-50i16, 400, -500];
        assert_eq!(mix_blocks(&[&a[..], &b[..]]), vec![50, 600, -200]);

        // Positive saturation: 30000 + 30000 = 60000 > i16::MAX → 32767.
        let hi = [30000i16; 3];
        assert_eq!(mix_blocks(&[&hi[..], &hi[..]]), vec![32767, 32767, 32767]);

        // Negative saturation.
        let lo = [-30000i16; 3];
        assert_eq!(mix_blocks(&[&lo[..], &lo[..]]), vec![-32768, -32768, -32768]);

        // Differing lengths: output is as long as the longest block.
        let short = [1i16];
        let long = [10i16, 20, 30];
        assert_eq!(mix_blocks(&[&short[..], &long[..]]), vec![11, 20, 30]);

        // Empty input → empty output.
        assert!(mix_blocks(&[]).is_empty());
    }

    #[test]
    fn control_cmd_roundtrip() {
        assert_eq!(parse_music_cmd(&make_music_cmd(CMD_START)), Some(CMD_START));
        assert_eq!(parse_music_cmd(&make_music_cmd(CMD_STOP)), Some(CMD_STOP));
        assert_eq!(parse_record_cmd(&make_record_cmd(CMD_START)), Some(CMD_START));
        assert_eq!(parse_playback_cmd(&make_playback_cmd(7, CMD_STOP)), Some((7, CMD_STOP)));
        // 录制列表往返
        let list = vec![(1, 1000), (2, 2500)];
        assert_eq!(parse_record_list(&make_record_list(&list)), Some(list));
        // 错误 magic / 缺命令字节
        assert!(parse_music_cmd(b"XXXX").is_none());
        assert!(parse_music_cmd(b"JTMU").is_none());
        assert!(parse_record_cmd(b"JTMU\x01").is_none());
    }
}
