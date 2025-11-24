use std::collections::HashMap;
use std::time::{Duration, Instant};

use bytes::Bytes;
use once_cell::sync::Lazy;
use thiserror::Error;
use tracing::warn;

pub const CHUNK_VERSION: u8 = 0xC1;
const HEADER_LEN: usize = 1 + 16 + 4 + 4;
pub const DEFAULT_MAX_CHUNK_BYTES: usize = 16 * 1024;
pub const DEFAULT_MAX_MESSAGE_BYTES: usize = 1024 * 1024;
pub const DEFAULT_GC_TIMEOUT: Duration = Duration::from_secs(10);
const DEFAULT_MAX_INFLIGHT: usize = 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChunkConfig {
    pub max_chunk_bytes: usize,
    pub max_message_bytes: usize,
    pub max_inflight: usize,
    pub gc_timeout: Duration,
}

impl ChunkConfig {
    pub fn from_env() -> Self {
        let max_chunk_bytes = parse_usize_env(
            "BEACH_WEBRTC_MAX_CHUNK_BYTES",
            DEFAULT_MAX_CHUNK_BYTES,
            HEADER_LEN + 1,
        );
        let max_message_bytes = parse_usize_env(
            "BEACH_WEBRTC_MAX_MESSAGE_BYTES",
            DEFAULT_MAX_MESSAGE_BYTES,
            HEADER_LEN + 1,
        );
        Self {
            max_chunk_bytes,
            max_message_bytes,
            max_inflight: DEFAULT_MAX_INFLIGHT,
            gc_timeout: DEFAULT_GC_TIMEOUT,
        }
    }

    pub fn payload_capacity(&self) -> usize {
        self.max_chunk_bytes.saturating_sub(HEADER_LEN).max(1)
    }

    pub fn max_chunks(&self) -> usize {
        let payload_cap = self.payload_capacity();
        (self.max_message_bytes + payload_cap - 1) / payload_cap
    }

    pub fn backpressure_budget(&self) -> u64 {
        self.max_message_bytes
            .saturating_mul(2)
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum ChunkError {
    #[error("message exceeds max size: {0} bytes")]
    MessageTooLarge(usize),
    #[error("chunk frame too large: {0} bytes")]
    ChunkTooLarge(usize),
    #[error("chunk frame malformed: {0}")]
    Malformed(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkFrame {
    pub msg_id: u128,
    pub seq: u32,
    pub total: u32,
    pub payload: Bytes,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GcReason {
    Timeout,
    Capacity,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcEvent {
    pub msg_id: u128,
    pub reason: GcReason,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reassembled {
    pub payload: Bytes,
    pub started_at: Instant,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct IngestOutcome {
    pub completed: Option<Reassembled>,
    pub gc_events: Vec<GcEvent>,
}

#[derive(Debug)]
struct PartialMessage {
    created_at: Instant,
    total: u32,
    chunks: Vec<Option<Bytes>>,
    received: u32,
    received_bytes: usize,
}

impl PartialMessage {
    fn new(total: u32, created_at: Instant) -> Self {
        Self {
            created_at,
            total,
            chunks: vec![None; total as usize],
            received: 0,
            received_bytes: 0,
        }
    }
}

pub struct Reassembler {
    partials: HashMap<u128, PartialMessage>,
    config: ChunkConfig,
}

impl Reassembler {
    pub fn new(config: ChunkConfig) -> Self {
        Self {
            partials: HashMap::new(),
            config,
        }
    }

    pub fn ingest(&mut self, frame: ChunkFrame, now: Instant) -> Result<IngestOutcome, ChunkError> {
        let mut outcome = IngestOutcome::default();

        validate_chunk_bounds(&frame, &self.config)?;
        if frame.total == 1 && frame.seq == 0 {
            if frame.payload.len() > self.config.max_message_bytes {
                return Err(ChunkError::MessageTooLarge(frame.payload.len()));
            }
            let ChunkFrame { payload, .. } = frame;
            outcome.completed = Some(Reassembled {
                payload,
                started_at: now,
            });
            return Ok(outcome);
        }

        let ChunkFrame {
            msg_id,
            seq,
            total,
            payload,
        } = frame;

        if self.partials.len() >= self.config.max_inflight {
            if let Some(evicted) = self.evict_oldest() {
                outcome.gc_events.push(evicted);
            }
        }

        let mut total_mismatch = false;
        let mut oversize_bytes = None;
        let mut completed_payload: Option<(Bytes, Instant)> = None;

        {
            let entry = self
                .partials
                .entry(msg_id)
                .or_insert_with(|| PartialMessage::new(total, now));

            if entry.total != total {
                total_mismatch = true;
            } else {
                let seq = seq as usize;
                if entry.chunks[seq].is_none() {
                    entry.chunks[seq] = Some(payload.clone());
                    entry.received += 1;
                    entry.received_bytes = entry.received_bytes.saturating_add(payload.len());
                    if entry.received_bytes > self.config.max_message_bytes {
                        oversize_bytes = Some(entry.received_bytes);
                    }
                }

                if entry.received == entry.total && oversize_bytes.is_none() {
                    let mut combined = Vec::with_capacity(entry.received_bytes);
                    for chunk in entry.chunks.iter() {
                        if let Some(payload) = chunk {
                            combined.extend_from_slice(payload);
                        } else {
                            return Err(ChunkError::Malformed("missing chunk during reassembly"));
                        }
                    }
                    let started_at = entry.created_at;
                    completed_payload = Some((Bytes::from(combined), started_at));
                }
            }
        }

        if total_mismatch {
            self.partials.remove(&msg_id);
            return Err(ChunkError::Malformed("chunk total changed for message"));
        }

        if let Some(size) = oversize_bytes {
            self.partials.remove(&msg_id);
            return Err(ChunkError::MessageTooLarge(size));
        }

        if let Some((payload, started_at)) = completed_payload {
            self.partials.remove(&msg_id);
            outcome.completed = Some(Reassembled {
                payload,
                started_at,
            });
        }

        Ok(outcome)
    }

    pub fn gc(&mut self, now: Instant) -> Vec<GcEvent> {
        let mut dropped = Vec::new();
        self.partials.retain(|msg_id, partial| {
            let expired =
                now.saturating_duration_since(partial.created_at) > self.config.gc_timeout;
            if expired {
                dropped.push(GcEvent {
                    msg_id: *msg_id,
                    reason: GcReason::Timeout,
                });
            }
            !expired
        });
        dropped
    }

    fn evict_oldest(&mut self) -> Option<GcEvent> {
        let oldest = self
            .partials
            .iter()
            .min_by_key(|(_, partial)| partial.created_at)
            .map(|(msg_id, _)| *msg_id)?;
        self.partials.remove(&oldest);
        Some(GcEvent {
            msg_id: oldest,
            reason: GcReason::Capacity,
        })
    }
}

pub fn runtime_config() -> &'static ChunkConfig {
    static CONFIG: Lazy<ChunkConfig> = Lazy::new(ChunkConfig::from_env);
    &CONFIG
}

pub fn chunking_enabled() -> bool {
    static ENABLED: Lazy<bool> = Lazy::new(|| match std::env::var("BEACH_WEBRTC_CHUNKING") {
        Ok(value) => !matches!(value.trim(), "0" | "false" | "no"),
        Err(_) => true,
    });
    *ENABLED
}

pub fn split_message(
    payload: &[u8],
    msg_id: u128,
    config: &ChunkConfig,
) -> Result<Vec<ChunkFrame>, ChunkError> {
    if payload.len() > config.max_message_bytes {
        return Err(ChunkError::MessageTooLarge(payload.len()));
    }
    let payload_cap = config.payload_capacity();
    let max_chunks = config.max_chunks();

    if payload.is_empty() {
        return Ok(vec![ChunkFrame {
            msg_id,
            seq: 0,
            total: 1,
            payload: Bytes::new(),
        }]);
    }

    let mut frames = Vec::new();
    for (seq, chunk) in payload.chunks(payload_cap).enumerate() {
        let seq_u32 = u32::try_from(seq)
            .map_err(|_| ChunkError::Malformed("chunk sequence overflowed u32"))?;
        frames.push(ChunkFrame {
            msg_id,
            seq: seq_u32,
            total: 0, // patched below
            payload: Bytes::copy_from_slice(chunk),
        });
    }

    if frames.len() > max_chunks {
        return Err(ChunkError::MessageTooLarge(payload.len()));
    }

    let total_u32 =
        u32::try_from(frames.len()).map_err(|_| ChunkError::Malformed("chunk total overflow"))?;
    for frame in frames.iter_mut() {
        frame.total = total_u32;
    }

    Ok(frames)
}

pub fn encode_chunk(frame: &ChunkFrame) -> Bytes {
    let mut buf = Vec::with_capacity(HEADER_LEN.saturating_add(frame.payload.len()));
    buf.push(CHUNK_VERSION);
    buf.extend_from_slice(&frame.msg_id.to_be_bytes());
    buf.extend_from_slice(&frame.seq.to_be_bytes());
    buf.extend_from_slice(&frame.total.to_be_bytes());
    buf.extend_from_slice(&frame.payload);
    Bytes::from(buf)
}

pub fn decode_chunk(bytes: &[u8], config: &ChunkConfig) -> Result<Option<ChunkFrame>, ChunkError> {
    if bytes.first().copied() != Some(CHUNK_VERSION) {
        return Ok(None);
    }
    if bytes.len() < HEADER_LEN {
        return Err(ChunkError::Malformed("chunk frame too short"));
    }
    if bytes.len() > config.max_chunk_bytes {
        return Err(ChunkError::ChunkTooLarge(bytes.len()));
    }
    let mut id_bytes = [0u8; 16];
    id_bytes.copy_from_slice(&bytes[1..17]);
    let msg_id = u128::from_be_bytes(id_bytes);
    let seq = u32::from_be_bytes(bytes[17..21].try_into().unwrap());
    let total = u32::from_be_bytes(bytes[21..25].try_into().unwrap());
    if total == 0 {
        return Err(ChunkError::Malformed("chunk total cannot be zero"));
    }
    if seq >= total {
        return Err(ChunkError::Malformed("chunk seq exceeds total"));
    }
    if total as usize > config.max_chunks() {
        return Err(ChunkError::MessageTooLarge(
            total as usize * config.payload_capacity(),
        ));
    }

    let payload = &bytes[HEADER_LEN..];
    if payload.len() > config.payload_capacity() {
        return Err(ChunkError::ChunkTooLarge(bytes.len()));
    }

    let payload = Bytes::copy_from_slice(payload);
    let frame = ChunkFrame {
        msg_id,
        seq,
        total,
        payload,
    };
    validate_chunk_bounds(&frame, config)?;
    Ok(Some(frame))
}

fn validate_chunk_bounds(frame: &ChunkFrame, config: &ChunkConfig) -> Result<(), ChunkError> {
    if frame.seq >= frame.total {
        return Err(ChunkError::Malformed("chunk seq out of range"));
    }
    if frame.total == 0 {
        return Err(ChunkError::Malformed("chunk total cannot be zero"));
    }
    if frame.payload.len() > config.payload_capacity() {
        return Err(ChunkError::ChunkTooLarge(frame.payload.len()));
    }
    if frame.total as usize > config.max_chunks() {
        return Err(ChunkError::MessageTooLarge(
            frame.total as usize * config.payload_capacity(),
        ));
    }
    Ok(())
}

fn parse_usize_env(var: &str, default: usize, min: usize) -> usize {
    match std::env::var(var) {
        Ok(value) => match value.trim().parse::<usize>() {
            Ok(parsed) if parsed >= min => parsed,
            Ok(parsed) => {
                warn!(
                    target = "beach::transport::webrtc::chunk",
                    var, parsed, min, default, "chunk config below minimum; using default"
                );
                default
            }
            Err(err) => {
                warn!(
                    target = "beach::transport::webrtc::chunk",
                    var,
                    error = %err,
                    default,
                    "failed to parse chunk config from env; using default"
                );
                default
            }
        },
        Err(_) => default,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::{seq::SliceRandom, thread_rng};

    #[test]
    fn split_and_reassemble_round_trip() {
        let config = ChunkConfig {
            max_chunk_bytes: 32,
            max_message_bytes: 1024,
            max_inflight: DEFAULT_MAX_INFLIGHT,
            gc_timeout: Duration::from_secs(1),
        };
        let payload = b"the quick brown fox jumps over the lazy dog".to_vec();
        let msg_id = 7;
        let frames = split_message(&payload, msg_id, &config).expect("split");
        assert!(frames.len() > 1);

        let mut reassembler = Reassembler::new(config);
        let mut recovered = None;
        for frame in frames {
            let result = reassembler
                .ingest(frame, Instant::now())
                .expect("ingest chunk");
            if let Some(done) = result.completed {
                recovered = Some(done.payload);
            }
        }
        assert_eq!(recovered.as_deref(), Some(payload.as_slice()));
    }

    #[test]
    fn missing_chunk_gets_gced() {
        let config = ChunkConfig {
            max_chunk_bytes: 32,
            max_message_bytes: 256,
            max_inflight: DEFAULT_MAX_INFLIGHT,
            gc_timeout: Duration::from_millis(25),
        };
        let mut reassembler = Reassembler::new(config);
        let now = Instant::now();
        let first = ChunkFrame {
            msg_id: 9,
            seq: 0,
            total: 2,
            payload: Bytes::from_static(b"hello "),
        };
        let _ = reassembler.ingest(first, now).expect("ingest first");
        std::thread::sleep(Duration::from_millis(30));
        let dropped = reassembler.gc(Instant::now());
        assert_eq!(dropped.len(), 1);
        assert_eq!(dropped[0].reason, GcReason::Timeout);
        assert!(reassembler.partials.is_empty());
    }

    #[test]
    fn duplicate_and_out_of_order_chunks_reassemble() {
        let config = ChunkConfig {
            max_chunk_bytes: 16,
            max_message_bytes: 512,
            max_inflight: DEFAULT_MAX_INFLIGHT,
            gc_timeout: Duration::from_secs(1),
        };
        let payload = b"abcdefghijklmno".to_vec();
        let frames = split_message(&payload, 42, &config).expect("split");
        assert!(frames.len() > 2);

        let mut shuffled = frames.clone();
        shuffled.shuffle(&mut thread_rng());
        // add a duplicate of the first frame to ensure dedupe
        shuffled.push(frames[0].clone());

        let mut reassembler = Reassembler::new(config);
        let mut recovered = None;
        for frame in shuffled {
            let result = reassembler
                .ingest(frame, Instant::now())
                .expect("ingest chunk");
            if let Some(done) = result.completed {
                recovered = Some(done.payload);
            }
        }
        assert_eq!(recovered.as_deref(), Some(payload.as_slice()));
    }

    #[test]
    fn oversize_rejected() {
        let config = ChunkConfig {
            max_chunk_bytes: 16,
            max_message_bytes: 64,
            max_inflight: DEFAULT_MAX_INFLIGHT,
            gc_timeout: Duration::from_secs(1),
        };
        let payload = vec![0u8; 128];
        let err = split_message(&payload, 5, &config).expect_err("expected oversize error");
        assert!(matches!(err, ChunkError::MessageTooLarge(_)));
    }

    #[test]
    fn mock_channel_handles_large_payload() {
        let config = ChunkConfig {
            max_chunk_bytes: DEFAULT_MAX_CHUNK_BYTES,
            max_message_bytes: DEFAULT_MAX_MESSAGE_BYTES,
            max_inflight: DEFAULT_MAX_INFLIGHT,
            gc_timeout: Duration::from_secs(1),
        };
        let payload = vec![7u8; 140 * 1024];
        let msg_id = 1234;
        let frames = split_message(&payload, msg_id, &config).expect("split");
        assert!(frames.len() > 1);

        let mut channel = Vec::new();
        for frame in frames {
            channel.push(encode_chunk(&frame));
        }

        let mut reassembler = Reassembler::new(config);
        let mut recovered = None;
        for bytes in channel {
            let frame = decode_chunk(&bytes, &config)
                .expect("decode chunk")
                .expect("should be chunk frame");
            let result = reassembler
                .ingest(frame, Instant::now())
                .expect("ingest frame");
            if let Some(done) = result.completed {
                recovered = Some(done.payload);
            }
        }

        assert_eq!(recovered.as_deref(), Some(payload.as_slice()));
    }
}
