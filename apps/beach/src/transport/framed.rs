use std::collections::{HashMap, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::{LazyLock, RwLock};
use std::time::{Duration, Instant};

use bytes::Bytes;
use crc32c::crc32c;
use hmac::{Hmac, Mac};
use once_cell::sync::Lazy;
use sha2::Sha256;
use thiserror::Error;
use tokio::sync::broadcast;

use crate::metrics;
use crate::transport::TransportId;

const FRAME_VERSION: u8 = 0xA1;
const FLAG_MAC_PRESENT: u8 = 0x1;
const DEFAULT_CHUNK_SIZE: usize = 14 * 1024;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_MAX_INFLIGHT: usize = 512;
const DEFAULT_MAX_BYTES: usize = 8 * 1024 * 1024;
const MAC_TAG_LEN: usize = 32;
const RECENT_HISTORY: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FramedMessage {
    pub namespace: String,
    pub kind: String,
    pub seq: u64,
    pub payload: Bytes,
    pub total_len: u32,
}

type Namespace = String;

static FRAMED_TOPICS: LazyLock<
    RwLock<HashMap<TransportId, HashMap<Namespace, broadcast::Sender<FramedMessage>>>>,
> = LazyLock::new(|| RwLock::new(HashMap::new()));

pub fn subscribe(id: TransportId, namespace: &str) -> broadcast::Receiver<FramedMessage> {
    let sender = {
        let mut guard = FRAMED_TOPICS.write().expect("framed topic lock poisoned");
        guard
            .entry(id)
            .or_default()
            .entry(namespace.to_string())
            .or_insert_with(|| broadcast::channel(256).0)
            .clone()
    };
    sender.subscribe()
}

pub fn publish(id: TransportId, frame: FramedMessage) {
    if let Some(namespace_map) = FRAMED_TOPICS
        .read()
        .expect("framed topic lock poisoned")
        .get(&id)
    {
        if let Some(sender) = namespace_map.get(&frame.namespace) {
            let _ = sender.send(frame);
        }
    }
}

#[derive(Debug, Clone)]
pub struct MacKey {
    pub key_id: u8,
    pub key: Vec<u8>,
}

#[derive(Debug, Clone, Default)]
pub struct MacConfig {
    pub active_key: Option<u8>,
    pub keys: Vec<MacKey>,
}

#[derive(Debug, Clone)]
pub struct FramingConfig {
    pub chunk_size: usize,
    pub timeout: Duration,
    pub max_inflight: usize,
    pub max_bytes: usize,
    pub mac: Option<MacConfig>,
}

impl Default for FramingConfig {
    fn default() -> Self {
        Self {
            chunk_size: DEFAULT_CHUNK_SIZE,
            timeout: DEFAULT_TIMEOUT,
            max_inflight: DEFAULT_MAX_INFLIGHT,
            max_bytes: DEFAULT_MAX_BYTES,
            mac: None,
        }
    }
}

impl FramingConfig {
    pub fn backpressure_budget(&self) -> u64 {
        self.max_bytes
            .saturating_mul(2)
            .try_into()
            .unwrap_or(u64::MAX)
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum FramingError {
    #[error("namespace too long")]
    NamespaceTooLong,
    #[error("kind too long")]
    KindTooLong,
    #[error("payload too large: {0}")]
    PayloadTooLarge(usize),
    #[error("unsupported frame version {0}")]
    UnsupportedVersion(u8),
    #[error("malformed frame: {0}")]
    Malformed(&'static str),
    #[error("crc mismatch")]
    CrcMismatch,
    #[error("mac missing from payload")]
    MacMissing,
    #[error("mac key {0} unknown")]
    UnknownMacKey(u8),
    #[error("mac verification failed")]
    MacMismatch,
}

#[derive(Debug, Clone)]
struct ParsedChunk {
    namespace: String,
    kind: String,
    seq: u64,
    total_len: u32,
    chunk_index: u16,
    chunk_count: u16,
    crc32c: u32,
    payload: Bytes,
    mac: Option<ParsedMac>,
}

#[derive(Debug, Clone)]
struct ParsedMac {
    key_id: u8,
    tag: [u8; MAC_TAG_LEN],
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct MessageKey {
    namespace: String,
    kind: String,
    seq: u64,
}

impl Hash for MessageKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.namespace.hash(state);
        self.kind.hash(state);
        self.seq.hash(state);
    }
}

#[derive(Debug)]
struct Assembly {
    namespace: String,
    kind: String,
    seq: u64,
    total_len: u32,
    chunk_count: u16,
    crc32c: u32,
    mac: Option<ParsedMac>,
    received: Vec<Option<Bytes>>,
    received_bytes: usize,
    started_at: Instant,
}

impl Assembly {
    fn new(meta: &ParsedChunk, now: Instant) -> Self {
        Self {
            namespace: meta.namespace.clone(),
            kind: meta.kind.clone(),
            seq: meta.seq,
            total_len: meta.total_len,
            chunk_count: meta.chunk_count,
            crc32c: meta.crc32c,
            mac: meta.mac.clone(),
            received: vec![None; meta.chunk_count as usize],
            received_bytes: 0,
            started_at: now,
        }
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct QueueDepth {
    pub inflight_messages: usize,
    pub inflight_bytes: usize,
}

pub fn encode_message(
    namespace: &str,
    kind: &str,
    seq: u64,
    payload: &[u8],
    config: &FramingConfig,
) -> Result<Vec<Bytes>, FramingError> {
    if namespace.len() > u8::MAX as usize {
        return Err(FramingError::NamespaceTooLong);
    }
    if kind.len() > u8::MAX as usize {
        return Err(FramingError::KindTooLong);
    }
    if payload.len() > u32::MAX as usize {
        return Err(FramingError::PayloadTooLarge(payload.len()));
    }

    let crc = crc32c(payload);
    let mac = build_mac(namespace, kind, seq, payload, config)?;
    let chunk_size = config.chunk_size.max(1);
    let chunk_count = if payload.is_empty() {
        1
    } else {
        (payload.len() + chunk_size - 1) / chunk_size
    };
    if chunk_count == 0 || chunk_count > u16::MAX as usize {
        return Err(FramingError::PayloadTooLarge(payload.len()));
    }

    metrics::FRAMED_MESSAGES
        .with_label_values(&["sent", namespace, kind])
        .inc();

    let mut frames = Vec::with_capacity(chunk_count);
    let chunks_iter: Vec<&[u8]> = if payload.is_empty() {
        vec![&[][..]]
    } else {
        payload.chunks(chunk_size).collect()
    };
    for (index, chunk) in chunks_iter.iter().enumerate() {
        let header_len = 2 // version + flags
            + mac_key_len(&mac)
            + 1
            + 1
            + 8
            + 4
            + 2
            + 2
            + 4;
        let mac_len = mac.as_ref().map(|_| MAC_TAG_LEN).unwrap_or(0);
        let mut buf = Vec::with_capacity(header_len + chunk.len() + mac_len);
        buf.push(FRAME_VERSION);
        let mut flags = 0u8;
        if mac.is_some() {
            flags |= FLAG_MAC_PRESENT;
        }
        buf.push(flags);
        if let Some(mac) = &mac {
            buf.push(mac.key_id);
        }
        buf.push(namespace.len() as u8);
        buf.push(kind.len() as u8);
        buf.extend_from_slice(namespace.as_bytes());
        buf.extend_from_slice(kind.as_bytes());
        buf.extend_from_slice(&seq.to_be_bytes());
        buf.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        buf.extend_from_slice(&(index as u16).to_be_bytes());
        buf.extend_from_slice(&(chunk_count as u16).to_be_bytes());
        buf.extend_from_slice(&crc.to_be_bytes());
        buf.extend_from_slice(chunk);
        if let Some(mac) = &mac {
            buf.extend_from_slice(&mac.tag);
        }
        frames.push(Bytes::from(buf));
    }

    Ok(frames)
}

fn build_mac(
    namespace: &str,
    kind: &str,
    seq: u64,
    payload: &[u8],
    config: &FramingConfig,
) -> Result<Option<ParsedMac>, FramingError> {
    let mac_config = match &config.mac {
        Some(cfg) => cfg,
        None => return Ok(None),
    };
    let key_id = match mac_config.active_key {
        Some(id) => id,
        None => return Ok(None),
    };
    let key = mac_config
        .keys
        .iter()
        .find(|k| k.key_id == key_id)
        .ok_or(FramingError::UnknownMacKey(key_id))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&key.key)
        .map_err(|_| FramingError::Malformed("invalid mac key length"))?;
    mac.update(&[FRAME_VERSION]);
    mac.update(&[namespace.len() as u8]);
    mac.update(namespace.as_bytes());
    mac.update(&[kind.len() as u8]);
    mac.update(kind.as_bytes());
    mac.update(&seq.to_be_bytes());
    mac.update(&(payload.len() as u32).to_be_bytes());
    mac.update(payload);
    let tag_bytes = mac.finalize().into_bytes();
    let mut tag = [0u8; MAC_TAG_LEN];
    tag.copy_from_slice(&tag_bytes[..MAC_TAG_LEN]);
    Ok(Some(ParsedMac { key_id, tag }))
}

#[derive(Debug)]
pub struct FramedDecoder {
    config: FramingConfig,
    assemblies: HashMap<MessageKey, Assembly>,
    inflight_bytes: usize,
    recent: VecDeque<(MessageKey, Instant)>,
}

impl FramedDecoder {
    pub fn new(config: FramingConfig) -> Self {
        Self {
            config,
            assemblies: HashMap::new(),
            inflight_bytes: 0,
            recent: VecDeque::new(),
        }
    }

    pub fn queue_depth(&self) -> QueueDepth {
        QueueDepth {
            inflight_messages: self.assemblies.len(),
            inflight_bytes: self.inflight_bytes,
        }
    }

    pub fn ingest(
        &mut self,
        bytes: &[u8],
        now: Instant,
    ) -> Result<Option<FramedMessage>, FramingError> {
        self.gc(now);
        let parsed = parse_chunk(bytes, &self.config)?;
        let key = MessageKey {
            namespace: parsed.namespace.clone(),
            kind: parsed.kind.clone(),
            seq: parsed.seq,
        };

        if self.recent.iter().any(|(recent_key, _)| *recent_key == key) {
            return Ok(None);
        }

        if parsed.chunk_count == 1 && parsed.chunk_index == 0 {
            metrics::FRAMED_MESSAGES
                .with_label_values(&["received", &parsed.namespace, &parsed.kind])
                .inc();
            verify_payload(
                &parsed.namespace,
                &parsed.kind,
                parsed.seq,
                parsed.total_len,
                parsed.crc32c,
                parsed.mac.as_ref(),
                &parsed.payload,
                &self.config,
            )?;
            self.record_recent(key, now);
            self.update_gauges();
            return Ok(Some(FramedMessage {
                namespace: parsed.namespace,
                kind: parsed.kind,
                seq: parsed.seq,
                payload: parsed.payload,
                total_len: parsed.total_len,
            }));
        }

        let assembly = self
            .assemblies
            .entry(key.clone())
            .or_insert_with(|| Assembly::new(&parsed, now));

        if assembly.chunk_count != parsed.chunk_count || assembly.total_len != parsed.total_len {
            self.assemblies.remove(&key);
            return Err(FramingError::Malformed("chunk count or length changed"));
        }

        let slot = parsed.chunk_index as usize;
        if assembly.received[slot].is_none() {
            assembly.received_bytes = assembly.received_bytes.saturating_add(parsed.payload.len());
            assembly.received[slot] = Some(parsed.payload.clone());
            self.inflight_bytes = self.inflight_bytes.saturating_add(parsed.payload.len());
            if assembly.received_bytes > assembly.total_len as usize {
                self.assemblies.remove(&key);
                return Err(FramingError::Malformed(
                    "received bytes exceed total length",
                ));
            }
        }

        let ready_payload = if assembly.received.iter().all(|entry| entry.is_some()) {
            let mut combined = Vec::with_capacity(assembly.received_bytes);
            for part in assembly.received.iter() {
                if let Some(bytes) = part {
                    combined.extend_from_slice(bytes);
                } else {
                    return Err(FramingError::Malformed("missing chunk during assembly"));
                }
            }
            Some(Bytes::from(combined))
        } else {
            None
        };

        if let Some(payload) = ready_payload {
            let namespace = assembly.namespace.clone();
            let kind = assembly.kind.clone();
            let seq = assembly.seq;
            let total_len = assembly.total_len;
            let crc = assembly.crc32c;
            let mac = assembly.mac.clone();
            let received_bytes = assembly.received_bytes;

            verify_payload(
                &namespace,
                &kind,
                seq,
                total_len,
                crc,
                mac.as_ref(),
                &payload,
                &self.config,
            )?;
            let complete = FramedMessage {
                namespace: namespace.clone(),
                kind: kind.clone(),
                seq,
                total_len,
                payload: payload.clone(),
            };
            metrics::FRAMED_MESSAGES
                .with_label_values(&["received", &complete.namespace, &complete.kind])
                .inc();
            self.assemblies.remove(&key);
            self.inflight_bytes = self.inflight_bytes.saturating_sub(received_bytes);
            self.record_recent(key, now);
            self.update_gauges();
            Ok(Some(complete))
        } else {
            self.evict_over_budget();
            self.update_gauges();
            Ok(None)
        }
    }

    pub fn gc(&mut self, now: Instant) {
        let timeout = self.config.timeout;
        let mut expired = Vec::new();
        for (key, assembly) in self.assemblies.iter() {
            if now.duration_since(assembly.started_at) > timeout {
                expired.push(key.clone());
            }
        }
        for key in expired {
            if let Some(assembly) = self.assemblies.remove(&key) {
                self.inflight_bytes = self.inflight_bytes.saturating_sub(assembly.received_bytes);
                metrics::FRAMED_ERRORS
                    .with_label_values(&["reassembly_timeout"])
                    .inc();
            }
        }
        self.update_gauges();
        self.prune_recent(now);
    }

    fn record_recent(&mut self, key: MessageKey, now: Instant) {
        self.recent.push_back((key, now));
        if self.recent.len() > RECENT_HISTORY {
            self.recent.pop_front();
        }
    }

    fn prune_recent(&mut self, now: Instant) {
        while let Some((_, seen)) = self.recent.front() {
            if now.duration_since(*seen) > self.config.timeout {
                self.recent.pop_front();
            } else {
                break;
            }
        }
    }

    fn evict_over_budget(&mut self) {
        while self.assemblies.len() > self.config.max_inflight
            || self.inflight_bytes > self.config.max_bytes
        {
            if let Some((oldest_key, oldest_started)) = self
                .assemblies
                .iter()
                .min_by_key(|(_, assembly)| assembly.started_at)
                .map(|(key, assembly)| (key.clone(), assembly.started_at))
            {
                let over_capacity = self.assemblies.len() > self.config.max_inflight;
                if let Some(assembly) = self.assemblies.remove(&oldest_key) {
                    let reason = if over_capacity { "capacity" } else { "memory" };
                    metrics::FRAMED_ERRORS.with_label_values(&[reason]).inc();
                    let age_ms = oldest_started.elapsed().as_millis();
                    tracing::warn!(
                        target = "beach::transport::framed",
                        namespace = %oldest_key.namespace,
                        kind = %oldest_key.kind,
                        seq = oldest_key.seq,
                        age_ms,
                        reason,
                        "evicting partial framed message"
                    );
                    self.inflight_bytes =
                        self.inflight_bytes.saturating_sub(assembly.received_bytes);
                }
            } else {
                break;
            }
        }
        self.update_gauges();
    }

    fn update_gauges(&self) {
        metrics::FRAMED_QUEUE_DEPTH
            .with_label_values(&["inflight_messages"])
            .set(self.assemblies.len() as i64);
        metrics::FRAMED_QUEUE_DEPTH
            .with_label_values(&["inflight_bytes"])
            .set(self.inflight_bytes as i64);
    }
}

fn parse_chunk(bytes: &[u8], config: &FramingConfig) -> Result<ParsedChunk, FramingError> {
    if bytes.len() < 2 {
        return Err(FramingError::Malformed("frame too short"));
    }
    let version = bytes[0];
    if version != FRAME_VERSION {
        return Err(FramingError::UnsupportedVersion(version));
    }
    let flags = bytes[1];
    let mut cursor = 2;
    let mac_present = flags & FLAG_MAC_PRESENT != 0;
    let mac_key = if mac_present {
        ensure_len(bytes, cursor + 1)?;
        let key_id = bytes[cursor];
        cursor += 1;
        Some(key_id)
    } else {
        None
    };

    let namespace_len = read_u8(bytes, &mut cursor, "namespace len")? as usize;
    let kind_len = read_u8(bytes, &mut cursor, "kind len")? as usize;
    ensure_len(bytes, cursor + namespace_len + kind_len)?;
    let namespace = std::str::from_utf8(&bytes[cursor..cursor + namespace_len])
        .map_err(|_| FramingError::Malformed("namespace utf8"))?
        .to_string();
    cursor += namespace_len;
    let kind = std::str::from_utf8(&bytes[cursor..cursor + kind_len])
        .map_err(|_| FramingError::Malformed("kind utf8"))?
        .to_string();
    cursor += kind_len;
    let seq = read_u64(bytes, &mut cursor)?;
    let total_len = read_u32(bytes, &mut cursor)?;
    let chunk_index = read_u16(bytes, &mut cursor)?;
    let chunk_count = read_u16(bytes, &mut cursor)?;
    let crc32c = read_u32(bytes, &mut cursor)?;
    if chunk_count == 0 || chunk_index >= chunk_count {
        return Err(FramingError::Malformed("invalid chunk index/count"));
    }
    let mac_len = if mac_present { MAC_TAG_LEN } else { 0 };
    if bytes.len() < cursor + mac_len {
        return Err(FramingError::Malformed("frame missing payload"));
    }
    let payload_len = bytes.len() - cursor - mac_len;
    if payload_len > total_len as usize {
        return Err(FramingError::Malformed(
            "chunk payload exceeds total length",
        ));
    }
    if payload_len > config.chunk_size {
        return Err(FramingError::PayloadTooLarge(payload_len));
    }
    let payload = Bytes::copy_from_slice(&bytes[cursor..cursor + payload_len]);
    cursor += payload_len;
    let mac = if mac_present {
        let key_id = mac_key.ok_or(FramingError::MacMissing)?;
        let mut tag = [0u8; MAC_TAG_LEN];
        tag.copy_from_slice(&bytes[cursor..cursor + MAC_TAG_LEN]);
        Some(ParsedMac { key_id, tag })
    } else {
        None
    };

    Ok(ParsedChunk {
        namespace,
        kind,
        seq,
        total_len,
        chunk_index,
        chunk_count,
        crc32c,
        payload,
        mac,
    })
}

fn verify_payload(
    namespace: &str,
    kind: &str,
    seq: u64,
    total_len: u32,
    expected_crc: u32,
    mac: Option<&ParsedMac>,
    payload: &[u8],
    config: &FramingConfig,
) -> Result<(), FramingError> {
    if payload.len() != total_len as usize {
        return Err(FramingError::Malformed("length mismatch"));
    }
    let crc = crc32c(payload);
    if crc != expected_crc {
        metrics::FRAMED_ERRORS
            .with_label_values(&["crc_failure"])
            .inc();
        return Err(FramingError::CrcMismatch);
    }
    if let Some(mac) = mac {
        if namespace.len() > u8::MAX as usize || kind.len() > u8::MAX as usize {
            return Err(FramingError::Malformed("mac namespace/kind length"));
        }
        let mac_config = config.mac.as_ref().ok_or(FramingError::MacMissing)?;
        let key = mac_config
            .keys
            .iter()
            .find(|k| k.key_id == mac.key_id)
            .ok_or(FramingError::UnknownMacKey(mac.key_id))?;
        let mut verifier = Hmac::<Sha256>::new_from_slice(&key.key)
            .map_err(|_| FramingError::Malformed("invalid mac key length"))?;
        verifier.update(&[FRAME_VERSION]);
        verifier.update(&[namespace.len() as u8]);
        verifier.update(namespace.as_bytes());
        verifier.update(&[kind.len() as u8]);
        verifier.update(kind.as_bytes());
        verifier.update(&seq.to_be_bytes());
        verifier.update(&total_len.to_be_bytes());
        verifier.update(payload);
        if verifier.verify_slice(&mac.tag).is_err() {
            metrics::FRAMED_ERRORS
                .with_label_values(&["mac_failure"])
                .inc();
            return Err(FramingError::MacMismatch);
        }
    }
    Ok(())
}

fn read_u8(bytes: &[u8], cursor: &mut usize, ctx: &'static str) -> Result<u8, FramingError> {
    ensure_len(bytes, *cursor + 1)?;
    let value = bytes[*cursor];
    *cursor += 1;
    if value == 0 {
        return Err(FramingError::Malformed(ctx));
    }
    Ok(value)
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, FramingError> {
    ensure_len(bytes, *cursor + 2)?;
    let value = u16::from_be_bytes(bytes[*cursor..*cursor + 2].try_into().unwrap());
    *cursor += 2;
    Ok(value)
}

fn read_u32(bytes: &[u8], cursor: &mut usize) -> Result<u32, FramingError> {
    ensure_len(bytes, *cursor + 4)?;
    let value = u32::from_be_bytes(bytes[*cursor..*cursor + 4].try_into().unwrap());
    *cursor += 4;
    Ok(value)
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, FramingError> {
    ensure_len(bytes, *cursor + 8)?;
    let value = u64::from_be_bytes(bytes[*cursor..*cursor + 8].try_into().unwrap());
    *cursor += 8;
    Ok(value)
}

fn ensure_len(bytes: &[u8], needed: usize) -> Result<(), FramingError> {
    if bytes.len() < needed {
        return Err(FramingError::Malformed("frame too short"));
    }
    Ok(())
}

fn mac_key_len(mac: &Option<ParsedMac>) -> usize {
    mac.as_ref().map(|_| 1).unwrap_or(0)
}

fn parse_duration_env(var: &str, default: Duration) -> Duration {
    match std::env::var(var) {
        Ok(value) => match value.trim().parse::<u64>() {
            Ok(ms) => Duration::from_millis(ms),
            Err(err) => {
                tracing::warn!(
                    target = "beach::transport::framed",
                    var,
                    error = %err,
                    default_ms = default.as_millis(),
                    "invalid duration env; using default"
                );
                default
            }
        },
        Err(_) => default,
    }
}

fn parse_usize_env(var: &str, default: usize, min: usize) -> usize {
    match std::env::var(var) {
        Ok(value) => match value.trim().parse::<usize>() {
            Ok(parsed) if parsed >= min => parsed,
            Ok(parsed) => {
                tracing::warn!(
                    target = "beach::transport::framed",
                    var,
                    parsed,
                    min,
                    default,
                    "framing config below minimum; using default"
                );
                default
            }
            Err(err) => {
                tracing::warn!(
                    target = "beach::transport::framed",
                    var,
                    error = %err,
                    default,
                    "failed to parse framing config from env; using default"
                );
                default
            }
        },
        Err(_) => default,
    }
}

fn parse_mac_config() -> Option<MacConfig> {
    if let Ok(list) = std::env::var("BEACH_FRAMED_MAC_KEYS") {
        let mut keys = Vec::new();
        for entry in list.split(',').map(|v| v.trim()).filter(|v| !v.is_empty()) {
            if let Some((id_str, key_str)) = entry.split_once(':') {
                if let (Ok(id), Ok(bytes)) = (id_str.parse::<u8>(), hex::decode(key_str.trim())) {
                    keys.push(MacKey {
                        key_id: id,
                        key: bytes,
                    });
                }
            }
        }
        if keys.is_empty() {
            return None;
        }
        let active = std::env::var("BEACH_FRAMED_MAC_ACTIVE_ID")
            .ok()
            .and_then(|v| v.parse::<u8>().ok())
            .or_else(|| keys.first().map(|k| k.key_id));
        return Some(MacConfig {
            active_key: active,
            keys,
        });
    }

    let key = std::env::var("BEACH_FRAMED_MAC_KEY").ok()?;
    let key_bytes = hex::decode(key.trim()).ok()?;
    let key_id = std::env::var("BEACH_FRAMED_MAC_KEY_ID")
        .ok()
        .and_then(|v| v.parse::<u8>().ok())
        .unwrap_or(1);
    Some(MacConfig {
        active_key: Some(key_id),
        keys: vec![MacKey {
            key_id,
            key: key_bytes,
        }],
    })
}

pub fn runtime_config() -> &'static FramingConfig {
    static CONFIG: Lazy<FramingConfig> = Lazy::new(|| FramingConfig {
        chunk_size: parse_usize_env("BEACH_FRAMED_CHUNK_SIZE", DEFAULT_CHUNK_SIZE, 512),
        timeout: parse_duration_env("BEACH_FRAMED_TIMEOUT_MS", DEFAULT_TIMEOUT),
        max_inflight: parse_usize_env("BEACH_FRAMED_MAX_INFLIGHT", DEFAULT_MAX_INFLIGHT, 1),
        max_bytes: parse_usize_env("BEACH_FRAMED_MAX_BYTES", DEFAULT_MAX_BYTES, 1024),
        mac: parse_mac_config(),
    });
    &CONFIG
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transport::TransportId;
    use std::thread;

    fn decoder_with_mac(key_id: u8, key: &[u8]) -> (FramingConfig, FramedDecoder) {
        let mac = MacConfig {
            active_key: Some(key_id),
            keys: vec![MacKey {
                key_id,
                key: key.to_vec(),
            }],
        };
        let config = FramingConfig {
            mac: Some(mac),
            chunk_size: 1024,
            timeout: Duration::from_millis(50),
            max_inflight: 32,
            max_bytes: 64 * 1024,
        };
        let decoder = FramedDecoder::new(config.clone());
        (config, decoder)
    }

    #[test]
    fn round_trip_without_mac() {
        let config = FramingConfig {
            chunk_size: 4096,
            timeout: DEFAULT_TIMEOUT,
            max_inflight: DEFAULT_MAX_INFLIGHT,
            max_bytes: DEFAULT_MAX_BYTES,
            mac: None,
        };
        let payload = b"hello framed transport".to_vec();
        let frames = encode_message("controller", "input", 1, &payload, &config).expect("encode");
        assert_eq!(frames.len(), 1);

        let mut decoder = FramedDecoder::new(config);
        let now = Instant::now();
        let result = decoder
            .ingest(&frames[0], now)
            .expect("ingest")
            .expect("complete");
        assert_eq!(result.namespace, "controller");
        assert_eq!(result.kind, "input");
        assert_eq!(result.seq, 1);
        assert_eq!(result.payload.as_ref(), payload.as_slice());
    }

    #[test]
    fn chunking_and_mac_round_trip() {
        let key = [7u8; 32];
        let (config, mut decoder) = decoder_with_mac(2, &key);
        let payload = vec![9u8; 50_000];
        let frames = encode_message("sync", "state", 5, &payload, &config).expect("encode payload");
        assert!(frames.len() > 1);
        let now = Instant::now();
        let mut delivered = None;
        for frame in frames {
            let result = decoder.ingest(&frame, now).expect("ingest");
            if let Some(msg) = result {
                delivered = Some(msg);
            }
        }
        let message = delivered.expect("reassembled message");
        assert_eq!(message.namespace, "sync");
        assert_eq!(message.kind, "state");
        assert_eq!(message.seq, 5);
        assert_eq!(message.payload.len(), payload.len());
        assert_eq!(message.payload.as_ref(), payload.as_slice());
    }

    #[test]
    fn crc_failure_rejected() {
        let config = FramingConfig::default();
        let payload = b"crc check data".to_vec();
        let frames =
            encode_message("controller", "ack", 3, &payload, &config).expect("encode payload");
        let mut corrupted = frames[0].to_vec();
        let last = corrupted.len() - 1;
        corrupted[last] ^= 0xFF;
        let mut decoder = FramedDecoder::new(config);
        let result = decoder.ingest(&corrupted, Instant::now());
        assert!(matches!(result, Err(FramingError::CrcMismatch)));
    }

    #[test]
    fn mac_failure_rejected() {
        let key = [5u8; 32];
        let (config, mut decoder) = decoder_with_mac(9, &key);
        let payload = b"mac protected data".to_vec();
        let mut frames =
            encode_message("controller", "input", 10, &payload, &config).expect("encode payload");
        assert_eq!(frames.len(), 1);
        let mut tampered = frames.remove(0).to_vec();
        let end = tampered.len() - 1;
        tampered[end] ^= 0xAA;
        let result = decoder.ingest(&tampered, Instant::now());
        assert!(matches!(result, Err(FramingError::MacMismatch)));
    }

    #[test]
    fn assembly_evicted_on_timeout() {
        let config = FramingConfig {
            chunk_size: 8,
            timeout: Duration::from_millis(10),
            max_inflight: 4,
            max_bytes: 1024,
            mac: None,
        };
        let payload = vec![1u8; 32];
        let frames = encode_message("controller", "input", 99, &payload, &config).expect("encode");
        let mut decoder = FramedDecoder::new(config);
        let now = Instant::now();
        // ingest only first chunk
        let _ = decoder.ingest(&frames[0], now).expect("ingest");
        thread::sleep(Duration::from_millis(20));
        decoder.gc(Instant::now());
        assert_eq!(decoder.assemblies.len(), 0);
    }

    #[test]
    fn publish_and_subscribe_namespace() {
        let id = TransportId(42);
        let mut rx = subscribe(id, "controller");
        let frame = FramedMessage {
            namespace: "controller".into(),
            kind: "input".into(),
            seq: 1,
            payload: Bytes::from_static(b"ping"),
            total_len: 4,
        };
        publish(id, frame.clone());
        let received = rx.try_recv().expect("frame delivered");
        assert_eq!(received.namespace, frame.namespace);
        assert_eq!(received.kind, frame.kind);
        assert_eq!(received.seq, frame.seq);
        assert_eq!(received.payload, frame.payload);
    }
}
