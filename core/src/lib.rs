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

/// Wire header size in bytes: magic(4) + client_id(4) + seq(4) + sample_count(4).
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

    /// Serialize into `out` (clears it first). `client_id` is the sender id;
    /// the demo uses `0` everywhere and relies on the UDP source address.
    ///
    /// Layout (little-endian):
    /// ```text
    /// [0..4)   MAGIC "JT01"
    /// [4..8)   client_id  u32
    /// [8..12)  seq        u32
    /// [12..16) num_samples u32
    /// [16..)   samples    i16 x num_samples
    /// ```
    pub fn serialize_into(&self, client_id: u32, out: &mut Vec<u8>) {
        out.clear();
        out.reserve(HEADER_SIZE + self.samples.len() * 2);
        out.extend_from_slice(&MAGIC);
        out.extend_from_slice(&client_id.to_le_bytes());
        out.extend_from_slice(&self.seq.to_le_bytes());
        out.extend_from_slice(&(self.samples.len() as u32).to_le_bytes());
        for s in &self.samples {
            out.extend_from_slice(&s.to_le_bytes());
        }
    }

    /// Parse a packet. Returns `(client_id, block)` on success.
    pub fn deserialize(buf: &[u8]) -> Option<(u32, AudioBlock)> {
        if buf.len() < HEADER_SIZE || buf[..4] != MAGIC {
            return None;
        }
        let client_id = u32::from_le_bytes(buf[4..8].try_into().ok()?);
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
        Some((client_id, AudioBlock { seq, samples }))
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
        let (id, parsed) = AudioBlock::deserialize(&buf).unwrap();
        assert_eq!(id, 7);
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
}
