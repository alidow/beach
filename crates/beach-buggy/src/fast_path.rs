use crate::{ActionAck, ActionCommand, HarnessError, HarnessResult, StateDiff};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine as _};
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{hash_map::Entry, HashMap},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{broadcast, Mutex as TokioMutex},
    time::sleep,
};
use tracing::{info, trace, warn};
use url::Url;
use uuid::Uuid;
use webrtc::{
    api::APIBuilder,
    data_channel::{
        data_channel_init::RTCDataChannelInit, data_channel_message::DataChannelMessage,
        RTCDataChannel,
    },
    ice_transport::ice_candidate::RTCIceCandidateInit,
    peer_connection::{
        configuration::RTCConfiguration, peer_connection_state::RTCPeerConnectionState,
        sdp::session_description::RTCSessionDescription, RTCPeerConnection,
    },
};

/// Canonical data-channel labels used by the fast-path transport.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastPathChannels {
    pub actions: String,
    pub acks: String,
    pub state: String,
}

impl Default for FastPathChannels {
    fn default() -> Self {
        Self {
            actions: "mgr-actions".into(),
            acks: "mgr-acks".into(),
            state: "mgr-state".into(),
        }
    }
}

/// Fully-qualified endpoints harnesses use to negotiate the fast-path WebRTC session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FastPathEndpoints {
    pub offer_url: Url,
    pub ice_url: Url,
    pub channels: FastPathChannels,
    pub status: FastPathStatus,
}

/// Indicates rollout status communicated by the manager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastPathStatus {
    Experimental,
    Stable,
}

impl FastPathStatus {
    fn from_hint(value: Option<&str>) -> FastPathStatus {
        match value {
            Some("stable") => FastPathStatus::Stable,
            _ => FastPathStatus::Experimental,
        }
    }
}

const FAST_PATH_CHUNK_VERSION: u8 = 1;
const FAST_PATH_CHUNK_PAYLOAD_BYTES: usize = 14 * 1024;
const FAST_PATH_CHUNK_TTL: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum FastPathPayloadKind {
    Actions,
    Acks,
    State,
    Health,
}

impl FastPathPayloadKind {
    fn as_str(&self) -> &'static str {
        match self {
            FastPathPayloadKind::Actions => "actions",
            FastPathPayloadKind::Acks => "acks",
            FastPathPayloadKind::State => "state",
            FastPathPayloadKind::Health => "health",
        }
    }
}

pub fn frame_fast_path_payload(
    kind: FastPathPayloadKind,
    payload: &str,
) -> HarnessResult<Vec<String>> {
    if payload.len() <= FAST_PATH_CHUNK_PAYLOAD_BYTES {
        return Ok(vec![payload.to_string()]);
    }
    chunk_payload(kind, payload.as_bytes())
}

fn chunk_payload(kind: FastPathPayloadKind, payload: &[u8]) -> HarnessResult<Vec<String>> {
    if payload.is_empty() {
        return Ok(vec!["".into()]);
    }
    let id = format!("{}-{}", kind.as_str(), Uuid::new_v4());
    let total_chunks = ((payload.len() + FAST_PATH_CHUNK_PAYLOAD_BYTES - 1)
        / FAST_PATH_CHUNK_PAYLOAD_BYTES) as u32;
    let mut frames = Vec::with_capacity(total_chunks as usize);
    for (index, chunk) in payload.chunks(FAST_PATH_CHUNK_PAYLOAD_BYTES).enumerate() {
        let encoded = BASE64.encode(chunk);
        let envelope = serde_json::json!({
            "type": "chunk",
            "version": FAST_PATH_CHUNK_VERSION,
            "scope": kind.as_str(),
            "id": id,
            "index": index,
            "count": total_chunks,
            "payload": encoded,
        });
        frames.push(
            serde_json::to_string(&envelope)
                .map_err(|err| HarnessError::Transport(format!("encode chunk failed: {err}")))?,
        );
    }
    Ok(frames)
}

#[derive(Debug)]
struct PendingChunk {
    parts: Vec<Option<Vec<u8>>>,
    expected: u32,
    received: u32,
    expires_at: Instant,
}

impl PendingChunk {
    fn new(count: u32) -> HarnessResult<Self> {
        if count == 0 {
            return Err(HarnessError::Transport(
                "chunk envelope missing chunk count".into(),
            ));
        }
        Ok(Self {
            parts: vec![None; count as usize],
            expected: count,
            received: 0,
            expires_at: Instant::now() + FAST_PATH_CHUNK_TTL,
        })
    }

    fn insert(&mut self, index: u32, payload: Vec<u8>) -> HarnessResult<bool> {
        let idx = index as usize;
        if idx >= self.parts.len() {
            return Err(HarnessError::Transport(
                "chunk envelope index exceeds count".into(),
            ));
        }
        if self.parts[idx].is_none() {
            self.parts[idx] = Some(payload);
            self.received += 1;
        }
        self.expires_at = Instant::now() + FAST_PATH_CHUNK_TTL;
        Ok(self.received == self.expected)
    }

    fn into_payload(self) -> HarnessResult<Vec<u8>> {
        let mut merged = Vec::new();
        for part in self.parts.into_iter() {
            let chunk = part
                .ok_or_else(|| HarnessError::Transport("chunk assembly missing fragment".into()))?;
            merged.extend_from_slice(&chunk);
        }
        Ok(merged)
    }
}

#[derive(Debug)]
pub struct FastPathChunkReassembler {
    kind: FastPathPayloadKind,
    pending: HashMap<String, PendingChunk>,
}

impl FastPathChunkReassembler {
    pub fn new(kind: FastPathPayloadKind) -> Self {
        Self {
            kind,
            pending: HashMap::new(),
        }
    }

    pub fn ingest(&mut self, text: &str) -> HarnessResult<Option<String>> {
        match serde_json::from_str::<OwnedChunkEnvelope>(text) {
            Ok(envelope) => {
                if envelope.kind != "chunk" {
                    return Ok(Some(text.to_string()));
                }
                if envelope.scope != self.kind.as_str() {
                    return Err(HarnessError::Transport(format!(
                        "chunk scope mismatch: expected {}, got {}",
                        self.kind.as_str(),
                        envelope.scope
                    )));
                }
                if envelope.version != 0 && envelope.version != FAST_PATH_CHUNK_VERSION {
                    return Err(HarnessError::Transport(format!(
                        "unsupported chunk version {}",
                        envelope.version
                    )));
                }
                let decoded = BASE64.decode(envelope.payload.as_bytes()).map_err(|err| {
                    HarnessError::Transport(format!("decode chunk payload: {err}"))
                })?;
                let message_id = envelope.id.clone();
                let entry = match self.pending.entry(message_id.clone()) {
                    Entry::Occupied(entry) => entry.into_mut(),
                    Entry::Vacant(vacant) => vacant.insert(PendingChunk::new(envelope.count)?),
                };
                let complete = entry.insert(envelope.index, decoded)?;
                if complete {
                    let payload = self
                        .pending
                        .remove(&message_id)
                        .expect("pending chunk missing")
                        .into_payload()?;
                    let text = String::from_utf8(payload).map_err(|err| {
                        HarnessError::Transport(format!("chunk payload utf8 error: {err}"))
                    })?;
                    return Ok(Some(text));
                }
                self.cleanup_expired();
                Ok(None)
            }
            Err(_) => Ok(Some(text.to_string())),
        }
    }

    fn cleanup_expired(&mut self) {
        let now = Instant::now();
        self.pending.retain(|_, pending| pending.expires_at > now);
    }
}

#[derive(Debug, Deserialize)]
struct OwnedChunkEnvelope {
    #[serde(rename = "type")]
    kind: String,
    scope: String,
    #[serde(default)]
    version: u8,
    id: String,
    index: u32,
    count: u32,
    payload: String,
}

/// Parsed subset of `transport_hints.fast_path_webrtc`.
#[derive(Debug, Deserialize)]
struct RawFastPathHint {
    offer_path: String,
    ice_path: String,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    channels: Option<RawChannels>,
}

#[derive(Debug, Deserialize)]
struct RawChannels {
    #[serde(default)]
    actions: Option<String>,
    #[serde(default)]
    acks: Option<String>,
    #[serde(default)]
    state: Option<String>,
}

impl From<RawChannels> for FastPathChannels {
    fn from(value: RawChannels) -> Self {
        let mut channels = FastPathChannels::default();
        if let Some(label) = value.actions {
            if !label.is_empty() {
                channels.actions = label;
            }
        }
        if let Some(label) = value.acks {
            if !label.is_empty() {
                channels.acks = label;
            }
        }
        if let Some(label) = value.state {
            if !label.is_empty() {
                channels.state = label;
            }
        }
        channels
    }
}

/// Parses fast-path metadata from the manager's `transport_hints` payload.
pub fn parse_fast_path_endpoints(
    base_url: &Url,
    hints: &HashMap<String, Value>,
) -> HarnessResult<Option<FastPathEndpoints>> {
    let value = match hints.get("fast_path_webrtc") {
        Some(Value::Object(obj)) => {
            serde_json::from_value::<RawFastPathHint>(Value::Object(obj.clone()))
                .map_err(|err| HarnessError::Transport(format!("invalid fast-path hint: {err}")))?
        }
        Some(_) => {
            return Err(HarnessError::Transport(
                "fast_path_webrtc hint must be an object".into(),
            ))
        }
        None => return Ok(None),
    };

    let offer_url = join_path(base_url, &value.offer_path)?;
    let ice_url = join_path(base_url, &value.ice_path)?;
    let channels = value
        .channels
        .map(FastPathChannels::from)
        .unwrap_or_default();
    let status = FastPathStatus::from_hint(value.status.as_deref());

    Ok(Some(FastPathEndpoints {
        offer_url,
        ice_url,
        channels,
        status,
    }))
}

fn join_path(base: &Url, path: &str) -> HarnessResult<Url> {
    use std::borrow::Cow;

    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(HarnessError::Transport(
            "fast_path_webrtc path must not be empty".into(),
        ));
    }
    let normalized: Cow<'_, str> = if trimmed.starts_with('/') {
        Cow::Borrowed(trimmed)
    } else {
        Cow::Owned(format!("/{trimmed}"))
    };
    base.join(normalized.as_ref())
        .map_err(|err| HarnessError::Transport(format!("invalid fast-path url: {err}")))
}

#[derive(Debug, Clone)]
pub struct FastPathClient {
    pub endpoints: FastPathEndpoints,
    http: reqwest::Client,
}

impl FastPathClient {
    pub fn new(endpoints: FastPathEndpoints) -> Self {
        Self {
            endpoints,
            http: reqwest::Client::new(),
        }
    }

    /// Establishes a WebRTC session with the manager using the provided bearer token.
    ///
    /// The returned [`FastPathConnection`] exposes the negotiated data channels and
    /// a broadcast stream for manager-issued `ActionCommand`s. Callers are expected to
    /// consume actions, apply them locally, and acknowledge via `send_acks`. Harnesses
    /// can stream incremental state via `send_state`.
    pub async fn connect(&self, bearer_token: &str) -> HarnessResult<FastPathConnection> {
        let api = APIBuilder::new().build();
        let pc = Arc::new(
            api.new_peer_connection(RTCConfiguration::default())
                .await
                .map_err(to_harness_error)?,
        );

        let (actions_tx, _) = broadcast::channel(64);
        let actions_dc = create_channel(
            pc.clone(),
            &self.endpoints.channels.actions,
            ChannelReliability::Reliable,
        )
        .await?;
        let acks_dc = create_channel(
            pc.clone(),
            &self.endpoints.channels.acks,
            ChannelReliability::Reliable,
        )
        .await?;
        let state_dc = create_channel(
            pc.clone(),
            &self.endpoints.channels.state,
            ChannelReliability::Unreliable,
        )
        .await?;

        wire_action_handler(actions_dc.clone(), actions_tx.clone());

        pc.on_peer_connection_state_change(Box::new(move |state: RTCPeerConnectionState| {
            Box::pin(async move {
                info!(target = "fast_path", state = ?state, "peer connection state changed");
            })
        }));

        let http = self.http.clone();
        let post_ice_url = self.endpoints.ice_url.clone();
        let token = bearer_token.to_string();
        pc.on_ice_candidate(Box::new(move |candidate| {
            let http = http.clone();
            let url = post_ice_url.clone();
            let token = token.clone();
            Box::pin(async move {
                if let Some(cand) = candidate {
                    if let Ok(json) = cand.to_json() {
                        trace!(
                            target = "fast_path.ice",
                            candidate = %json.candidate,
                            sdp_mid = json.sdp_mid.as_deref(),
                            sdp_mline_index = json.sdp_mline_index,
                            "local ICE candidate gathered"
                        );
                        let body = IcePostBody {
                            candidate: json.candidate,
                            sdp_mid: json.sdp_mid,
                            sdp_mline_index: json.sdp_mline_index,
                        };
                        if let Err(error) = http
                            .post(url.clone())
                            .bearer_auth(&token)
                            .json(&body)
                            .send()
                            .await
                        {
                            warn!(
                                target = "fast_path",
                                error = %error,
                                "failed to post local ICE candidate"
                            );
                        } else {
                            trace!(
                                target = "fast_path.ice",
                                "posted local ICE candidate to manager"
                            );
                        }
                    }
                }
            })
        }));

        let offer = pc.create_offer(None).await.map_err(to_harness_error)?;
        pc.set_local_description(offer.clone())
            .await
            .map_err(to_harness_error)?;

        let answer = self
            .http
            .post(self.endpoints.offer_url.clone())
            .bearer_auth(bearer_token)
            .json(&OfferRequest {
                sdp: offer.sdp,
                r#type: "offer".into(),
            })
            .send()
            .await
            .map_err(to_transport_error)?
            .error_for_status()
            .map_err(|err| HarnessError::Transport(format!("offer failed: {err}")))?
            .json::<OfferResponse>()
            .await
            .map_err(to_transport_error)?;

        let answer_desc =
            RTCSessionDescription::answer(answer.sdp.clone()).map_err(to_harness_error)?;
        pc.set_remote_description(answer_desc)
            .await
            .map_err(to_harness_error)?;

        gather_remote_ice(
            &self.http,
            &self.endpoints.ice_url,
            bearer_token,
            pc.clone(),
        )
        .await?;

        Ok(FastPathConnection {
            peer: pc,
            channels: self.endpoints.channels.clone(),
            actions: actions_tx,
            acks_dc,
            state_dc,
        })
    }
}

#[derive(Clone)]
pub struct FastPathConnection {
    pub peer: Arc<RTCPeerConnection>,
    channels: FastPathChannels,
    actions: broadcast::Sender<ActionCommand>,
    acks_dc: Arc<RTCDataChannel>,
    state_dc: Arc<RTCDataChannel>,
}

impl FastPathConnection {
    pub fn subscribe_actions(&self) -> broadcast::Receiver<ActionCommand> {
        self.actions.subscribe()
    }

    pub fn channels(&self) -> &FastPathChannels {
        &self.channels
    }

    pub async fn send_acks(&self, acks: &[ActionAck]) -> HarnessResult<()> {
        for ack in acks {
            let payload = serde_json::json!({
                "type": "ack",
                "payload": ack,
            });
            let text = serde_json::to_string(&payload)
                .map_err(|err| HarnessError::Transport(format!("serialize ack failed: {err}")))?;
            let frames = frame_fast_path_payload(FastPathPayloadKind::Acks, &text)?;
            for frame in frames {
                self.acks_dc
                    .send_text(frame)
                    .await
                    .map_err(to_harness_error)?;
            }
        }
        Ok(())
    }

    pub async fn send_state(&self, diff: &StateDiff) -> HarnessResult<()> {
        let payload = serde_json::json!({
            "type": "state",
            "payload": diff,
        });
        let text = serde_json::to_string(&payload)
            .map_err(|err| HarnessError::Transport(format!("serialize state failed: {err}")))?;
        let frames = frame_fast_path_payload(FastPathPayloadKind::State, &text)?;
        for frame in frames {
            self.state_dc
                .send_text(frame)
                .await
                .map_err(to_harness_error)?;
        }
        Ok(())
    }
}

async fn gather_remote_ice(
    http: &reqwest::Client,
    url: &Url,
    bearer_token: &str,
    pc: Arc<RTCPeerConnection>,
) -> HarnessResult<()> {
    for attempt in 0..5 {
        let response = http
            .get(url.clone())
            .bearer_auth(bearer_token)
            .send()
            .await
            .map_err(to_transport_error)?;
        if response.status() == StatusCode::NOT_FOUND {
            sleep(Duration::from_millis(200)).await;
            continue;
        }
        let body = response
            .json::<IceListResponse>()
            .await
            .map_err(to_transport_error)?;
        let mut added = 0usize;
        for cand in body.candidates.into_iter() {
            let init = RTCIceCandidateInit {
                candidate: cand.candidate,
                sdp_mid: cand.sdp_mid,
                sdp_mline_index: cand.sdp_mline_index,
                ..Default::default()
            };
            trace!(
                target = "fast_path.ice",
                candidate = %init.candidate,
                sdp_mid = init.sdp_mid.as_deref(),
                sdp_mline_index = init.sdp_mline_index,
                "applying remote ICE candidate from manager"
            );
            pc.add_ice_candidate(init).await.map_err(to_harness_error)?;
            added += 1;
        }
        if added == 0 {
            // Give ICE gathering a moment to converge; bail once stable.
            if attempt >= 2 {
                break;
            }
            sleep(Duration::from_millis(150)).await;
        }
    }
    Ok(())
}

fn wire_action_handler(dc: Arc<RTCDataChannel>, sender: broadcast::Sender<ActionCommand>) {
    let open_label = dc.label().to_string();
    dc.on_open(Box::new(move || {
        let label = open_label.clone();
        Box::pin(async move {
            info!(target = "fast_path", channel = %label, "actions channel open");
        })
    }));

    let close_label = dc.label().to_string();
    dc.on_close(Box::new(move || {
        let label = close_label.clone();
        Box::pin(async move {
            info!(target = "fast_path", channel = %label, "actions channel closed");
        })
    }));

    let error_label = dc.label().to_string();
    dc.on_error(Box::new(move |err| {
        let label = error_label.clone();
        Box::pin(async move {
            warn!(
                target = "fast_path",
                channel = %label,
                error = %err,
                "actions channel error"
            );
        })
    }));

    let reassembler = Arc::new(TokioMutex::new(FastPathChunkReassembler::new(
        FastPathPayloadKind::Actions,
    )));
    dc.on_message(Box::new(move |msg: DataChannelMessage| {
        let sender = sender.clone();
        let reassembler = reassembler.clone();
        Box::pin(async move {
            if msg.is_string {
                match decode_fast_path_payload(&reassembler, &msg).await {
                    Ok(Some(text)) => match parse_action_payload(&text) {
                        Ok(action) => {
                            let _ = sender.send(action);
                        }
                        Err(err) => {
                            warn!(target = "fast_path", error = %err, "failed to parse action message");
                        }
                    },
                    Ok(None) => {
                        // awaiting more chunks
                    }
                    Err(err) => {
                        warn!(
                            target = "fast_path",
                            error = %err,
                            "failed to decode chunked action message"
                        );
                    }
                }
            } else {
                match decode_binary_action_message(&msg.data) {
                    Ok(action) => {
                        let _ = sender.send(action);
                    }
                    Err(err) => {
                        warn!(
                            target = "fast_path",
                            error = %err,
                            "failed to decode binary action message"
                        );
                    }
                }
            }
        })
    }));
}

const WIRE_PROTOCOL_VERSION: u8 = 2;
const CLIENT_KIND_INPUT: u8 = 0;

fn decode_binary_action_message(data: &[u8]) -> HarnessResult<ActionCommand> {
    let mut cursor = BinaryCursor::new(data);
    let header = cursor.read_u8().map_err(|err| {
        HarnessError::Transport(format!("failed to read fast-path header: {err}"))
    })?;
    let version = header >> 5;
    if version != WIRE_PROTOCOL_VERSION {
        return Err(HarnessError::Transport(format!(
            "unsupported fast-path frame version {version}"
        )));
    }
    let frame_type = header & 0x1f;
    if frame_type != CLIENT_KIND_INPUT {
        return Err(HarnessError::Transport(format!(
            "unexpected fast-path frame type {frame_type}"
        )));
    }
    let seq = cursor.read_var_u64()?;
    let len = cursor.read_var_u32()? as usize;
    let payload = cursor.read_bytes(len)?.to_vec();
    let text = String::from_utf8_lossy(&payload).to_string();
    Ok(ActionCommand {
        id: format!("fastpath-seq-{seq}"),
        action_type: "terminal_write".into(),
        payload: json!({ "bytes": text }),
        expires_at: None,
    })
}

struct BinaryCursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> BinaryCursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, &'static str> {
        if self.pos >= self.buf.len() {
            return Err("unexpected end of fast-path frame");
        }
        let byte = self.buf[self.pos];
        self.pos += 1;
        Ok(byte)
    }

    fn read_var_u64(&mut self) -> HarnessResult<u64> {
        self.read_var_int::<u64>()
    }

    fn read_var_u32(&mut self) -> HarnessResult<u32> {
        self.read_var_int::<u32>()
    }

    fn read_var_int<T>(&mut self) -> HarnessResult<T>
    where
        T: TryFrom<u64> + Default,
    {
        let mut value: u64 = 0;
        let mut shift = 0u32;
        loop {
            let byte = self
                .read_u8()
                .map_err(|err| HarnessError::Transport(err.into()))?;
            let slice = (byte & 0x7f) as u64;
            value |= slice << shift;
            if (byte & 0x80) == 0 {
                return T::try_from(value).map_err(|_| {
                    HarnessError::Transport("varint overflow in fast-path frame".into())
                });
            }
            shift += 7;
            if shift > 63 {
                return Err(HarnessError::Transport(
                    "varint overflow in fast-path frame".into(),
                ));
            }
        }
    }

    fn read_bytes(&mut self, len: usize) -> Result<&'a [u8], HarnessError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| HarnessError::Transport("fast-path frame length overflow".into()))?;
        if end > self.buf.len() {
            return Err(HarnessError::Transport(
                "fast-path frame truncated before payload end".into(),
            ));
        }
        let slice = &self.buf[self.pos..end];
        self.pos = end;
        Ok(slice)
    }
}

async fn decode_fast_path_payload(
    reassembler: &TokioMutex<FastPathChunkReassembler>,
    msg: &DataChannelMessage,
) -> HarnessResult<Option<String>> {
    if !msg.is_string {
        return Err(HarnessError::Transport(
            "expected text payload for action message".into(),
        ));
    }
    let text = String::from_utf8(msg.data.to_vec())
        .map_err(|err| HarnessError::Transport(format!("invalid utf8 payload: {err}")))?;
    let mut guard = reassembler.lock().await;
    guard.ingest(&text)
}

async fn create_channel(
    pc: Arc<RTCPeerConnection>,
    label: &str,
    reliability: ChannelReliability,
) -> HarnessResult<Arc<RTCDataChannel>> {
    let label_string = Arc::new(label.to_string());
    let init = reliability.into();
    let dc = pc
        .create_data_channel(label, Some(init))
        .await
        .map_err(to_harness_error)?;

    {
        let label = label_string.clone();
        dc.on_open(Box::new(move || {
            let label = label.clone();
            Box::pin(async move {
                info!(target = "fast_path", channel = %label, "data channel created");
            })
        }));
    }

    {
        let label = label_string.clone();
        dc.on_close(Box::new(move || {
            let label = label.clone();
            Box::pin(async move {
                info!(target = "fast_path", channel = %label, "data channel closed");
            })
        }));
    }

    Ok(dc)
}

fn parse_action_message(msg: &DataChannelMessage) -> HarnessResult<ActionCommand> {
    if !msg.is_string {
        return Err(HarnessError::Transport(
            "expected text payload for action message".into(),
        ));
    }
    let text = String::from_utf8(msg.data.to_vec())
        .map_err(|err| HarnessError::Transport(format!("invalid utf8 payload: {err}")))?;
    parse_action_payload(&text)
}

fn parse_action_payload(text: &str) -> HarnessResult<ActionCommand> {
    let envelope: ActionEnvelope =
        serde_json::from_str(text).map_err(|err| HarnessError::Transport(format!("{err}")))?;
    if envelope.r#type != "action" {
        return Err(HarnessError::Transport(format!(
            "unexpected message type {}",
            envelope.r#type
        )));
    }
    serde_json::from_value(envelope.payload)
        .map_err(|err| HarnessError::Transport(format!("invalid action payload: {err}")))
}

fn to_harness_error(err: impl std::error::Error) -> HarnessError {
    HarnessError::Transport(err.to_string())
}

fn to_transport_error(err: reqwest::Error) -> HarnessError {
    HarnessError::Transport(err.to_string())
}

#[derive(Debug, Deserialize)]
struct OfferResponse {
    pub sdp: String,
}

#[derive(Debug, Serialize)]
struct OfferRequest {
    pub sdp: String,
    pub r#type: String,
}

#[derive(Debug, Serialize)]
struct IcePostBody {
    candidate: String,
    sdp_mid: Option<String>,
    sdp_mline_index: Option<u16>,
}

#[derive(Debug, Deserialize)]
struct IceListResponse {
    candidates: Vec<IceCandidateRecord>,
}

#[derive(Debug, Deserialize)]
struct IceCandidateRecord {
    candidate: String,
    #[serde(default)]
    sdp_mid: Option<String>,
    #[serde(default)]
    sdp_mline_index: Option<u16>,
}

#[derive(Clone, Copy, Debug)]
enum ChannelReliability {
    Reliable,
    Unreliable,
}

impl From<ChannelReliability> for RTCDataChannelInit {
    fn from(value: ChannelReliability) -> Self {
        match value {
            ChannelReliability::Reliable => RTCDataChannelInit {
                ordered: Some(true),
                ..Default::default()
            },
            ChannelReliability::Unreliable => RTCDataChannelInit {
                ordered: Some(false),
                max_retransmits: Some(0),
                ..Default::default()
            },
        }
    }
}

#[derive(Debug, Deserialize)]
struct ActionEnvelope {
    #[serde(rename = "type")]
    r#type: String,
    payload: Value,
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytes::Bytes;

    #[test]
    fn parses_basic_hint() {
        let base = Url::parse("https://manager.local/").unwrap();
        let mut hints = HashMap::new();
        hints.insert(
            "fast_path_webrtc".into(),
            serde_json::json!({
                "offer_path": "/fastpath/sessions/abc/webrtc/offer",
                "ice_path": "/fastpath/sessions/abc/webrtc/ice",
                "channels": {
                    "actions": "mgr-actions",
                    "acks": "mgr-acks",
                    "state": "mgr-state"
                },
                "status": "experimental"
            }),
        );

        let parsed = parse_fast_path_endpoints(&base, &hints)
            .expect("parse ok")
            .expect("hint present");
        assert_eq!(
            parsed.offer_url.as_str(),
            "https://manager.local/fastpath/sessions/abc/webrtc/offer"
        );
        assert_eq!(
            parsed.ice_url.as_str(),
            "https://manager.local/fastpath/sessions/abc/webrtc/ice"
        );
        assert_eq!(parsed.status, FastPathStatus::Experimental);
        assert_eq!(parsed.channels.actions, "mgr-actions");
    }

    #[test]
    fn missing_hint_returns_none() {
        let base = Url::parse("https://manager.local/").unwrap();
        let hints = HashMap::<String, Value>::new();
        assert!(parse_fast_path_endpoints(&base, &hints).unwrap().is_none());
    }

    #[test]
    fn invalid_hint_shape_errors() {
        let base = Url::parse("https://manager.local/").unwrap();
        let mut hints = HashMap::new();
        hints.insert("fast_path_webrtc".into(), serde_json::json!(42));

        let err = parse_fast_path_endpoints(&base, &hints).unwrap_err();
        assert!(matches!(err, HarnessError::Transport(_)));
    }

    #[test]
    fn parses_action_message() {
        let envelope = serde_json::json!({
            "type": "action",
            "payload": {
                "id": "123",
                "action_type": "terminal_write",
                "payload": serde_json::json!({"data": "ping"}),
                "expires_at": null
            }
        });
        let msg = DataChannelMessage {
            is_string: true,
            data: Bytes::from(serde_json::to_string(&envelope).unwrap()),
        };
        let action = parse_action_message(&msg).expect("parsed action");
        assert_eq!(action.id, "123");
    }

    #[test]
    fn rejects_non_action_message() {
        let envelope = serde_json::json!({
            "type": "ack",
            "payload": {}
        });
        let msg = DataChannelMessage {
            is_string: true,
            data: Bytes::from(serde_json::to_string(&envelope).unwrap()),
        };
        assert!(parse_action_message(&msg).is_err());
    }

    #[test]
    fn rejects_binary_action_message() {
        let msg = DataChannelMessage {
            is_string: false,
            data: Bytes::from_static(b"\x01\x02\x03"),
        };
        assert!(parse_action_message(&msg).is_err());
    }

    #[test]
    fn frame_payload_passthrough_when_small() {
        let frames =
            frame_fast_path_payload(FastPathPayloadKind::Actions, "hello").expect("framed");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0], "hello");
    }

    #[test]
    fn chunk_round_trip_reassembles() {
        let payload = "x".repeat(FAST_PATH_CHUNK_PAYLOAD_BYTES * 2 + 128);
        let frames = frame_fast_path_payload(FastPathPayloadKind::State, &payload).expect("framed");
        assert!(frames.len() > 1, "payload should be chunked");
        let mut reassembler = FastPathChunkReassembler::new(FastPathPayloadKind::State);
        let mut assembled = None;
        for frame in frames {
            let result = reassembler.ingest(&frame).expect("ingest chunk");
            if result.is_some() {
                assembled = result;
            }
        }
        assert_eq!(assembled.expect("assembled payload"), payload);
    }

    #[test]
    fn chunk_round_trip_actions() {
        let payload = "y".repeat(FAST_PATH_CHUNK_PAYLOAD_BYTES + 256);
        let frames =
            frame_fast_path_payload(FastPathPayloadKind::Actions, &payload).expect("framed");
        assert!(frames.len() > 1);
        let mut reassembler = FastPathChunkReassembler::new(FastPathPayloadKind::Actions);
        let mut assembled = None;
        for frame in frames {
            if let Some(text) = reassembler.ingest(&frame).expect("ingest chunk") {
                assembled = Some(text);
            }
        }
        assert_eq!(assembled.expect("assembled payload"), payload);
    }

    #[test]
    fn non_chunk_payload_passes_through() {
        let mut reassembler = FastPathChunkReassembler::new(FastPathPayloadKind::State);
        let payload = r#"{"type":"state","payload":{"ops":[]}}"#.to_string();
        let result = reassembler.ingest(&payload).expect("ingest");
        assert_eq!(result.expect("pass through"), payload);
    }
}
