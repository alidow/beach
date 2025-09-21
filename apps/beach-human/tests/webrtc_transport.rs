use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use beach_human::transport::webrtc::{WebRtcRole, connect_via_signaling, create_test_pair};
use beach_human::transport::{Payload, Transport, TransportKind, TransportMessage};
use hyper::body::to_bytes;
use hyper::service::{make_service_fn, service_fn};
use hyper::{Body, Method, Request, Response, Server, StatusCode};
use tokio::sync::{Mutex as AsyncMutex, oneshot};
use tokio::time::{Instant, sleep};
use tracing_subscriber::{EnvFilter, fmt::SubscriberBuilder};
use webrtc::ice_transport::ice_candidate::RTCIceCandidateInit;

fn payload_text(message: TransportMessage) -> Option<String> {
    match message.payload {
        Payload::Text(text) => Some(text),
        Payload::Binary(_) => None,
    }
}

fn payload_bytes(message: TransportMessage) -> Vec<u8> {
    match message.payload {
        Payload::Binary(bytes) => bytes,
        Payload::Text(text) => text.into_bytes(),
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

#[tokio::test]
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
    assert_eq!(
        payload_text(server_msg).as_deref(),
        Some("hello from client")
    );

    server
        .send_text("hello from server")
        .expect("send server text");
    let client_msg = recv_with_timeout(&client, Duration::from_secs(5)).await;
    assert_eq!(
        payload_text(client_msg).as_deref(),
        Some("hello from server")
    );

    let bytes = vec![1u8, 2, 3, 4, 5];
    server.send_bytes(&bytes).expect("send server binary");
    let binary_msg = recv_with_timeout(&client, Duration::from_secs(5)).await;
    assert_eq!(payload_bytes(binary_msg), bytes);

    for idx in 0..10 {
        client
            .send_text(&format!("msg-{idx}"))
            .expect("batched send");
    }
    for idx in 0..10 {
        let expected = format!("msg-{idx}");
        let received = recv_with_timeout(&server, Duration::from_secs(5)).await;
        assert_eq!(payload_text(received).as_deref(), Some(expected.as_str()));
    }
}

struct SignalingState {
    offer: Option<Vec<u8>>,
    answer: Option<Vec<u8>>,
    offer_candidates: Vec<Option<RTCIceCandidateInit>>,
    answer_candidates: Vec<Option<RTCIceCandidateInit>>,
}

impl SignalingState {
    fn new() -> Self {
        Self {
            offer: None,
            answer: None,
            offer_candidates: Vec::new(),
            answer_candidates: Vec::new(),
        }
    }
}

async fn handle_signaling_request(
    req: Request<Body>,
    state: Arc<AsyncMutex<SignalingState>>,
) -> Result<Response<Body>, Infallible> {
    let path = req.uri().path().to_string();
    let method = req.method().clone();
    let mut guard = state.lock().await;
    let response = match (method, path.as_str()) {
        (Method::POST, "/offer") => {
            let body = to_bytes(req.into_body()).await.unwrap_or_default();
            guard.offer = Some(body.to_vec());
            Response::new(Body::empty())
        }
        (Method::GET, "/offer") => match &guard.offer {
            Some(payload) => Response::new(Body::from(payload.clone())),
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty())
                .unwrap(),
        },
        (Method::POST, "/answer") => {
            let body = to_bytes(req.into_body()).await.unwrap_or_default();
            guard.answer = Some(body.to_vec());
            Response::new(Body::empty())
        }
        (Method::POST, "/offer/candidates") => {
            let body = to_bytes(req.into_body()).await.unwrap_or_default();
            match serde_json::from_slice::<Option<RTCIceCandidateInit>>(&body) {
                Ok(candidate) => {
                    guard.offer_candidates.push(candidate);
                    Response::new(Body::empty())
                }
                Err(_) => Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Body::empty())
                    .unwrap(),
            }
        }
        (Method::GET, "/offer/candidates") => {
            let candidates = std::mem::take(&mut guard.offer_candidates);
            let body = serde_json::to_vec(&candidates).unwrap_or_default();
            Response::new(Body::from(body))
        }
        (Method::POST, "/answer/candidates") => {
            let body = to_bytes(req.into_body()).await.unwrap_or_default();
            match serde_json::from_slice::<Option<RTCIceCandidateInit>>(&body) {
                Ok(candidate) => {
                    guard.answer_candidates.push(candidate);
                    Response::new(Body::empty())
                }
                Err(_) => Response::builder()
                    .status(StatusCode::BAD_REQUEST)
                    .body(Body::empty())
                    .unwrap(),
            }
        }
        (Method::GET, "/answer/candidates") => {
            let candidates = std::mem::take(&mut guard.answer_candidates);
            let body = serde_json::to_vec(&candidates).unwrap_or_default();
            Response::new(Body::from(body))
        }
        (Method::GET, "/answer") => match &guard.answer {
            Some(payload) => Response::new(Body::from(payload.clone())),
            None => Response::builder()
                .status(StatusCode::NOT_FOUND)
                .body(Body::empty())
                .unwrap(),
        },
        _ => Response::builder()
            .status(StatusCode::NOT_FOUND)
            .body(Body::empty())
            .unwrap(),
    };
    Ok(response)
}

#[tokio::test]
async fn webrtc_signaling_end_to_end() {
    let _ = SubscriberBuilder::default().with_test_writer().try_init();
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(true).expect("set nonblocking");
    let addr = listener.local_addr().expect("local addr");
    let state = Arc::new(AsyncMutex::new(SignalingState::new()));
    let (shutdown_tx, shutdown_rx) = oneshot::channel();

    let server_state = state.clone();
    tokio::spawn(
        Server::from_tcp(listener)
            .expect("server from tcp")
            .serve(make_service_fn(move |_conn| {
                let state = server_state.clone();
                async move {
                    Ok::<_, Infallible>(service_fn(move |req| {
                        let state = state.clone();
                        handle_signaling_request(req, state)
                    }))
                }
            }))
            .with_graceful_shutdown(async {
                let _ = shutdown_rx.await;
            }),
    );

    let base_url = format!("http://{}", addr);
    let offer_fut =
        connect_via_signaling(&base_url, WebRtcRole::Offerer, Duration::from_millis(50));
    let answer_fut =
        connect_via_signaling(&base_url, WebRtcRole::Answerer, Duration::from_millis(50));

    let (offer_res, answer_res) = tokio::join!(offer_fut, answer_fut);
    let offer_transport = offer_res.expect("offer transport");
    let answer_transport = answer_res.expect("answer transport");

    offer_transport.send_text("ping").expect("offer send");
    let pong = loop {
        let message = answer_transport
            .recv(Duration::from_secs(5))
            .expect("answer recv");
        if payload_text(message.clone()).as_deref() == Some("__offer_ready__") {
            continue;
        }
        break message;
    };
    assert_eq!(payload_text(pong).as_deref(), Some("ping"));

    answer_transport.send_text("pong").expect("answer send");
    let ping = offer_transport
        .recv(Duration::from_secs(5))
        .expect("offer recv");
    assert_eq!(payload_text(ping).as_deref(), Some("pong"));

    shutdown_tx.send(()).ok();
}
