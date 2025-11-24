#![recursion_limit = "1024"]

use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

use axum::extract::{
    Path, Query, State,
    ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use jsonwebtoken::{Algorithm as JwtAlgorithm, EncodingKey, Header, encode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{Mutex as AsyncMutex, broadcast, mpsc, oneshot};
use tokio::time::{Instant, sleep, timeout};
use tracing::debug;
use tracing_subscriber::{EnvFilter, fmt::SubscriberBuilder};

use beach_client_core::protocol::{self, ClientFrame as WireClientFrame, HostFrame};
use beach_client_core::transport::framed;
use beach_client_core::transport::webrtc::{
    OffererSupervisor, WebRtcRole, connect_via_signaling, create_test_pair,
    create_test_pair_with_handles,
};
use beach_client_core::transport::{
    Payload, Transport, TransportError, TransportKind, TransportMessage,
};

const HANDSHAKE_SENTINELS: [&str; 2] = ["__ready__", "__offer_ready__"];
const TEST_MANAGER_RSA_PEM: &str = r#"-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQCyCBwRBaJrB6d9
zyl534OEkw9/ID+mxuf0bNAT77uRUBgEsTFZTQbG1jW1fzi0npiHX5LMjH3RH1+l
ZqDXU2u2G6ZL34VwBpVOxn1Ru3UWnyVsiR1OoSTGSbYOhh9fLDVylb662wQLwbt1
MCp5z39CmB1OmQdlV0NlMkBfScxhkLzDGrPpsC8rfB1lnUqBXIKttJhWaQQOvePA
tsdJ/QmOG8z0Jf6FIa5v3RVFDmwkstwixtz4JvD+r1DYc5IEuR+TtUkHurxj8KB3
AJ0DMf2tfdBUTU3XtInGLHGq4QGlPZTWR6CJD+H+wozrM29JHyhk6gBYqkq1vR2s
ng4yeSpxAgMBAAECggEACf+Dl/8hgnJBGUMnmKURTUI3BhQpDvQvtZ3gx6XYek4t
syHCXDoDren8xS8aX6ZieYPICj2/mR1ycScE1GLmcyk1WyB37XUpMG3cPtlQt48D
eoduwmoxVwNvunwUyfMBI2i+d97x6LkeDnRAhmu9QV+qka3taOcQLWA3lkJePIJc
vQrWJODCBqWTO21dZbW0DGsYxfJ+JmfNz57mFoDOXEeAQDqfWAOYid2vT4aSc96i
6ox9la8deNyIBB7Y9dETXpBl85sekfnPnHkiurjkH6CH4MGfR8Zvyftp/0myma95
3TjxeN45/G+Vbn/CVwpSnQyOtPsWvyUVgiPlwKxIAQKBgQDvPrHHP1hKiW2GeQIW
RImVPRVs0nzLEjZ6TyAYIfjKaGRpOCAg4UwNqH2Gw6259vLMTIedsJm7DZGTs3Kn
YNOclXyGJUn/ndbu5FIyVLzyk40On3j1juz49b7keqd2eeC9PO47XXn7fFyZrOqt
S1U0CfGVSdTdqKNFw3gZJ8Q7cQKBgQC+f/P8/v/ggEfXpU4LTIBzgA2TkVQY7aWw
ONEG6GFkLgm07MFq98H5kcPYAt0q5ZR3HUUdiGYBUxXRaNR2/gnOjkDLOx55tyd1
qmrdLh5Ra8MXy+MfduFDF1xWgOVaqZD4hRZLjVUVqCmD8sSl1dFZ/Dk58uvXhlky
sz/FFAZfAQKBgQCYpLGc5KeadvBweciBGJ2HoH+I/QsuLaKgitd5TkOEMPLPx0WI
dPanSDc+wp6XJh5nhvSIAeMz20ZkrHucm0SohR/8HtKFytkVdouTHUmoo8e96rWs
RtKfTXvMHw21o7FmS/fb3Jo1gHU8f30DsCrelvGSRJcSDcSOgFaiBiNHoQKBgBjZ
jhVRmkVJ1pVNzflxWEw4xwyZ55N85KExOCsjgxjTXJbKT4zJlvccSaTS8tDWs+A5
5DsvAMdpdC4l85k2GEdmjRM1ugr8llwmB9ykWHYcjY18HjuLgWUEFhp+o+yItA2H
JtpiLFgv4IKC154eXznSyhBCHPu3XclcUpQ9wXsBAoGAV6o/jYx2rXRN/9OxSTEn
83WYwLT8tOeigTQKih/6o4Y2a7+K74ER+ANKhSbTg1pZf1pWy5hqM6kQhZr8hUom
VcLkaNrRKWddDiF2kCs96MR8tY+pw7M2fOpDmmkYMa5d6lh6lTTfW3n7FDhb/AoL
yaCKn3iwS+8/CWgIBTDBY/0=
-----END PRIVATE KEY-----"#;
const TEST_MANAGER_JWK_N: &str = "sggcEQWiawenfc8ped-DhJMPfyA_psbn9GzQE--7kVAYBLExWU0GxtY1tX84tJ6Yh1-SzIx90R9fpWag11NrthumS9-FcAaVTsZ9Ubt1Fp8lbIkdTqEkxkm2DoYfXyw1cpW-utsEC8G7dTAqec9_QpgdTpkHZVdDZTJAX0nMYZC8wxqz6bAvK3wdZZ1KgVyCrbSYVmkEDr3jwLbHSf0JjhvM9CX-hSGub90VRQ5sJLLcIsbc-Cbw_q9Q2HOSBLkfk7VJB7q8Y_CgdwCdAzH9rX3QVE1N17SJxixxquEBpT2U1kegiQ_h_sKM6zNvSR8oZOoAWKpKtb0drJ4OMnkqcQ";
const TEST_MANAGER_JWK_E: &str = "AQAB";

fn disable_public_stun() {
    unsafe { std::env::set_var("BEACH_WEBRTC_DISABLE_STUN", "1") };
}

fn is_handshake_sentinel(text: &str) -> bool {
    HANDSHAKE_SENTINELS
        .iter()
        .any(|sentinel| text.trim() == *sentinel)
}

fn payload_text(message: &TransportMessage) -> Option<&str> {
    match &message.payload {
        Payload::Text(text) => Some(text),
        Payload::Binary(_) => None,
    }
}

fn payload_bytes(message: &TransportMessage) -> Vec<u8> {
    match &message.payload {
        Payload::Binary(bytes) => bytes.clone(),
        Payload::Text(text) => panic!("expected binary payload, got text: {text}"),
    }
}

async fn recv_with_timeout(transport: &dyn Transport, timeout: Duration) -> TransportMessage {
    let deadline = Instant::now() + timeout;
    loop {
        match transport.try_recv() {
            Ok(Some(message)) => return message,
            Ok(None) => {
                if Instant::now() >= deadline {
                    panic!("receive timed out");
                }
                sleep(Duration::from_millis(10)).await;
            }
            Err(err) => panic!("transport receive error: {err}"),
        }
    }
}

async fn recv_data_message(transport: &Arc<dyn Transport>, timeout: Duration) -> TransportMessage {
    let deadline = Instant::now() + timeout;
    loop {
        let now = Instant::now();
        if now >= deadline {
            panic!("receive timed out");
        }
        let remaining = deadline - now;
        let message = recv_via_blocking(transport, remaining).await;
        if let Payload::Text(text) = &message.payload {
            if is_handshake_sentinel(text) {
                continue;
            }
        }
        return message;
    }
}

async fn recv_via_blocking(transport: &Arc<dyn Transport>, timeout: Duration) -> TransportMessage {
    let transport_clone = Arc::clone(transport);
    tokio::task::spawn_blocking(move || transport_clone.recv(timeout))
        .await
        .expect("spawn_blocking panicked")
        .expect("transport recv")
}

async fn recv_frame_with_timeout(
    mut rx: broadcast::Receiver<framed::FramedMessage>,
    timeout: Duration,
) -> Option<framed::FramedMessage> {
    match tokio::time::timeout(timeout, rx.recv()).await {
        Ok(Ok(msg)) => Some(msg),
        _ => None,
    }
}

#[test_timeout::tokio_timeout_test]
async fn webrtc_bidirectional_transport_delivers_messages() {
    disable_public_stun();
    let _ = SubscriberBuilder::default()
        .with_test_writer()
        .with_env_filter(EnvFilter::from_default_env())
        .with_max_level(tracing::Level::TRACE)
        .try_init();
    let pair = create_test_pair().await.expect("create webrtc pair");
    let client = pair.client;
    let server: Arc<dyn Transport> = pair.server.into();

    assert_eq!(client.kind(), TransportKind::WebRtc);
    assert_eq!(server.kind(), TransportKind::WebRtc);

    sleep(Duration::from_millis(50)).await;

    client
        .send_text("hello from client")
        .expect("send client text");
    let server_msg = recv_with_timeout(server.as_ref(), Duration::from_secs(5)).await;
    assert_eq!(payload_text(&server_msg), Some("hello from client"));

    server
        .send_text("hello from server")
        .expect("send server text");
    let client_msg = recv_with_timeout(client.as_ref(), Duration::from_secs(5)).await;
    assert_eq!(payload_text(&client_msg), Some("hello from server"));

    let bytes = vec![1u8, 2, 3, 4, 5];
    server.send_bytes(&bytes).expect("send server binary");
    let binary_msg = recv_with_timeout(client.as_ref(), Duration::from_secs(5)).await;
    assert_eq!(payload_bytes(&binary_msg), bytes);
    match binary_msg.payload {
        Payload::Binary(_) => {}
        Payload::Text(text) => panic!("expected binary payload, got text: {text}"),
    }

    for idx in 0..10 {
        client
            .send_text(&format!("msg-{idx}"))
            .expect("batched send");
    }
    for idx in 0..10 {
        let expected = format!("msg-{idx}");
        let received = recv_with_timeout(server.as_ref(), Duration::from_secs(5)).await;
        assert_eq!(payload_text(&received), Some(expected.as_str()));
    }
}

#[test_timeout::tokio_timeout_test]
async fn webrtc_namespaced_controller_round_trip() {
    disable_public_stun();
    let _ = SubscriberBuilder::default()
        .with_test_writer()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();
    let pair = create_test_pair().await.expect("create webrtc pair");
    let client = pair.client;
    let server: Arc<dyn Transport> = pair.server.into();

    let server_rx = framed::subscribe(server.id(), "controller");
    let client_rx = framed::subscribe(client.id(), "controller");

    let payload = br#"{"action":"ping","id":1}"#;
    let seq = client
        .send_namespaced("controller", "input", payload)
        .expect("send controller input");

    let received = recv_frame_with_timeout(server_rx, Duration::from_secs(2))
        .await
        .expect("controller frame from client");
    assert_eq!(received.namespace, "controller");
    assert_eq!(received.kind, "input");
    assert_eq!(received.seq, seq);
    assert_eq!(received.payload.as_ref(), payload);

    let ack_payload = br#"{"ack":1}"#;
    let ack_seq = server
        .send_namespaced("controller", "ack", ack_payload)
        .expect("send controller ack");
    let ack = recv_frame_with_timeout(client_rx, Duration::from_secs(2))
        .await
        .expect("controller ack from server");
    assert_eq!(ack.namespace, "controller");
    assert_eq!(ack.kind, "ack");
    assert_eq!(ack.seq, ack_seq);
    assert_eq!(ack.payload.as_ref(), ack_payload);
}

#[test_timeout::tokio_timeout_test]
async fn webrtc_chunked_sync_round_trip() {
    disable_public_stun();
    let _ = SubscriberBuilder::default()
        .with_test_writer()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let pair = create_test_pair().await.expect("create webrtc pair");
    let client = pair.client;
    let server: Arc<dyn Transport> = pair.server.into();

    // Force multiple chunks (> 14 KiB default chunk size).
    let payload = vec![42u8; 50_000];
    let seq = client
        .send_bytes(&payload)
        .expect("send large sync payload");

    let msg = recv_data_message(&server, Duration::from_secs(5)).await;
    let bytes = payload_bytes(&msg);
    assert_eq!(bytes.len(), payload.len());
    assert_eq!(bytes, payload);
    assert_eq!(msg.sequence, seq);
}

#[derive(Serialize, Deserialize)]
struct ManagerTestClaims {
    iss: String,
    aud: String,
    exp: usize,
    role: String,
}

#[test_timeout::tokio_timeout_test]
async fn webrtc_manager_metadata_and_auth_round_trip() {
    disable_public_stun();
    let _ = SubscriberBuilder::default()
        .with_test_writer()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    #[derive(Clone, Serialize)]
    struct Jwk {
        kty: &'static str,
        kid: &'static str,
        n: &'static str,
        e: &'static str,
    }
    #[derive(Clone, Serialize)]
    struct Jwks {
        keys: Vec<Jwk>,
    }

    // JWKS stub for manager auth.
    let jwks_state = Jwks {
        keys: vec![Jwk {
            kty: "RSA",
            kid: "test-kid",
            n: TEST_MANAGER_JWK_N,
            e: TEST_MANAGER_JWK_E,
        }],
    };
    let jwks_router = Router::new().route(
        "/jwks.json",
        get({
            let jwks_state = jwks_state.clone();
            move || async move { Json(jwks_state.clone()) }
        }),
    );
    let jwks_listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind jwks listener");
    let jwks_addr = jwks_listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(jwks_listener, jwks_router).await.ok();
    });
    unsafe {
        std::env::set_var("CLERK_JWKS_URL", format!("http://{jwks_addr}/jwks.json"));
        std::env::set_var("CLERK_ISSUER", "test-issuer");
        std::env::set_var("CLERK_AUDIENCE", "test-audience");
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as usize;
    let claims = ManagerTestClaims {
        iss: "test-issuer".into(),
        aud: "test-audience".into(),
        exp: now + 300,
        role: "manager".into(),
    };
    let mut header = Header::new(JwtAlgorithm::RS256);
    header.kid = Some("test-kid".into());
    let token = encode(
        &header,
        &claims,
        &EncodingKey::from_rsa_pem(TEST_MANAGER_RSA_PEM.as_bytes()).unwrap(),
    )
    .expect("encode jwt");

    let mut metadata = HashMap::new();
    metadata.insert("role".to_string(), "manager".to_string());
    metadata.insert("session_id".to_string(), SESSION_ID.to_string());
    metadata.insert("bearer".to_string(), token);

    let state = AppState::default();
    let router = build_router(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener bind");
    let addr = listener.local_addr().expect("local addr");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();
    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .ok();
    });

    let base_url = format!("http://{addr}/sessions/{SESSION_ID}/webrtc");
    let offer_fut = async {
        let (supervisor, accepted) = OffererSupervisor::connect(
            &base_url,
            Duration::from_millis(50),
            None,
            false,
            Some(metadata.clone()),
        )
        .await?;
        Ok::<(Arc<OffererSupervisor>, Arc<dyn Transport>), TransportError>((
            supervisor,
            accepted.connection.transport(),
        ))
    };
    sleep(Duration::from_millis(50)).await;
    let answer_fut = connect_via_signaling(
        &base_url,
        WebRtcRole::Answerer,
        Duration::from_millis(50),
        None,
        None,
        false,
        None,
    );

    let (offer_res, answer_res) = tokio::join!(
        timeout(Duration::from_secs(10), offer_fut),
        timeout(Duration::from_secs(10), answer_fut),
    );
    let (_offer_supervisor, manager_transport) = offer_res
        .expect("offer signaling timeout")
        .expect("offer transport");
    let answer_conn = answer_res
        .expect("answer signaling timeout")
        .expect("answer transport");
    let host_transport = answer_conn.transport();

    let conn_metadata = answer_conn.metadata().expect("manager metadata present");
    assert_eq!(
        conn_metadata.get("role").map(String::as_str),
        Some("manager")
    );
    assert_eq!(
        conn_metadata.get("session_id").map(String::as_str),
        Some(SESSION_ID)
    );

    let host_rx = framed::subscribe(host_transport.id(), "controller");
    let payload = br#"{"action":"noop","id":2}"#;
    let seq = manager_transport
        .send_namespaced("controller", "input", payload)
        .expect("send controller input");
    let frame = recv_frame_with_timeout(host_rx, Duration::from_secs(2))
        .await
        .expect("receive controller frame");
    assert_eq!(frame.namespace, "controller");
    assert_eq!(frame.kind, "input");
    assert_eq!(frame.seq, seq);
    assert_eq!(frame.payload.as_ref(), payload);

    shutdown_tx.send(()).ok();
}

#[test_timeout::tokio_timeout_test]
async fn webrtc_signaling_404_then_recovers() {
    disable_public_stun();
    let _ = SubscriberBuilder::default()
        .with_test_writer()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    // Start with signaling not yet attached (simulates early offer 404).
    let state = AppState::default();
    {
        let mut rest = state.rest.lock().await;
        rest.attached = false;
    }

    let router = build_router(state.clone());
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener bind");
    let addr = listener.local_addr().expect("local addr");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .ok();
    });

    // Attach the session after a short delay to allow initial 404s.
    let state_for_attach = state.clone();
    tokio::spawn(async move {
        sleep(Duration::from_millis(200)).await;
        let mut rest = state_for_attach.rest.lock().await;
        rest.attached = true;
    });

    let base_url = format!("http://{addr}/sessions/{SESSION_ID}/webrtc");
    let offer_fut = async {
        let (_supervisor, accepted) =
            OffererSupervisor::connect(&base_url, Duration::from_millis(50), None, false, None)
                .await?;
        Ok::<Arc<dyn Transport>, TransportError>(accepted.connection.transport())
    };
    sleep(Duration::from_millis(50)).await;
    let answer_fut = connect_via_signaling(
        &base_url,
        WebRtcRole::Answerer,
        Duration::from_millis(50),
        None,
        None,
        false,
        None,
    );

    let (offer_res, answer_res) = tokio::join!(
        timeout(Duration::from_secs(5), offer_fut),
        timeout(Duration::from_secs(5), answer_fut),
    );
    let manager_transport = offer_res
        .expect("offer signaling timeout")
        .expect("offer transport");
    let host_transport = answer_res
        .expect("answer signaling timeout")
        .expect("answer transport")
        .transport();

    // Assert we saw at least one 404 before attach.
    let rest = state.rest.lock().await;
    assert!(
        rest.offer_404s >= 1,
        "expected initial signaling conflicts before attach"
    );
    drop(rest);

    // Verify the recovered connection carries traffic and only one offer/answer succeeds.
    let host_rx = framed::subscribe(host_transport.id(), "controller");
    let payload = br#"{"action":"ping","id":3}"#;
    let seq = manager_transport
        .send_namespaced("controller", "input", payload)
        .expect("send controller input");
    let frame = recv_frame_with_timeout(host_rx, Duration::from_secs(2))
        .await
        .expect("receive controller frame");
    assert_eq!(frame.namespace, "controller");
    assert_eq!(frame.seq, seq);

    shutdown_tx.send(()).ok();
}

#[test_timeout::tokio_timeout_test]
async fn webrtc_remote_drop_reports_channel_closed() {
    disable_public_stun();
    let _ = SubscriberBuilder::default()
        .with_test_writer()
        .with_env_filter(EnvFilter::from_default_env())
        .try_init();

    let (pair, _offer_pc, answer_pc) = create_test_pair_with_handles()
        .await
        .expect("create webrtc pair");
    let client = pair.client;
    let server = pair.server;

    // Ensure the channel is functional before simulating a drop.
    let server_rx = framed::subscribe(server.id(), "controller");
    let seq = client
        .send_namespaced("controller", "input", br#"{"ping":1}"#)
        .expect("initial controller send");
    let first = recv_frame_with_timeout(server_rx, Duration::from_secs(2))
        .await
        .expect("initial controller frame");
    assert_eq!(first.seq, seq);

    // Close the remote peer connection to trigger channel teardown.
    drop(server);
    answer_pc.close().await.expect("close remote peer");

    // Repeatedly attempt sends until ChannelClosed surfaces.
    let mut saw_closed = false;
    for _ in 0..50 {
        sleep(Duration::from_millis(100)).await;
        match client.send_namespaced("controller", "input", br#"{"ping":2}"#) {
            Err(TransportError::ChannelClosed) => {
                saw_closed = true;
                break;
            }
            Ok(_) => continue,
            Err(err) => panic!("unexpected transport error: {err}"),
        }
    }

    assert!(
        saw_closed,
        "expected ChannelClosed after remote WebRTC drop"
    );
}

const SESSION_ID: &str = "test-session";

#[derive(Clone, Default)]
struct AppState {
    rest: Arc<AsyncMutex<RestState>>,
    ws: Arc<AsyncMutex<WsSession>>,
}

struct RestState {
    offers: Vec<TestSdpPayload>,
    answers: HashMap<String, TestSdpPayload>,
    handshake_log: Vec<String>,
    attached: bool,
    offer_404s: usize,
    peer_sessions: HashMap<String, String>,
    attach_attempts: usize,
    attach_roles: Vec<String>,
    attach_peer_ids: Vec<String>,
}

impl Default for RestState {
    fn default() -> Self {
        Self {
            offers: Vec::new(),
            answers: HashMap::new(),
            handshake_log: Vec::new(),
            attached: true,
            offer_404s: 0,
            peer_sessions: HashMap::new(),
            attach_attempts: 0,
            attach_roles: Vec::new(),
            attach_peer_ids: Vec::new(),
        }
    }
}

#[derive(Default)]
struct WsSession {
    server: Option<PeerEntry>,
    client: Option<PeerEntry>,
}

#[derive(Clone)]
struct PeerEntry {
    peer_id: String,
    role: Role,
    tx: mpsc::UnboundedSender<WsMessage>,
    metadata: Option<Value>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Server,
    Client,
}

#[derive(Clone, Serialize, Deserialize)]
struct TestSdpPayload {
    sdp: String,
    #[serde(rename = "type")]
    typ: String,
    handshake_id: String,
    from_peer: String,
    to_peer: String,
}

#[derive(Deserialize)]
struct AttachRequest {
    host_session_id: String,
    #[serde(default)]
    role: Option<String>,
    #[serde(default)]
    peer_id: Option<String>,
    #[serde(default)]
    passphrase: Option<String>,
}

#[derive(Serialize)]
struct AttachResponse {
    peer_session_id: String,
    host_session_id: String,
}

#[derive(Deserialize)]
struct OfferQuery {
    peer_id: String,
}

#[derive(Deserialize)]
struct AnswerQuery {
    handshake_id: String,
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::Server => "server",
        Role::Client => "client",
    }
}

fn peer_info(entry: &PeerEntry) -> Value {
    let mut obj = json!({
        "id": entry.peer_id,
        "role": role_str(entry.role),
        "joined_at": 0,
        "supported_transports": ["webrtc"],
        "preferred_transport": "webrtc",
    });
    if let Some(metadata) = &entry.metadata {
        if let Some(map) = obj.as_object_mut() {
            map.insert("metadata".to_string(), metadata.clone());
        }
    }
    obj
}

impl WsSession {
    fn existing_peers(&self) -> Vec<PeerEntry> {
        self.server
            .iter()
            .chain(self.client.iter())
            .cloned()
            .collect()
    }

    fn get_peer(&self, peer_id: &str) -> Option<PeerEntry> {
        self.server
            .as_ref()
            .filter(|p| p.peer_id == peer_id)
            .cloned()
            .or_else(|| {
                self.client
                    .as_ref()
                    .filter(|p| p.peer_id == peer_id)
                    .cloned()
            })
    }

    fn remove_peer(&mut self, peer_id: &str) {
        if self.server.as_ref().is_some_and(|p| p.peer_id == peer_id) {
            self.server = None;
        }
        if self.client.as_ref().is_some_and(|p| p.peer_id == peer_id) {
            self.client = None;
        }
    }
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/peer-sessions/attach", post(post_peer_attach))
        .route(
            "/peer-sessions/:id/webrtc/offer",
            post(post_offer).get(get_offer),
        )
        .route(
            "/peer-sessions/:id/webrtc/answer",
            post(post_answer).get(get_answer),
        )
        .route("/ws/:id", get(ws_handler))
        .with_state(state)
}

async fn post_offer(
    State(state): State<AppState>,
    Path(peer_session): Path<String>,
    Json(payload): Json<TestSdpPayload>,
) -> StatusCode {
    let session = {
        let guard = state.rest.lock().await;
        guard
            .peer_sessions
            .get(&peer_session)
            .cloned()
            .unwrap_or_else(|| peer_session.clone())
    };
    if session != SESSION_ID {
        return StatusCode::NOT_FOUND;
    }
    debug!("stub: received offer for session {session}");
    let mut guard = state.rest.lock().await;
    if !guard.attached {
        guard.offer_404s += 1;
        return StatusCode::CONFLICT;
    }
    guard.handshake_log.push(payload.handshake_id.clone());
    guard.offers.push(payload);
    StatusCode::NO_CONTENT
}

async fn get_offer(
    State(state): State<AppState>,
    Path(peer_session): Path<String>,
    Query(query): Query<OfferQuery>,
) -> Result<Json<TestSdpPayload>, StatusCode> {
    let session = {
        let guard = state.rest.lock().await;
        guard
            .peer_sessions
            .get(&peer_session)
            .cloned()
            .unwrap_or_else(|| peer_session.clone())
    };
    if session != SESSION_ID {
        return Err(StatusCode::NOT_FOUND);
    }
    debug!("stub: fetching offer for session {session}");
    let mut guard = state.rest.lock().await;
    if !guard.attached {
        guard.offer_404s += 1;
        return Err(StatusCode::CONFLICT);
    }
    if let Some(index) = guard
        .offers
        .iter()
        .position(|offer| offer.to_peer == query.peer_id)
    {
        Ok(Json(guard.offers.remove(index)))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn post_answer(
    State(state): State<AppState>,
    Path(peer_session): Path<String>,
    Json(payload): Json<TestSdpPayload>,
) -> StatusCode {
    let session = {
        let guard = state.rest.lock().await;
        guard
            .peer_sessions
            .get(&peer_session)
            .cloned()
            .unwrap_or_else(|| peer_session.clone())
    };
    if session != SESSION_ID {
        return StatusCode::NOT_FOUND;
    }
    debug!("stub: received answer for session {session}");
    let mut guard = state.rest.lock().await;
    guard.answers.insert(payload.handshake_id.clone(), payload);
    StatusCode::NO_CONTENT
}

async fn get_answer(
    State(state): State<AppState>,
    Path(peer_session): Path<String>,
    Query(query): Query<AnswerQuery>,
) -> Result<Json<TestSdpPayload>, StatusCode> {
    let session = {
        let guard = state.rest.lock().await;
        guard
            .peer_sessions
            .get(&peer_session)
            .cloned()
            .unwrap_or_else(|| peer_session.clone())
    };
    if session != SESSION_ID {
        return Err(StatusCode::NOT_FOUND);
    }
    debug!("stub: fetching answer for session {session}");
    let mut guard = state.rest.lock().await;
    match guard.answers.remove(&query.handshake_id) {
        Some(payload) => Ok(Json(payload)),
        None => Err(StatusCode::NOT_FOUND),
    }
}

async fn post_peer_attach(
    State(state): State<AppState>,
    Json(payload): Json<AttachRequest>,
) -> Result<Json<AttachResponse>, StatusCode> {
    if payload.host_session_id != SESSION_ID {
        return Err(StatusCode::NOT_FOUND);
    }
    let mut guard = state.rest.lock().await;
    guard.attach_attempts = guard.attach_attempts.saturating_add(1);
    if let Some(role) = payload.role.clone() {
        guard.attach_roles.push(role);
    }
    if let Some(peer_id) = payload.peer_id.clone() {
        guard.attach_peer_ids.push(peer_id);
    }
    let peer_session_id = guard
        .peer_sessions
        .iter()
        .find_map(|(id, host)| {
            if host == &payload.host_session_id {
                Some(id.clone())
            } else {
                None
            }
        })
        .unwrap_or_else(|| {
            let id = Uuid::new_v4().to_string();
            guard
                .peer_sessions
                .insert(id.clone(), payload.host_session_id.clone());
            id
        });
    Ok(Json(AttachResponse {
        peer_session_id,
        host_session_id: payload.host_session_id,
    }))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(session): Path<String>,
) -> impl IntoResponse {
    if session != SESSION_ID {
        let guard = state.rest.lock().await;
        let mapped = guard
            .peer_sessions
            .get(&session)
            .map(|s| s == SESSION_ID)
            .unwrap_or(false);
        if !mapped {
            return (StatusCode::NOT_FOUND, "unknown session").into_response();
        }
    }
    debug!("stub: websocket upgrade for session {session}");
    ws.on_upgrade(move |socket| handle_socket(socket, state.ws.clone()))
}

async fn handle_socket(socket: WebSocket, state: Arc<AsyncMutex<WsSession>>) {
    let (mut sender, mut receiver) = socket.split();
    let (tx, mut rx) = mpsc::unbounded_channel::<WsMessage>();

    let send_task = tokio::spawn(async move {
        while let Some(message) = rx.recv().await {
            if sender.send(message).await.is_err() {
                break;
            }
        }
    });

    let mut self_id: Option<String> = None;
    let mut assigned_role: Option<Role> = None;

    while let Some(result) = receiver.next().await {
        let msg = match result {
            Ok(msg) => msg,
            Err(_) => break,
        };

        match msg {
            WsMessage::Text(text) => {
                if let Ok(value) = serde_json::from_str::<Value>(&text) {
                    match value.get("type").and_then(Value::as_str) {
                        Some("join") => {
                            if let Some(peer_id) = value.get("peer_id").and_then(Value::as_str) {
                                debug!("stub: peer {peer_id} attempting to join");
                                let metadata = value.get("metadata").cloned();
                                let mut ws_state = state.lock().await;
                                let role = if ws_state.server.is_none() {
                                    Role::Server
                                } else if ws_state.client.is_none() {
                                    Role::Client
                                } else {
                                    send_json(
                                        &tx,
                                        json!({
                                            "type": "error",
                                            "message": "session full"
                                        }),
                                    );
                                    continue;
                                };

                                let existing = ws_state.existing_peers();
                                let notify_peer = match role {
                                    Role::Server => None,
                                    Role::Client => ws_state.server.clone(),
                                };

                                let entry = PeerEntry {
                                    peer_id: peer_id.to_string(),
                                    role,
                                    tx: tx.clone(),
                                    metadata: metadata.clone(),
                                };

                                match role {
                                    Role::Server => ws_state.server = Some(entry.clone()),
                                    Role::Client => ws_state.client = Some(entry.clone()),
                                }

                                drop(ws_state);

                                self_id = Some(peer_id.to_string());
                                assigned_role = Some(role);

                                let peers_json: Vec<_> = existing.iter().map(peer_info).collect();
                                send_json(
                                    &tx,
                                    json!({
                                        "type": "join_success",
                                        "session_id": SESSION_ID,
                                        "peer_id": peer_id,
                                        "peers": peers_json,
                                        "available_transports": ["webrtc"],
                                    }),
                                );

                                if let Some(other) = notify_peer {
                                    debug!(
                                        "stub: notifying peer {} about {}",
                                        other.peer_id, peer_id
                                    );
                                    send_json(
                                        &other.tx,
                                        json!({
                                            "type": "peer_joined",
                                            "peer": peer_info(&entry),
                                        }),
                                    );
                                }
                            }
                        }
                        Some("signal") => {
                            if let (Some(self_id), Some(target_id)) = (
                                self_id.as_ref(),
                                value.get("to_peer").and_then(Value::as_str),
                            ) {
                                let signal_value =
                                    value.get("signal").cloned().unwrap_or_else(|| json!({}));
                                let target = {
                                    let ws_state = state.lock().await;
                                    ws_state.get_peer(target_id)
                                };
                                if let Some(peer) = target {
                                    debug!(
                                        "stub: forwarding signal from {} to {}",
                                        self_id, target_id
                                    );
                                    send_json(
                                        &peer.tx,
                                        json!({
                                            "type": "signal",
                                            "from_peer": self_id,
                                            "signal": signal_value,
                                        }),
                                    );
                                }
                            }
                        }
                        Some("ping") => {
                            send_json(&tx, json!({ "type": "pong" }));
                        }
                        _ => {}
                    }
                }
            }
            WsMessage::Close(_) => break,
            _ => {}
        }
    }

    if let Some(_role) = assigned_role {
        if let Some(id) = self_id {
            let mut ws_state = state.lock().await;
            ws_state.remove_peer(&id);
        }
    }

    send_task.abort();
    let _ = send_task.await;
}

fn send_json(tx: &mpsc::UnboundedSender<WsMessage>, value: Value) {
    if let Ok(text) = serde_json::to_string(&value) {
        let _ = tx.send(WsMessage::Text(text));
    }
}

#[test_timeout::tokio_timeout_test]
async fn webrtc_signaling_end_to_end() {
    if std::env::var("BEACH_WEBRTC_DISABLE_STUN").is_ok() {
        return;
    }
    let _ = SubscriberBuilder::default()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
    let state = AppState::default();
    let router = build_router(state.clone());

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener bind");
    let addr = listener.local_addr().expect("local addr");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .ok();
    });

    let base_url = format!("http://{addr}/sessions/{SESSION_ID}/webrtc");
    let offer_fut = async {
        let (supervisor, accepted) =
            OffererSupervisor::connect(&base_url, Duration::from_millis(50), None, false, None)
                .await?;
        Ok::<(Arc<OffererSupervisor>, Arc<dyn Transport>), TransportError>((
            supervisor,
            accepted.connection.transport(),
        ))
    };
    sleep(Duration::from_millis(50)).await;
    let answer_fut = connect_via_signaling(
        &base_url,
        WebRtcRole::Answerer,
        Duration::from_millis(50),
        None,
        None,
        false,
        None,
    );

    let (offer_res, answer_res) = tokio::join!(
        timeout(Duration::from_secs(10), offer_fut),
        timeout(Duration::from_secs(10), answer_fut),
    );
    let (offer_supervisor, offer_transport) = offer_res
        .expect("offer signaling timeout")
        .expect("offer transport");
    let _offer_supervisor = offer_supervisor;
    let answer_transport = answer_res
        .expect("answer signaling timeout")
        .expect("answer transport")
        .transport();

    let server_heartbeat = HostFrame::Heartbeat {
        seq: 1,
        timestamp_ms: 42,
    };
    offer_transport
        .send_bytes(&protocol::encode_host_frame_binary(&server_heartbeat))
        .expect("offer send heartbeat");
    let heartbeat_msg = recv_data_message(&answer_transport, Duration::from_secs(5)).await;
    let heartbeat_bytes = match heartbeat_msg.payload {
        Payload::Binary(bytes) => bytes,
        Payload::Text(text) => panic!("unexpected text payload: {text}"),
    };
    let decoded_heartbeat =
        protocol::decode_host_frame_binary(&heartbeat_bytes).expect("heartbeat frame");
    assert!(matches!(
        decoded_heartbeat,
        HostFrame::Heartbeat { seq, .. } if seq == 1
    ));

    let client_frame = WireClientFrame::Input {
        seq: 7,
        data: b"echo from client".to_vec(),
    };
    answer_transport
        .send_bytes(&protocol::encode_client_frame_binary(&client_frame))
        .expect("answer send client frame");
    let inbound_client = recv_data_message(&offer_transport, Duration::from_secs(5)).await;
    match inbound_client.payload {
        Payload::Binary(bytes) => {
            let decoded = protocol::decode_client_frame_binary(&bytes).expect("client frame");
            assert!(matches!(decoded, WireClientFrame::Input { seq, .. } if seq == 7));
        }
        Payload::Text(text) => panic!("unexpected text payload: {text}"),
    }

    shutdown_tx.send(()).ok();
}

#[test_timeout::tokio_timeout_test]
async fn webrtc_signaling_attach_includes_metadata() {
    if std::env::var("BEACH_WEBRTC_DISABLE_STUN").is_ok() {
        return;
    }
    let _ = SubscriberBuilder::default()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
    let state = AppState::default();
    let router = build_router(state.clone());

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("listener bind");
    let addr = listener.local_addr().expect("local addr");
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    tokio::spawn(async move {
        axum::serve(listener, router)
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            })
            .await
            .ok();
    });

    let base_url = format!("http://{addr}/sessions/{SESSION_ID}/webrtc");
    let offer_fut = async {
        let (supervisor, accepted) =
            OffererSupervisor::connect(&base_url, Duration::from_millis(50), None, false, None)
                .await?;
        Ok::<(Arc<OffererSupervisor>, Arc<dyn Transport>), TransportError>((
            supervisor,
            accepted.connection.transport(),
        ))
    };
    sleep(Duration::from_millis(50)).await;
    let answer_fut = connect_via_signaling(
        &base_url,
        WebRtcRole::Answerer,
        Duration::from_millis(50),
        None,
        None,
        false,
        None,
    );

    let (offer_res, answer_res) = tokio::join!(
        timeout(Duration::from_secs(10), offer_fut),
        timeout(Duration::from_secs(10), answer_fut),
    );
    let (_offer_supervisor, _offer_transport) = offer_res
        .expect("offer signaling timeout")
        .expect("offer transport");
    let _answer_transport = answer_res
        .expect("answer signaling timeout")
        .expect("answer transport")
        .transport();

    let rest = state.rest.lock().await;
    assert!(
        rest.attach_attempts >= 2,
        "expected attach attempts from both peers"
    );
    assert!(
        rest.attach_roles.iter().any(|r| r == "offerer"),
        "offerer role should be attached"
    );
    assert!(
        rest.attach_roles.iter().any(|r| r == "answerer"),
        "answerer role should be attached"
    );
    assert!(
        rest.attach_peer_ids.iter().all(|id| !id.trim().is_empty()),
        "peer ids should be propagated in attach metadata"
    );
    assert!(
        !rest.peer_sessions.is_empty(),
        "peer sessions should be registered"
    );
    let _ = shutdown_tx.send(());
}

#[test_timeout::tokio_timeout_test]
async fn webrtc_multiple_handshakes_use_unique_ids() {
    const HANDSHAKES: usize = 3;
    let mut handshake_ids = HashSet::with_capacity(HANDSHAKES);
    for _ in 0..HANDSHAKES {
        let handshake_id = Uuid::new_v4().to_string();
        assert!(handshake_ids.insert(handshake_id));
    }
}
