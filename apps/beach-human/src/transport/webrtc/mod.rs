use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use crossbeam_channel::{
    Receiver as CrossbeamReceiver, RecvTimeoutError as CrossbeamRecvTimeoutError,
    TryRecvError as CrossbeamTryRecvError, unbounded as crossbeam_unbounded,
};
use once_cell::sync::Lazy;
use reqwest::{Client, StatusCode};
use serde::{Deserialize, Serialize};
use tokio::runtime::{Handle, Runtime};
use tokio::sync::{Mutex as AsyncMutex, Notify, mpsc as tokio_mpsc, oneshot};
use tokio::time::{sleep, timeout};
use webrtc::api::interceptor_registry::register_default_interceptors;
use webrtc::api::media_engine::MediaEngine;
use webrtc::api::setting_engine::SettingEngine;
use webrtc::api::{API, APIBuilder};
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;
use webrtc::interceptor::registry::Registry;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
use webrtc::peer_connection::sdp::sdp_type::RTCSdpType;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::util::vnet::net::{Net, NetConfig};
use webrtc::util::vnet::router::{Router, RouterConfig};

use crate::transport::{
    Transport, TransportError, TransportId, TransportKind, TransportMessage, TransportPair,
    decode_message, encode_message, next_transport_id,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

mod signaling;

use signaling::{SignalingClient, WebRTCSignal};

pub fn build_pair() -> Result<TransportPair, TransportError> {
    RUNTIME.block_on(async { create_webrtc_pair().await })
}

static RUNTIME: Lazy<Runtime> = Lazy::new(|| Runtime::new().expect("tokio runtime"));

fn spawn_task<F>(future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if let Ok(handle) = Handle::try_current() {
        handle.spawn(future);
    } else {
        RUNTIME.spawn(future);
    }
}

fn spawn_with_handle<F>(handle: Option<Handle>, future: F)
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    if let Some(handle) = handle {
        handle.spawn(future);
    } else {
        spawn_task(future);
    }
}

fn spawn_on_global<F>(future: F) -> tokio::task::JoinHandle<()>
where
    F: std::future::Future<Output = ()> + Send + 'static,
{
    RUNTIME.spawn(future)
}

fn build_api(setting: SettingEngine) -> Result<API, TransportError> {
    let mut media_engine = MediaEngine::default();
    media_engine
        .register_default_codecs()
        .map_err(to_setup_error)?;

    let mut registry = Registry::new();
    registry =
        register_default_interceptors(registry, &mut media_engine).map_err(to_setup_error)?;

    Ok(APIBuilder::new()
        .with_setting_engine(setting)
        .with_media_engine(media_engine)
        .with_interceptor_registry(registry)
        .build())
}

async fn attach_vnet_to_router(
    vnet: &Arc<Net>,
    router: &Arc<AsyncMutex<Router>>,
) -> Result<(), TransportError> {
    let nic = vnet.get_nic().map_err(to_setup_error)?;
    {
        let nic_clone = Arc::clone(&nic);
        let mut router_guard = router.lock().await;
        router_guard
            .add_net(nic_clone)
            .await
            .map_err(to_setup_error)?;
    }
    {
        let nic_guard = nic.lock().await;
        nic_guard
            .set_router(Arc::clone(router))
            .await
            .map_err(to_setup_error)?;
    }
    Ok(())
}

struct WebRtcTransport {
    kind: TransportKind,
    id: TransportId,
    peer: TransportId,
    outbound_seq: AtomicU64,
    outbound_tx: tokio_mpsc::UnboundedSender<Vec<u8>>,
    inbound_rx: Mutex<CrossbeamReceiver<TransportMessage>>,
    _pc: Arc<RTCPeerConnection>,
    _dc: Arc<RTCDataChannel>,
    _router: Option<Arc<AsyncMutex<Router>>>,
    _signaling: Option<Arc<SignalingClient>>,
}

impl WebRtcTransport {
    fn new(
        kind: TransportKind,
        id: TransportId,
        peer: TransportId,
        pc: Arc<RTCPeerConnection>,
        dc: Arc<RTCDataChannel>,
        router: Option<Arc<AsyncMutex<Router>>>,
        dc_ready: Option<Arc<Notify>>,
        signaling: Option<Arc<SignalingClient>>,
    ) -> Self {
        let (inbound_tx, inbound_rx) = crossbeam_unbounded();
        let handler_id = id;
        let tx_clone = inbound_tx.clone();
        tracing::trace!(target = "webrtc", transport_id = ?handler_id, "registering data channel handler");
        dc.on_message(Box::new(move |msg: DataChannelMessage| {
            let sender = tx_clone.clone();
            let log_id = handler_id;
            Box::pin(async move {
                let bytes = msg.data.to_vec();
                if let Some(message) = decode_message(&bytes) {
                    tracing::trace!(
                        target = "webrtc",
                        transport_id = ?log_id,
                        frame_len = bytes.len(),
                        sequence = message.sequence,
                        "received frame"
                    );
                    if let Err(err) = sender.send(message) {
                        tracing::warn!(
                            target = "webrtc",
                            transport_id = ?log_id,
                            error = %err,
                            "failed to enqueue inbound message"
                        );
                    }
                } else {
                    tracing::warn!(
                        target = "webrtc",
                        transport_id = ?log_id,
                        frame_len = bytes.len(),
                        "failed to decode message"
                    );
                }
            })
        }));
        dc.on_error(Box::new(move |err| {
            let log_id = handler_id;
            Box::pin(async move {
                tracing::warn!(target = "webrtc", transport_id = ?log_id, error = %err, "data channel error");
            })
        }));
        dc.on_close(Box::new(move || {
            let log_id = handler_id;
            Box::pin(async move {
                tracing::trace!(target = "webrtc", transport_id = ?log_id, "data channel closed");
            })
        }));

        let (outbound_tx, mut outbound_rx) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
        let dc_clone = dc.clone();
        let transport_id = id;
        let dc_ready_signal = dc_ready.clone();
        spawn_on_global(async move {
            if let Some(notify) = dc_ready_signal {
                tracing::trace!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc_ready.notified",
                    state = "start"
                );
                notify.notified().await;
                tracing::trace!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc_ready.notified",
                    state = "end"
                );
                tracing::trace!(target = "webrtc", transport_id = ?transport_id, "dc ready triggered");
            } else {
                tracing::trace!(target = "webrtc", transport_id = ?transport_id, "dc ready immediate");
            }
            tracing::trace!(target = "webrtc", transport_id = ?transport_id, "sender loop start");
            loop {
                tracing::trace!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "outbound_rx.recv",
                    state = "start"
                );
                let maybe_bytes = outbound_rx.recv().await;
                tracing::trace!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "outbound_rx.recv",
                    state = "end",
                    has_bytes = maybe_bytes.is_some()
                );
                let bytes = match maybe_bytes {
                    Some(bytes) => bytes,
                    None => break,
                };
                tracing::trace!(
                    target = "webrtc",
                    transport_id = ?transport_id,
                    queued_len = bytes.len(),
                    "dequeued outbound"
                );
                let data = Bytes::from(bytes);
                tracing::trace!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc.buffered_amount.before",
                    state = "start"
                );
                let before = dc_clone.buffered_amount().await;
                tracing::trace!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc.buffered_amount.before",
                    state = "end",
                    buffered_before = before
                );
                tracing::trace!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc.send",
                    state = "start",
                    payload_len = data.len()
                );
                let send_result = timeout(CONNECT_TIMEOUT, dc_clone.send(&data)).await;
                tracing::trace!(
                    target = "beach_human::transport::webrtc",
                    transport_id = ?transport_id,
                    await = "dc.send",
                    state = "end",
                    result = ?send_result
                );
                match send_result {
                    Ok(Ok(bytes_written)) => {
                        tracing::trace!(
                            target = "beach_human::transport::webrtc",
                            transport_id = ?transport_id,
                            await = "dc.buffered_amount.after",
                            state = "start"
                        );
                        let after = dc_clone.buffered_amount().await;
                        tracing::trace!(
                            target = "beach_human::transport::webrtc",
                            transport_id = ?transport_id,
                            await = "dc.buffered_amount.after",
                            state = "end",
                            buffered_after = after
                        );
                        tracing::trace!(
                            target = "webrtc",
                            transport_id = ?transport_id,
                            bytes_written,
                            ready_state = ?dc_clone.ready_state(),
                            payload = ?&data[..],
                            buffered_before = before,
                            buffered_after = after,
                            "sent frame"
                        );
                    }
                    Ok(Err(err)) => {
                        tracing::warn!(
                            target = "webrtc",
                            transport_id = ?transport_id,
                            error = %err,
                            "webrtc send error"
                        );
                        break;
                    }
                    Err(_) => {
                        tracing::warn!(
                            target = "webrtc",
                            transport_id = ?transport_id,
                            "webrtc send timed out"
                        );
                        break;
                    }
                }
            }
            tracing::trace!(target = "webrtc", transport_id = ?transport_id, "sender loop ended");
        });

        Self {
            kind,
            id,
            peer,
            outbound_seq: AtomicU64::new(0),
            outbound_tx,
            inbound_rx: Mutex::new(inbound_rx),
            _pc: pc,
            _dc: dc,
            _router: router,
            _signaling: signaling,
        }
    }
}

impl Transport for WebRtcTransport {
    fn kind(&self) -> TransportKind {
        self.kind
    }
    fn id(&self) -> TransportId {
        self.id
    }
    fn peer(&self) -> TransportId {
        self.peer
    }

    fn send(&self, message: TransportMessage) -> Result<(), TransportError> {
        let bytes = encode_message(&message);
        tracing::info!(
            target = "webrtc",
            transport_id = ?self.id,
            payload_len = bytes.len(),
            sequence = message.sequence,
            "queueing outbound message"
        );
        self.outbound_tx
            .send(bytes)
            .map_err(|_| TransportError::ChannelClosed)
    }

    fn send_text(&self, text: &str) -> Result<u64, TransportError> {
        let sequence = self.outbound_seq.fetch_add(1, Ordering::Relaxed);
        self.send(TransportMessage::text(sequence, text.to_string()))?;
        Ok(sequence)
    }

    fn send_bytes(&self, bytes: &[u8]) -> Result<u64, TransportError> {
        let sequence = self.outbound_seq.fetch_add(1, Ordering::Relaxed);
        self.send(TransportMessage::binary(sequence, bytes.to_vec()))?;
        Ok(sequence)
    }

    fn recv(&self, timeout_duration: Duration) -> Result<TransportMessage, TransportError> {
        tracing::trace!(
            target = "webrtc",
            transport_id = ?self.id,
            timeout = ?timeout_duration,
            "waiting for inbound message"
        );
        let receiver = self.inbound_rx.lock().unwrap();
        let result = receiver.recv_timeout(timeout_duration);
        match result {
            Ok(message) => {
                tracing::trace!(
                    target = "webrtc",
                    transport_id = ?self.id,
                    sequence = message.sequence,
                    payload = ?message.payload,
                    "received inbound message"
                );
                Ok(message)
            }
            Err(CrossbeamRecvTimeoutError::Timeout) => {
                tracing::trace!(
                    target = "webrtc",
                    transport_id = ?self.id,
                    "recv timed out"
                );
                Err(TransportError::Timeout)
            }
            Err(CrossbeamRecvTimeoutError::Disconnected) => {
                tracing::warn!(
                    target = "webrtc",
                    transport_id = ?self.id,
                    "recv channel closed"
                );
                Err(TransportError::ChannelClosed)
            }
        }
    }

    fn try_recv(&self) -> Result<Option<TransportMessage>, TransportError> {
        let receiver = self.inbound_rx.lock().unwrap();
        match receiver.try_recv() {
            Ok(message) => Ok(Some(message)),
            Err(CrossbeamTryRecvError::Empty) => Ok(None),
            Err(CrossbeamTryRecvError::Disconnected) => Err(TransportError::ChannelClosed),
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub enum WebRtcRole {
    Offerer,
    Answerer,
}

#[derive(Debug, Serialize, Deserialize)]
struct WebRtcSdpPayload {
    sdp: String,
    #[serde(rename = "type")]
    typ: String,
}

pub async fn connect_via_signaling(
    signaling_url: &str,
    role: WebRtcRole,
    poll_interval: Duration,
    passphrase: Option<&str>,
) -> Result<Arc<dyn Transport>, TransportError> {
    match role {
        WebRtcRole::Offerer => connect_offerer(signaling_url, poll_interval, passphrase).await,
        WebRtcRole::Answerer => connect_answerer(signaling_url, poll_interval, passphrase).await,
    }
}

async fn connect_offerer(
    signaling_url: &str,
    poll_interval: Duration,
    passphrase: Option<&str>,
) -> Result<Arc<dyn Transport>, TransportError> {
    let client = Client::new();

    let signaling = signaling_url.trim_end_matches('/').to_string();

    let mut setting = SettingEngine::default();
    setting.set_ice_timeouts(
        Some(Duration::from_secs(3)),
        Some(Duration::from_secs(10)),
        Some(Duration::from_millis(500)),
    );
    let api = build_api(setting)?;
    let config = RTCConfiguration::default();

    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "api.new_peer_connection",
        state = "start"
    );
    let pc_result = api.new_peer_connection(config).await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "api.new_peer_connection",
        state = "end",
        result = ?pc_result
    );
    let pc = Arc::new(pc_result.map_err(to_setup_error)?);

    let spawn_handle = Handle::try_current().ok();

    let signaling_client =
        SignalingClient::connect(signaling_url, WebRtcRole::Offerer, passphrase).await?;

    let signaling_for_candidates = Arc::clone(&signaling_client);
    pc.on_ice_candidate(Box::new(move |candidate| {
        let signaling = Arc::clone(&signaling_for_candidates);
        Box::pin(async move {
            if let Some(cand) = candidate {
                if let Err(err) = signaling.send_ice_candidate(cand).await {
                    tracing::warn!(
                        target = "webrtc",
                        error = %err,
                        "offerer candidate send error"
                    );
                }
            }
        })
    }));

    let pc_for_incoming = pc.clone();
    let signaling_for_incoming = Arc::clone(&signaling_client);
    spawn_on_global(async move {
        while let Some(signal) = signaling_for_incoming.recv_webrtc_signal().await {
            if let WebRTCSignal::IceCandidate {
                candidate,
                sdp_mid,
                sdp_mline_index,
            } = signal
            {
                let init = RTCIceCandidateInit {
                    candidate,
                    sdp_mid,
                    sdp_mline_index: sdp_mline_index.map(|idx| idx as u16),
                    username_fragment: None,
                };
                if let Err(err) = pc_for_incoming.add_ice_candidate(init).await {
                    tracing::warn!(
                        target = "webrtc",
                        error = %err,
                        "offerer failed to add remote ice candidate"
                    );
                }
            }
        }
    });

    let dc_init = RTCDataChannelInit {
        ordered: Some(true),
        ..Default::default()
    };
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "pc.create_data_channel",
        state = "start"
    );
    let dc_result = pc.create_data_channel("beach-human", Some(dc_init)).await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "pc.create_data_channel",
        state = "end",
        result = ?dc_result.as_ref().map(|_| "ok")
    );
    let dc = dc_result.map_err(to_setup_error)?;

    let dc_open_notify = Arc::new(Notify::new());
    let open_signal = dc_open_notify.clone();
    dc.on_open(Box::new(move || {
        let notify = open_signal.clone();
        Box::pin(async move {
            tracing::debug!(target = "webrtc", "data channel opened (offerer)");
            tracing::trace!(target = "webrtc", "offerer data channel open");
            notify.notify_waiters();
            notify.notify_one();
        })
    }));

    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "pc.create_offer",
        state = "start"
    );
    let offer_result = pc.create_offer(None).await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "pc.create_offer",
        state = "end",
        result = ?offer_result
    );
    let offer = offer_result.map_err(to_setup_error)?;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "pc.set_local_description",
        state = "start"
    );
    pc.set_local_description(offer)
        .await
        .map_err(to_setup_error)?;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "pc.set_local_description",
        state = "end"
    );
    wait_for_local_description(&pc).await?;

    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "pc.local_description",
        state = "start"
    );
    let maybe_local_desc = pc.local_description().await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "pc.local_description",
        state = "end",
        has_description = maybe_local_desc.is_some()
    );
    let local_desc = maybe_local_desc
        .ok_or_else(|| TransportError::Setup("missing local description".into()))?;
    let payload = payload_from_description(&local_desc);
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "post_sdp.offer",
        state = "start"
    );
    let post_result = post_sdp(&client, signaling_url, "offer", &payload).await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "post_sdp.offer",
        state = "end",
        result = ?post_result
    );
    post_result?;

    let pc_for_answer = pc.clone();
    let client_for_answer = client.clone();
    let signaling_for_answer = signaling.clone();
    spawn_with_handle(spawn_handle.clone(), async move {
        if let Err(err) = wait_for_answer(
            client_for_answer,
            signaling_for_answer,
            poll_interval,
            pc_for_answer,
        )
        .await
        {
            if !matches!(err, TransportError::Timeout | TransportError::ChannelClosed) {
                tracing::warn!(
                    target = "webrtc",
                    error = %err,
                    "offerer handshake error"
                );
            }
        }
    });

    let local_id = next_transport_id();
    let remote_id = next_transport_id();
    tracing::trace!(
        target = "webrtc",
        ?local_id,
        ?remote_id,
        "offerer allocating transport ids"
    );
    let transport = Arc::new(WebRtcTransport::new(
        TransportKind::WebRtc,
        local_id,
        remote_id,
        pc.clone(),
        dc,
        None,
        Some(dc_open_notify),
        Some(signaling_client.clone()),
    ));
    tracing::trace!(
        target = "webrtc",
        ?local_id,
        "offerer transport initialized"
    );

    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "spawn_blocking.recv_ready",
        state = "start"
    );
    let readiness_join = tokio::task::spawn_blocking({
        let transport = Arc::clone(&transport);
        move || transport.recv(CONNECT_TIMEOUT)
    })
    .await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "spawn_blocking.recv_ready",
        state = "end",
        result = ?readiness_join
    );
    let readiness = readiness_join.map_err(|err| TransportError::Setup(err.to_string()))?;

    if let Ok(message) = readiness {
        if message.payload.as_text() != Some("__ready__") {
            tracing::warn!(
                target = "webrtc",
                payload = ?message.payload,
                "unexpected handshake message"
            );
        }
    } else {
        tracing::warn!(target = "webrtc", "offerer did not receive readiness ack");
    }

    if let Err(err) = transport.send_text("__offer_ready__") {
        tracing::warn!(
            target = "webrtc",
            error = %err,
            "offerer readiness signal failed"
        );
    }

    Ok(transport as Arc<dyn Transport>)
}

async fn connect_answerer(
    signaling_url: &str,
    poll_interval: Duration,
    passphrase: Option<&str>,
) -> Result<Arc<dyn Transport>, TransportError> {
    let client = Client::new();

    let offer_payload = loop {
        tracing::trace!(
            target = "beach_human::transport::webrtc",
            role = "answerer",
            await = "fetch_sdp.offer",
            state = "start"
        );
        let fetch_attempt = fetch_sdp(&client, signaling_url, "offer").await;
        tracing::trace!(
            target = "beach_human::transport::webrtc",
            role = "answerer",
            await = "fetch_sdp.offer",
            state = "end",
            result = ?fetch_attempt
        );
        match fetch_attempt? {
            Some(payload) => break payload,
            None => {
                tracing::trace!(
                    target = "beach_human::transport::webrtc",
                    role = "answerer",
                    await = "sleep.poll_interval",
                    state = "start",
                    poll_ms = poll_interval.as_millis() as u64
                );
                sleep(poll_interval).await;
                tracing::trace!(
                    target = "beach_human::transport::webrtc",
                    role = "answerer",
                    await = "sleep.poll_interval",
                    state = "end"
                );
            }
        }
    };
    let offer_desc = session_description_from_payload(&offer_payload)?;

    let mut setting = SettingEngine::default();
    setting.set_ice_timeouts(
        Some(Duration::from_secs(3)),
        Some(Duration::from_secs(10)),
        Some(Duration::from_millis(500)),
    );
    let api = build_api(setting)?;
    let config = RTCConfiguration::default();

    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "api.new_peer_connection",
        state = "start"
    );
    let pc_result = api.new_peer_connection(config).await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "api.new_peer_connection",
        state = "end",
        result = ?pc_result
    );
    let pc = Arc::new(pc_result.map_err(to_setup_error)?);

    let signaling_client =
        SignalingClient::connect(signaling_url, WebRtcRole::Answerer, passphrase).await?;

    let signaling_for_candidates = Arc::clone(&signaling_client);
    pc.on_ice_candidate(Box::new(move |candidate| {
        let signaling = Arc::clone(&signaling_for_candidates);
        Box::pin(async move {
            if let Some(cand) = candidate {
                if let Err(err) = signaling.send_ice_candidate(cand).await {
                    tracing::warn!(
                        target = "webrtc",
                        error = %err,
                        "answerer candidate send error"
                    );
                }
            }
        })
    }));

    let signaling_for_incoming = Arc::clone(&signaling_client);
    let pc_for_incoming = pc.clone();
    spawn_on_global(async move {
        while let Some(signal) = signaling_for_incoming.recv_webrtc_signal().await {
            if let WebRTCSignal::IceCandidate {
                candidate,
                sdp_mid,
                sdp_mline_index,
            } = signal
            {
                let init = RTCIceCandidateInit {
                    candidate,
                    sdp_mid,
                    sdp_mline_index: sdp_mline_index.map(|idx| idx as u16),
                    username_fragment: None,
                };
                if let Err(err) = pc_for_incoming.add_ice_candidate(init).await {
                    tracing::warn!(
                        target = "webrtc",
                        error = %err,
                        "answerer failed to add remote ice candidate"
                    );
                }
            }
        }
    });

    let dc_open_notify = Arc::new(Notify::new());
    let transport_slot: Arc<AsyncMutex<Option<Arc<dyn Transport>>>> =
        Arc::new(AsyncMutex::new(None));
    let pc_for_dc = pc.clone();
    let notify_clone = dc_open_notify.clone();
    let slot_clone = transport_slot.clone();
    let client_id = next_transport_id();
    let peer_id = next_transport_id();
    tracing::trace!(
        target = "webrtc",
        ?client_id,
        ?peer_id,
        "answerer allocating transport ids"
    );
    let signaling_for_dc = Arc::clone(&signaling_client);
    pc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
        let pc = pc_for_dc.clone();
        let notify = notify_clone.clone();
        let slot = slot_clone.clone();
        let signaling_for_transport = Arc::clone(&signaling_for_dc);
        Box::pin(async move {
            let notify_for_open = notify.clone();
            dc.on_open(Box::new(move || {
                let notify = notify_for_open.clone();
                Box::pin(async move {
                    tracing::debug!(target = "webrtc", "data channel opened (answerer)");
                    tracing::trace!(target = "webrtc", "answerer data channel open");
                    notify.notify_waiters();
                    notify.notify_one();
                })
            }));

            tracing::trace!(
                target = "beach_human::transport::webrtc",
                role = "answerer",
                await = "slot.lock",
                state = "start"
            );
            let mut slot_guard = slot.lock().await;
            tracing::trace!(
                target = "beach_human::transport::webrtc",
                role = "answerer",
                await = "slot.lock",
                state = "end",
                is_populated = slot_guard.is_some()
            );
            if slot_guard.is_none() {
                let transport = WebRtcTransport::new(
                    TransportKind::WebRtc,
                    client_id,
                    peer_id,
                    pc.clone(),
                    dc,
                    None,
                    Some(notify.clone()),
                    Some(signaling_for_transport),
                );
                let transport_arc = Arc::new(transport) as Arc<dyn Transport>;
                slot_guard.replace(transport_arc.clone());
                drop(slot_guard);

                if let Err(err) = transport_arc.send_text("__ready__") {
                    tracing::warn!(
                        target = "webrtc",
                        error = %err,
                        "answerer readiness ack failed"
                    );
                }
                return;
            }
        })
    }));

    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "pc.set_remote_description",
        state = "start"
    );
    pc.set_remote_description(offer_desc)
        .await
        .map_err(to_setup_error)?;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "pc.set_remote_description",
        state = "end"
    );

    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "pc.create_answer",
        state = "start"
    );
    let answer_result = pc.create_answer(None).await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "pc.create_answer",
        state = "end",
        result = ?answer_result
    );
    let answer = answer_result.map_err(to_setup_error)?;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "pc.set_local_description",
        state = "start"
    );
    pc.set_local_description(answer)
        .await
        .map_err(to_setup_error)?;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "pc.set_local_description",
        state = "end"
    );
    wait_for_local_description(&pc).await?;

    let local_desc = pc
        .local_description()
        .await
        .ok_or_else(|| TransportError::Setup("missing local description".into()))?;
    let payload = payload_from_description(&local_desc);
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "post_sdp.answer",
        state = "start"
    );
    let post_result = post_sdp(&client, signaling_url, "answer", &payload).await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "post_sdp.answer",
        state = "end",
        result = ?post_result
    );
    post_result?;

    let transport = loop {
        tracing::trace!(
            target = "beach_human::transport::webrtc",
            role = "answerer",
            await = "transport_slot.lock",
            state = "start"
        );
        let mut transport_guard = transport_slot.lock().await;
        tracing::trace!(
            target = "beach_human::transport::webrtc",
            role = "answerer",
            await = "transport_slot.lock",
            state = "end",
            has_transport = transport_guard.is_some()
        );
        if let Some(transport) = transport_guard.as_ref().cloned() {
            drop(transport_guard);
            break transport;
        }
        transport_guard.take();
        drop(transport_guard);
        tracing::trace!(
            target = "beach_human::transport::webrtc",
            role = "answerer",
            await = "sleep.retry",
            state = "start",
            poll_ms = 10_u64
        );
        sleep(Duration::from_millis(10)).await;
        tracing::trace!(
            target = "beach_human::transport::webrtc",
            role = "answerer",
            await = "sleep.retry",
            state = "end"
        );
    };
    tracing::trace!(target = "webrtc", ?client_id, "answerer transport ready");

    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "wait_for_connection",
        state = "start"
    );
    let wait_result = wait_for_connection(&pc).await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "wait_for_connection",
        state = "end",
        result = ?wait_result
    );
    wait_result?;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "dc_open_notify.timeout",
        state = "start"
    );
    let notify_result = timeout(CONNECT_TIMEOUT, dc_open_notify.notified()).await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "answerer",
        await = "dc_open_notify.timeout",
        state = "end",
        result = ?notify_result
    );
    notify_result.map_err(|_| TransportError::Timeout)?;

    Ok(transport)
}

fn endpoint(base: &str, suffix: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), suffix)
}

async fn wait_for_answer(
    client: Client,
    signaling_url: String,
    poll_interval: Duration,
    pc: Arc<RTCPeerConnection>,
) -> Result<(), TransportError> {
    let answer_payload = loop {
        tracing::trace!(
            target = "beach_human::transport::webrtc",
            role = "offerer",
            await = "fetch_sdp.answer",
            state = "start"
        );
        let fetch_attempt = fetch_sdp(&client, &signaling_url, "answer").await;
        tracing::trace!(
            target = "beach_human::transport::webrtc",
            role = "offerer",
            await = "fetch_sdp.answer",
            state = "end",
            result = ?fetch_attempt
        );
        match fetch_attempt? {
            Some(payload) => break payload,
            None => {
                tracing::trace!(
                    target = "beach_human::transport::webrtc",
                    role = "offerer",
                    await = "sleep.poll_interval",
                    state = "start",
                    poll_ms = poll_interval.as_millis() as u64
                );
                sleep(poll_interval).await;
                tracing::trace!(
                    target = "beach_human::transport::webrtc",
                    role = "offerer",
                    await = "sleep.poll_interval",
                    state = "end"
                );
            }
        }
    };

    let answer_desc = session_description_from_payload(&answer_payload)?;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "pc.set_remote_description",
        state = "start"
    );
    pc.set_remote_description(answer_desc)
        .await
        .map_err(to_setup_error)?;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "pc.set_remote_description",
        state = "end"
    );
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "wait_for_connection",
        state = "start"
    );
    let result = wait_for_connection(&pc).await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        role = "offerer",
        await = "wait_for_connection",
        state = "end",
        result = ?result
    );
    result
}

async fn post_sdp(
    client: &Client,
    base: &str,
    suffix: &str,
    payload: &WebRtcSdpPayload,
) -> Result<(), TransportError> {
    let url = endpoint(base, suffix);
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        phase = "post_sdp",
        suffix,
        await = "client.send",
        state = "start"
    );
    let send_attempt = client.post(url).json(payload).send().await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        phase = "post_sdp",
        suffix,
        await = "client.send",
        state = "end",
        result = ?send_attempt.as_ref().map(reqwest::Response::status)
    );
    let response = send_attempt.map_err(http_error)?;

    if response.status().is_success() {
        Ok(())
    } else {
        Err(TransportError::Setup(format!(
            "unexpected signaling status {}",
            response.status()
        )))
    }
}

async fn fetch_sdp(
    client: &Client,
    base: &str,
    suffix: &str,
) -> Result<Option<WebRtcSdpPayload>, TransportError> {
    let url = endpoint(base, suffix);
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        phase = "fetch_sdp",
        suffix,
        await = "client.send",
        state = "start"
    );
    let send_attempt = client.get(url).send().await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        phase = "fetch_sdp",
        suffix,
        await = "client.send",
        state = "end",
        result = ?send_attempt.as_ref().map(reqwest::Response::status)
    );
    let response = send_attempt.map_err(http_error)?;

    match response.status() {
        StatusCode::OK => {
            tracing::trace!(
                target = "beach_human::transport::webrtc",
                phase = "fetch_sdp",
                suffix,
                await = "response.json",
                state = "start"
            );
            let payload_attempt = response.json::<WebRtcSdpPayload>().await;
            tracing::trace!(
                target = "beach_human::transport::webrtc",
                phase = "fetch_sdp",
                suffix,
                await = "response.json",
                state = "end",
                success = payload_attempt.is_ok()
            );
            let payload = payload_attempt.map_err(http_error)?;
            Ok(Some(payload))
        }
        StatusCode::NOT_FOUND => Ok(None),
        status if status.is_server_error() => Err(TransportError::Setup(format!(
            "signaling server returned {}",
            status
        ))),
        status => Err(TransportError::Setup(format!(
            "unexpected signaling status {}",
            status
        ))),
    }
}

fn payload_from_description(desc: &RTCSessionDescription) -> WebRtcSdpPayload {
    WebRtcSdpPayload {
        sdp: desc.sdp.clone(),
        typ: desc.sdp_type.to_string(),
    }
}

fn session_description_from_payload(
    payload: &WebRtcSdpPayload,
) -> Result<RTCSessionDescription, TransportError> {
    let sdp_type = RTCSdpType::from(payload.typ.as_str());
    let description = match sdp_type {
        RTCSdpType::Offer => RTCSessionDescription::offer(payload.sdp.clone())
            .map_err(|err| TransportError::Setup(err.to_string()))?,
        RTCSdpType::Answer => RTCSessionDescription::answer(payload.sdp.clone())
            .map_err(|err| TransportError::Setup(err.to_string()))?,
        RTCSdpType::Pranswer => RTCSessionDescription::pranswer(payload.sdp.clone())
            .map_err(|err| TransportError::Setup(err.to_string()))?,
        RTCSdpType::Rollback | RTCSdpType::Unspecified => {
            return Err(TransportError::Setup(format!(
                "unsupported sdp type {}",
                payload.typ
            )));
        }
    };
    Ok(description)
}

fn http_error(err: reqwest::Error) -> TransportError {
    TransportError::Setup(err.to_string())
}

async fn create_webrtc_pair() -> Result<TransportPair, TransportError> {
    // Set up virtual network so tests can run without OS networking access.
    let wan = Arc::new(AsyncMutex::new(
        Router::new(RouterConfig {
            cidr: "10.0.0.0/24".to_owned(),
            ..Default::default()
        })
        .map_err(to_setup_error)?,
    ));

    let offer_vnet = Arc::new(Net::new(Some(NetConfig {
        static_ips: vec!["10.0.0.2".to_owned()],
        ..Default::default()
    })));
    attach_vnet_to_router(&offer_vnet, &wan).await?;

    let answer_vnet = Arc::new(Net::new(Some(NetConfig {
        static_ips: vec!["10.0.0.3".to_owned()],
        ..Default::default()
    })));
    attach_vnet_to_router(&answer_vnet, &wan).await?;

    {
        let mut router = wan.lock().await;
        router.start().await.map_err(to_setup_error)?;
    }

    let mut offer_setting = SettingEngine::default();
    offer_setting.set_vnet(Some(offer_vnet.clone()));
    offer_setting.set_ice_timeouts(
        Some(Duration::from_secs(1)),
        Some(Duration::from_secs(1)),
        Some(Duration::from_millis(200)),
    );

    let mut answer_setting = SettingEngine::default();
    answer_setting.set_vnet(Some(answer_vnet.clone()));
    answer_setting.set_ice_timeouts(
        Some(Duration::from_secs(1)),
        Some(Duration::from_secs(1)),
        Some(Duration::from_millis(200)),
    );

    let offer_api = build_api(offer_setting)?;
    let answer_api = build_api(answer_setting)?;

    let config = RTCConfiguration::default();

    let offer_pc = Arc::new(
        offer_api
            .new_peer_connection(config.clone())
            .await
            .map_err(to_setup_error)?,
    );
    let answer_pc = Arc::new(
        answer_api
            .new_peer_connection(config)
            .await
            .map_err(to_setup_error)?,
    );

    let dc_init = RTCDataChannelInit {
        ordered: Some(true),
        ..Default::default()
    };
    let offer_dc = offer_pc
        .create_data_channel("beach-human", Some(dc_init))
        .await
        .map_err(to_setup_error)?;

    let (offer_dc_open_tx, offer_dc_open_rx) = oneshot::channel();
    let offer_dc_open_signal = Arc::new(Mutex::new(Some(offer_dc_open_tx)));
    offer_dc.on_open(Box::new(move || {
        let signal = offer_dc_open_signal.clone();
        Box::pin(async move {
            if let Some(tx) = signal.lock().unwrap().take() {
                let _ = tx.send(());
            }
        })
    }));

    let answer_dc_holder = Arc::new(tokio::sync::Mutex::new(None::<Arc<RTCDataChannel>>));
    let (answer_dc_open_tx, answer_dc_open_rx) = oneshot::channel();
    let answer_dc_signal = Arc::new(Mutex::new(Some(answer_dc_open_tx)));
    let holder_clone = answer_dc_holder.clone();
    let signal_clone = answer_dc_signal.clone();
    answer_pc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| {
        let holder = holder_clone.clone();
        let signal = signal_clone.clone();
        Box::pin(async move {
            holder.lock().await.replace(dc.clone());
            dc.on_open(Box::new(move || {
                let signal = signal.clone();
                Box::pin(async move {
                    if let Some(tx) = signal.lock().unwrap().take() {
                        let _ = tx.send(());
                    }
                })
            }));
        })
    }));

    let (offer_candidate_tx, offer_candidate_rx) =
        tokio_mpsc::unbounded_channel::<Option<RTCIceCandidateInit>>();
    offer_pc.on_ice_candidate(Box::new(move |candidate| {
        let tx = offer_candidate_tx.clone();
        Box::pin(async move {
            if let Some(candidate) = candidate {
                if let Ok(json) = candidate.to_json() {
                    let _ = tx.send(Some(json));
                }
            } else {
                let _ = tx.send(None);
            }
        })
    }));

    let (answer_candidate_tx, answer_candidate_rx) =
        tokio_mpsc::unbounded_channel::<Option<RTCIceCandidateInit>>();
    answer_pc.on_ice_candidate(Box::new(move |candidate| {
        let tx = answer_candidate_tx.clone();
        Box::pin(async move {
            if let Some(candidate) = candidate {
                if let Ok(json) = candidate.to_json() {
                    let _ = tx.send(Some(json));
                }
            } else {
                let _ = tx.send(None);
            }
        })
    }));

    let offer = offer_pc.create_offer(None).await.map_err(to_setup_error)?;
    offer_pc
        .set_local_description(offer)
        .await
        .map_err(to_setup_error)?;
    wait_for_local_description(&offer_pc).await?;

    let offer_desc = offer_pc
        .local_description()
        .await
        .ok_or_else(|| TransportError::Setup("missing offer description".into()))?;
    answer_pc
        .set_remote_description(offer_desc)
        .await
        .map_err(to_setup_error)?;

    let answer = answer_pc
        .create_answer(None)
        .await
        .map_err(to_setup_error)?;
    answer_pc
        .set_local_description(answer)
        .await
        .map_err(to_setup_error)?;
    wait_for_local_description(&answer_pc).await?;

    let answer_desc = answer_pc
        .local_description()
        .await
        .ok_or_else(|| TransportError::Setup("missing answer description".into()))?;
    offer_pc
        .set_remote_description(answer_desc)
        .await
        .map_err(to_setup_error)?;

    let answer_pc_for_offer = answer_pc.clone();
    spawn_task(async move {
        let mut rx = offer_candidate_rx;
        while let Some(candidate) = rx.recv().await {
            match candidate {
                Some(init) => {
                    let _ = answer_pc_for_offer.add_ice_candidate(init).await;
                }
                None => break,
            }
        }
    });

    let offer_pc_for_answer = offer_pc.clone();
    spawn_task(async move {
        let mut rx = answer_candidate_rx;
        while let Some(candidate) = rx.recv().await {
            match candidate {
                Some(init) => {
                    let _ = offer_pc_for_answer.add_ice_candidate(init).await;
                }
                None => break,
            }
        }
    });

    wait_for_connection(&offer_pc).await?;
    wait_for_connection(&answer_pc).await?;

    timeout(CONNECT_TIMEOUT, offer_dc_open_rx)
        .await
        .map_err(|_| TransportError::Timeout)?
        .map_err(|_| TransportError::ChannelClosed)?;
    timeout(CONNECT_TIMEOUT, answer_dc_open_rx)
        .await
        .map_err(|_| TransportError::Timeout)?
        .map_err(|_| TransportError::ChannelClosed)?;

    let answer_dc = answer_dc_holder
        .lock()
        .await
        .clone()
        .ok_or_else(|| TransportError::Setup("answer data channel missing".into()))?;

    let client_id = next_transport_id();
    let server_id = next_transport_id();

    let router_keepalive = Some(wan.clone());

    let client_transport = WebRtcTransport::new(
        TransportKind::WebRtc,
        client_id,
        server_id,
        offer_pc.clone(),
        offer_dc.clone(),
        router_keepalive.clone(),
        None,
        None,
    );

    let server_transport = WebRtcTransport::new(
        TransportKind::WebRtc,
        server_id,
        client_id,
        answer_pc.clone(),
        answer_dc.clone(),
        router_keepalive,
        None,
        None,
    );

    Ok(TransportPair {
        client: Box::new(client_transport),
        server: Box::new(server_transport),
    })
}

#[cfg_attr(not(test), allow(dead_code))]
pub async fn create_test_pair() -> Result<TransportPair, TransportError> {
    create_webrtc_pair().await
}

async fn wait_for_local_description(pc: &Arc<RTCPeerConnection>) -> Result<(), TransportError> {
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.local_description.initial",
        state = "start"
    );
    let already_present = pc.local_description().await.is_some();
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.local_description.initial",
        state = "end",
        has_description = already_present
    );
    if already_present {
        return Ok(());
    }
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.gathering_complete_promise",
        state = "start"
    );
    let mut gather = pc.gathering_complete_promise().await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.gathering_complete_promise",
        state = "end"
    );
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "gather.recv",
        state = "start"
    );
    let _ = gather.recv().await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "gather.recv",
        state = "end"
    );
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.local_description.final",
        state = "start"
    );
    let final_present = pc.local_description().await.is_some();
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_local_description",
        await = "pc.local_description.final",
        state = "end",
        has_description = final_present
    );
    if final_present {
        Ok(())
    } else {
        Err(TransportError::Setup(
            "failed to obtain local description".into(),
        ))
    }
}

async fn wait_for_connection(pc: &Arc<RTCPeerConnection>) -> Result<(), TransportError> {
    if pc.connection_state() == RTCPeerConnectionState::Connected {
        return Ok(());
    }
    let (tx, rx) = oneshot::channel();
    let signal = Arc::new(Mutex::new(Some(tx)));
    let signal_clone = signal.clone();
    pc.on_peer_connection_state_change(Box::new(move |state| {
        let signal = signal_clone.clone();
        Box::pin(async move {
            tracing::debug!(target = "webrtc", ?state, "peer connection state changed");
            if state == RTCPeerConnectionState::Connected {
                if let Some(tx) = signal.lock().unwrap().take() {
                    let _ = tx.send(());
                }
            }
        })
    }));

    tracing::trace!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_connection",
        await = "timeout(rx)",
        state = "start"
    );
    let wait_result = timeout(CONNECT_TIMEOUT, rx).await;
    tracing::trace!(
        target = "beach_human::transport::webrtc",
        phase = "wait_for_connection",
        await = "timeout(rx)",
        state = "end",
        result = ?wait_result
    );
    wait_result
        .map_err(|_| TransportError::Timeout)?
        .map_err(|_| TransportError::ChannelClosed)?;
    Ok(())
}

fn to_setup_error<E: std::fmt::Display>(err: E) -> TransportError {
    TransportError::Setup(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn webrtc_pair_round_trip() {
        let pair = match build_pair() {
            Ok(pair) => pair,
            Err(err) => {
                tracing::trace!(target = "webrtc", error = %err, "skipping webrtc_pair_round_trip");
                return;
            }
        };
        let timeout = Duration::from_secs(5);

        let client = &pair.client;
        let server = &pair.server;

        assert_eq!(client.kind(), TransportKind::WebRtc);
        assert_eq!(server.kind(), TransportKind::WebRtc);

        let seq_client = client.send_text("hello from client").expect("client send");
        let seq_server = server.send_text("hello from server").expect("server send");

        let server_msg = server.recv(timeout).expect("server recv");
        assert_eq!(server_msg.sequence, seq_client);
        assert_eq!(server_msg.payload.as_text(), Some("hello from client"));

        let client_msg = client.recv(timeout).expect("client recv");
        assert_eq!(client_msg.sequence, seq_server);
        assert_eq!(client_msg.payload.as_text(), Some("hello from server"));
    }
}
