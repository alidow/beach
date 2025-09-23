#![recursion_limit = "1024"]

use std::sync::Arc;
use std::time::Duration;

use axum::extract::{
    Path, State,
    ws::{Message as WsMessage, WebSocket, WebSocketUpgrade},
};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::{Mutex as AsyncMutex, mpsc, oneshot};
use tokio::time::{Instant, sleep, timeout};
use tracing::debug;
use tracing_subscriber::{EnvFilter, fmt::SubscriberBuilder};

use beach_human::protocol::{self, ClientFrame as WireClientFrame, HostFrame};
use beach_human::transport::webrtc::{WebRtcRole, connect_via_signaling, create_test_pair};
use beach_human::transport::{Payload, Transport, TransportKind, TransportMessage};

const HANDSHAKE_SENTINELS: [&str; 2] = ["__ready__", "__offer_ready__"];

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

async fn recv_with_timeout(transport: &Box<dyn Transport>, timeout: Duration) -> TransportMessage {
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

#[test_timeout::tokio_timeout_test]
async fn webrtc_bidirectional_transport_delivers_messages() {
    let _ = SubscriberBuilder::default()
        .with_test_writer()
        .with_env_filter(EnvFilter::from_default_env())
        .with_max_level(tracing::Level::TRACE)
        .try_init();
    let pair = create_test_pair().await.expect("create webrtc pair");
    let client = pair.client;
    let server = pair.server;

    assert_eq!(client.kind(), TransportKind::WebRtc);
    assert_eq!(server.kind(), TransportKind::WebRtc);

    sleep(Duration::from_millis(50)).await;

    client
        .send_text("hello from client")
        .expect("send client text");
    let server_msg = recv_with_timeout(&server, Duration::from_secs(5)).await;
    assert_eq!(payload_text(&server_msg), Some("hello from client"));

    server
        .send_text("hello from server")
        .expect("send server text");
    let client_msg = recv_with_timeout(&client, Duration::from_secs(5)).await;
    assert_eq!(payload_text(&client_msg), Some("hello from server"));

    let bytes = vec![1u8, 2, 3, 4, 5];
    server.send_bytes(&bytes).expect("send server binary");
    let binary_msg = recv_with_timeout(&client, Duration::from_secs(5)).await;
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
        let received = recv_with_timeout(&server, Duration::from_secs(5)).await;
        assert_eq!(payload_text(&received), Some(expected.as_str()));
    }
}

const SESSION_ID: &str = "test-session";

#[derive(Clone, Default)]
struct AppState {
    rest: Arc<AsyncMutex<RestState>>,
    ws: Arc<AsyncMutex<WsSession>>,
}

#[derive(Default)]
struct RestState {
    offer: Option<Vec<u8>>,
    answer: Option<Vec<u8>>,
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
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Role {
    Server,
    Client,
}

#[derive(Serialize, Deserialize)]
struct TestSdpPayload {
    sdp: String,
    #[serde(rename = "type")]
    typ: String,
}

fn role_str(role: Role) -> &'static str {
    match role {
        Role::Server => "server",
        Role::Client => "client",
    }
}

fn peer_info(entry: &PeerEntry) -> Value {
    json!({
        "id": entry.peer_id,
        "role": role_str(entry.role),
        "joined_at": 0,
        "supported_transports": ["webrtc"],
        "preferred_transport": "webrtc",
    })
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
        if self.server.as_ref().map_or(false, |p| p.peer_id == peer_id) {
            self.server = None;
        }
        if self.client.as_ref().map_or(false, |p| p.peer_id == peer_id) {
            self.client = None;
        }
    }
}

fn build_router(state: AppState) -> Router {
    Router::new()
        .route(
            "/sessions/:id/webrtc/offer",
            post(post_offer).get(get_offer),
        )
        .route(
            "/sessions/:id/webrtc/answer",
            post(post_answer).get(get_answer),
        )
        .route("/ws/:id", get(ws_handler))
        .with_state(state)
}

async fn post_offer(
    State(state): State<AppState>,
    Path(session): Path<String>,
    Json(payload): Json<TestSdpPayload>,
) -> StatusCode {
    if session != SESSION_ID {
        return StatusCode::NOT_FOUND;
    }
    debug!("stub: received offer for session {session}");
    let mut guard = state.rest.lock().await;
    guard.offer = Some(serde_json::to_vec(&payload).unwrap_or_default());
    StatusCode::NO_CONTENT
}

async fn get_offer(
    State(state): State<AppState>,
    Path(session): Path<String>,
) -> Result<Json<TestSdpPayload>, StatusCode> {
    if session != SESSION_ID {
        return Err(StatusCode::NOT_FOUND);
    }
    debug!("stub: fetching offer for session {session}");
    let guard = state.rest.lock().await;
    guard
        .offer
        .as_ref()
        .map(|bytes| {
            serde_json::from_slice::<TestSdpPayload>(bytes)
                .map(Json)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        })
        .unwrap_or(Err(StatusCode::NOT_FOUND))
}

async fn post_answer(
    State(state): State<AppState>,
    Path(session): Path<String>,
    Json(payload): Json<TestSdpPayload>,
) -> StatusCode {
    if session != SESSION_ID {
        return StatusCode::NOT_FOUND;
    }
    debug!("stub: received answer for session {session}");
    let mut guard = state.rest.lock().await;
    guard.answer = Some(serde_json::to_vec(&payload).unwrap_or_default());
    StatusCode::NO_CONTENT
}

async fn get_answer(
    State(state): State<AppState>,
    Path(session): Path<String>,
) -> Result<Json<TestSdpPayload>, StatusCode> {
    if session != SESSION_ID {
        return Err(StatusCode::NOT_FOUND);
    }
    debug!("stub: fetching answer for session {session}");
    let guard = state.rest.lock().await;
    guard
        .answer
        .as_ref()
        .map(|bytes| {
            serde_json::from_slice::<TestSdpPayload>(bytes)
                .map(Json)
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)
        })
        .unwrap_or(Err(StatusCode::NOT_FOUND))
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    Path(session): Path<String>,
) -> impl IntoResponse {
    if session != SESSION_ID {
        return (StatusCode::NOT_FOUND, "unknown session").into_response();
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

    let base_url = format!("http://{}/sessions/{}/webrtc", addr, SESSION_ID);
    let offer_fut = connect_via_signaling(
        &base_url,
        WebRtcRole::Offerer,
        Duration::from_millis(50),
        None,
    );
    let answer_fut = connect_via_signaling(
        &base_url,
        WebRtcRole::Answerer,
        Duration::from_millis(50),
        None,
    );

    let (offer_res, answer_res) = tokio::join!(
        timeout(Duration::from_secs(10), offer_fut),
        timeout(Duration::from_secs(10), answer_fut),
    );
    let offer_transport = offer_res
        .expect("offer signaling timeout")
        .expect("offer transport");
    let answer_transport = answer_res
        .expect("answer signaling timeout")
        .expect("answer transport");

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
