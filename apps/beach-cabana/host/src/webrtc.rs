use std::time::{Duration, Instant};
use thiserror::Error;
use serde::{Serialize, Deserialize};
#[cfg(target_os = "macos")]
use std::time::SystemTime;

use crate::noise::{HandshakeConfig, NoiseController, NoiseError};
use crate::security::{HandshakeId, SessionMaterial};
#[cfg(target_os = "macos")]
use crate::capture::{self, Frame, PixelFormat};

#[derive(Serialize, Deserialize, Debug)]
struct RoadSealedEnvelope<'a> {
    version: u32,
    nonce: &'a str,
    ciphertext: &'a str,
}

#[derive(Serialize, Deserialize, Debug)]
struct RoadSdpPayload<'a> {
    sdp: &'a str,
    #[serde(rename = "type")]
    typ: &'a str,
    handshake_id: &'a str,
    from_peer: &'a str,
    to_peer: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    sealed: Option<RoadSealedEnvelope<'a>>,
}

fn default_road_base(url: &Option<String>) -> String {
    url.clone().unwrap_or_else(|| "http://127.0.0.1:8080".to_string())
}

use std::sync::{Arc, Mutex};
use bytes::Bytes;
use tokio::{runtime::Handle, sync::{mpsc, oneshot}, task, time};
use webrtc::data_channel::{data_channel_message::DataChannelMessage, data_channel_state::RTCDataChannelState, RTCDataChannel};
use webrtc::ice_transport::ice_gathering_state::RTCIceGatheringState;
use webrtc::peer_connection::sdp::sdp_type::RTCSdpType;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::api::{interceptor_registry::register_default_interceptors, media_engine::MediaEngine, APIBuilder};
use webrtc::interceptor::registry::Registry;
use webrtc::ice_transport::{ice_candidate::RTCIceCandidateInit, ice_server::RTCIceServer};
use webrtc::peer_connection::configuration::RTCConfiguration;

#[allow(dead_code)]
struct DataChannelAdapter {
    channel: Arc<RTCDataChannel>,
    handle: Handle,
    receiver: mpsc::UnboundedReceiver<Option<Vec<u8>>>,
}

impl DataChannelAdapter {
    fn new(channel: Arc<RTCDataChannel>, handle: Handle) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let message_tx = tx.clone();
        channel.on_message(Box::new(move |msg: DataChannelMessage| {
            let sender = message_tx.clone();
            Box::pin(async move { let _ = sender.send(Some(msg.data.to_vec())); })
        }));
        let close_tx = tx.clone();
        channel.on_close(Box::new(move || { let _ = close_tx.send(None); Box::pin(async {}) }));
        Self { channel, handle, receiver: rx }
    }
}

impl CabanaChannel for DataChannelAdapter {
    fn send(&mut self, payload: &[u8]) -> Result<(), NoiseDriverError> {
        let channel = self.channel.clone();
        let data = Bytes::from(payload.to_vec());
        self.handle.block_on(async move { channel.send(&data).await.map(|_| ()).map_err(|e| NoiseDriverError::ChannelSend(e.to_string())) })
    }

    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, NoiseDriverError> {
        self.handle.block_on(async { match time::timeout(timeout, self.receiver.recv()).await { Ok(Some(Some(payload))) => Ok(payload), Ok(Some(None)) => Err(NoiseDriverError::ChannelClosed), Ok(None) => Err(NoiseDriverError::ChannelClosed), Err(_) => Err(NoiseDriverError::Timeout) } })
    }
}

pub struct DataChannelSecureTransport {
    driver: Arc<Mutex<NoiseDriver<DataChannelAdapter>>>,
    verification_code: Option<String>,
}

impl DataChannelSecureTransport {
    pub fn verification_code(&self) -> Option<&str> { self.verification_code.as_deref() }
    pub async fn send_media(&self, payload: &[u8]) -> Result<(), NoiseDriverError> { let driver = self.driver.clone(); let data = payload.to_vec(); task::spawn_blocking(move || { let mut guard = driver.lock().unwrap(); guard.send_media(&data) }).await.map_err(|e| NoiseDriverError::Join(e.to_string()))? }
    pub async fn recv_media(&self, timeout: Duration) -> Result<Vec<u8>, NoiseDriverError> { let driver = self.driver.clone(); task::spawn_blocking(move || { let mut guard = driver.lock().unwrap(); guard.recv_media(timeout) }).await.map_err(|e| NoiseDriverError::Join(e.to_string()))? }
}

async fn wait_for_channel_open(channel: &Arc<RTCDataChannel>, timeout: Duration) -> Result<Duration, NoiseDriverError> {
    if channel.ready_state() == RTCDataChannelState::Open { return Ok(timeout); }
    let (tx, rx) = oneshot::channel();
    channel.on_open(Box::new(move || { let _ = tx.send(()); Box::pin(async {}) }));
    let start = Instant::now();
    time::timeout(timeout, rx).await.map_err(|_| NoiseDriverError::Timeout)?.map_err(|_| NoiseDriverError::ChannelClosed)?;
    Ok(timeout.saturating_sub(start.elapsed()))
}

pub async fn run_local_webrtc_noise_demo(
    material: crate::security::SessionMaterial,
    handshake: crate::security::HandshakeId,
    host_id: String,
    viewer_id: String,
    prologue: Vec<u8>,
) -> Result<String, NoiseDriverError> {
    let mut m = MediaEngine::default();
    m.register_default_codecs().map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m).map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    let api = APIBuilder::new().with_media_engine(m).with_interceptor_registry(registry).build();
    let config = RTCConfiguration { ice_servers: vec![RTCIceServer { urls: vec!["stun:stun.l.google.com:19302".to_string()], ..Default::default() }], ..Default::default() };
    let host_pc = Arc::new(api.new_peer_connection(config.clone()).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?);
    let viewer_pc = Arc::new(api.new_peer_connection(config).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?);
    let (h2v_tx, mut h2v_rx) = mpsc::unbounded_channel::<RTCIceCandidateInit>();
    let (v2h_tx, mut v2h_rx) = mpsc::unbounded_channel::<RTCIceCandidateInit>();
    { let _v_pc = viewer_pc.clone(); host_pc.on_ice_candidate(Box::new(move |c| { let tx = h2v_tx.clone(); Box::pin(async move { if let Some(c) = c { if let Ok(json) = c.to_json() { let _ = tx.send(json); } } }) })); }
    { let _h_pc = host_pc.clone(); viewer_pc.on_ice_candidate(Box::new(move |c| { let tx = v2h_tx.clone(); Box::pin(async move { if let Some(c) = c { if let Ok(json) = c.to_json() { let _ = tx.send(json); } } }) })); }
    { let v_pc = viewer_pc.clone(); tokio::spawn(async move { while let Some(c) = h2v_rx.recv().await { let _ = v_pc.add_ice_candidate(c).await; } }); }
    { let h_pc = host_pc.clone(); tokio::spawn(async move { while let Some(c) = v2h_rx.recv().await { let _ = h_pc.add_ice_candidate(c).await; } }); }
    let dc = host_pc.create_data_channel("cabana", None).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    let dc = Arc::new(dc);
    let offer = host_pc.create_offer(None).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    host_pc.set_local_description(offer).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    wait_ice_complete(&host_pc, Duration::from_secs(10)).await?;
    let offer = host_pc.local_description().await.ok_or_else(|| NoiseDriverError::UnexpectedFrame)?;
    viewer_pc.set_remote_description(offer).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    let ans = viewer_pc.create_answer(None).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    viewer_pc.set_local_description(ans).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    wait_ice_complete(&viewer_pc, Duration::from_secs(10)).await?;
    let ans = viewer_pc.local_description().await.ok_or_else(|| NoiseDriverError::UnexpectedFrame)?;
    host_pc.set_remote_description(ans).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    let remaining = wait_for_channel_open(&dc, Duration::from_secs(10)).await?;

    let adapter = DataChannelAdapter::new(dc.clone(), Handle::current());
    let mut driver = NoiseDriver::new(adapter, HandshakeConfig { material: &material, handshake_id: &handshake, role: crate::noise::HandshakeRole::Initiator, local_id: &host_id, remote_id: &viewer_id, prologue_context: &prologue })?;
    task::spawn_blocking(move || -> Result<NoiseDriver<DataChannelAdapter>, NoiseDriverError> { driver.run_handshake(remaining)?; Ok(driver) }).await.map_err(|e| NoiseDriverError::Join(e.to_string()))??;
    Ok("000000".to_string())
}

async fn wait_ice_complete(pc: &webrtc::peer_connection::RTCPeerConnection, timeout: Duration) -> Result<(), NoiseDriverError> {
    let deadline = Instant::now() + timeout;
    loop {
        let state = pc.ice_gathering_state().await;
        if state == RTCIceGatheringState::Complete { return Ok(()); }
        if Instant::now() >= deadline { return Err(NoiseDriverError::Timeout); }
        time::sleep(Duration::from_millis(100)).await;
    }
}

async fn build_api() -> Result<webrtc::api::API, NoiseDriverError> {
    let mut m = MediaEngine::default();
    m.register_default_codecs().map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    let mut registry = Registry::new();
    registry = register_default_interceptors(registry, &mut m).map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    Ok(APIBuilder::new().with_media_engine(m).with_interceptor_registry(registry).build())
}

pub async fn host_run(
    session_id: String,
    passcode: String,
    codec: EncodeCodec,
    road_url: Option<String>,
    fixture_url: Option<String>,
    fixture_dir: Option<std::path::PathBuf>,
    prologue: Vec<u8>,
    window_id: Option<String>,
    frames: u32,
    interval_ms: u64,
    max_width: Option<u32>,
    from_id: String,
    to_id: String,
) -> Result<String, NoiseDriverError> {
    let api = build_api().await?;
    let config = RTCConfiguration { ice_servers: vec![], ..Default::default() };
    let pc = Arc::new(api.new_peer_connection(config).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?);
    let dc = pc.create_data_channel("cabana", None).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    let offer = pc.create_offer(None).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    pc.set_local_description(offer).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    wait_ice_complete(&pc, Duration::from_secs(10)).await?;
    let sdp = pc.local_description().await.ok_or_else(|| NoiseDriverError::UnexpectedFrame)?.sdp;
    let material = SessionMaterial::derive(&session_id, &passcode).map_err(|e| NoiseDriverError::Noise(NoiseError::Security(e)))?;
    let handshake = HandshakeId::random();
    let envelope = crate::security::seal_signaling_payload(&material, &handshake, sdp.as_bytes()).map_err(|e| NoiseDriverError::Noise(NoiseError::Security(e)))?;
    let handshake_b64 = handshake.to_base64();
    let road_base = default_road_base(&road_url);
    let payload = RoadSdpPayload { sdp: "", typ: "offer", handshake_id: &handshake_b64, from_peer: &from_id, to_peer: &to_id, sealed: Some(RoadSealedEnvelope { version: envelope.version as u32, nonce: &envelope.nonce_b64, ciphertext: &envelope.ciphertext_b64 }) };
    let post_url = format!("{}/sessions/{}/webrtc/offer", road_base.trim_end_matches('/'), session_id);
    let res = reqwest::blocking::Client::new().post(post_url).json(&payload).send().map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?;
    if !res.status().is_success() { return Err(NoiseDriverError::ChannelSend(format!("road offer POST failed: {}", res.status()))); }
    println!("Host: posted sealed offer to Beach Road (handshake {})", handshake_b64);
    println!("Passcode fingerprint: {}", material.passcode_fingerprint());
    let get_url = format!("{}/sessions/{}/webrtc/answer?handshake_id={}", road_base.trim_end_matches('/'), session_id, handshake_b64);
    let deadline = Instant::now() + Duration::from_secs(60);
    let answer_envelope = loop { if Instant::now() >= deadline { return Err(NoiseDriverError::Timeout); } let resp = reqwest::blocking::get(&get_url).map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; if resp.status().is_success() { let obj: serde_json::Value = resp.json().map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; if let Some(sealed) = obj.get("sealed") { let nonce = sealed.get("nonce").and_then(|v| v.as_str()).unwrap_or(""); let ciphertext = sealed.get("ciphertext").and_then(|v| v.as_str()).unwrap_or(""); let version = sealed.get("version").and_then(|v| v.as_u64()).unwrap_or(1) as u8; let env = crate::security::SealedEnvelope { version, handshake_b64: handshake_b64.clone(), nonce_b64: nonce.to_string(), ciphertext_b64: ciphertext.to_string() }; break env.compact_encoding(); } } std::thread::sleep(Duration::from_millis(500)); };
    let answer = crate::security::SealedEnvelope::from_compact(&answer_envelope).map_err(|e| NoiseDriverError::Noise(NoiseError::Security(e)))?;
    let answer_bytes = crate::security::open_signaling_payload(&material, &answer).map_err(|e| NoiseDriverError::Noise(NoiseError::Security(e)))?;
    let answer_sdp = String::from_utf8_lossy(&answer_bytes).to_string();
    let desc = RTCSessionDescription::answer(answer_sdp).map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    pc.set_remote_description(desc).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    let transport = negotiate_data_channel(Arc::new(dc), HandshakeConfig { material: &material, handshake_id: &handshake, role: crate::noise::HandshakeRole::Initiator, local_id: "host", remote_id: "viewer", prologue_context: &prologue }, Duration::from_secs(10)).await?;
    #[cfg(target_os = "macos")]
    if let (Some(target), n) = (window_id, frames) { if n > 0 { let interval = Duration::from_millis(interval_ms); match codec { EncodeCodec::Gif => { stream_png_frames(&transport, &target, n, interval, max_width).await?; } EncodeCodec::H264 => { #[cfg(all(target_os = "macos", feature = "cabana_sck"))] { let fps = (1000u64 / interval_ms.max(1)) as u32; stream_h264_frames(&transport, &target, n, interval, max_width, fps).await?; } #[cfg(not(all(target_os = "macos", feature = "cabana_sck")))] { stream_png_frames(&transport, &target, n, interval, max_width).await?; } } } } }
    Ok(transport.verification_code().unwrap_or("unknown").to_string())
}

pub async fn viewer_answer(
    session_id: String,
    passcode: String,
    host_envelope: String,
    fixture_url: Option<String>,
    _prologue: Vec<u8>,
) -> Result<String, NoiseDriverError> {
    let api = build_api().await?;
    let config = RTCConfiguration { ice_servers: vec![], ..Default::default() };
    let pc = Arc::new(api.new_peer_connection(config).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?);
    let material = SessionMaterial::derive(&session_id, &passcode).map_err(|e| NoiseDriverError::Noise(NoiseError::Security(e)))?;
    let env = crate::security::SealedEnvelope::from_compact(&host_envelope).map_err(|e| NoiseDriverError::Noise(NoiseError::Security(e)))?;
    let host_sdp = crate::security::open_signaling_payload(&material, &env).map_err(|e| NoiseDriverError::Noise(NoiseError::Security(e)))?;
    let host_sdp = String::from_utf8_lossy(&host_sdp).to_string();
    let desc = RTCSessionDescription::offer(host_sdp).map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    pc.set_remote_description(desc).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    let answer = pc.create_answer(None).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    pc.set_local_description(answer).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    wait_ice_complete(&pc, Duration::from_secs(10)).await?;
    let sdp = pc.local_description().await.ok_or_else(|| NoiseDriverError::UnexpectedFrame)?.sdp;
    let handshake = crate::security::HandshakeId::from_base64(&env.handshake_b64).map_err(|e| NoiseDriverError::Noise(NoiseError::Security(e)))?;
    let viewer_env = crate::security::seal_signaling_payload(&material, &handshake, sdp.as_bytes()).map_err(|e| NoiseDriverError::Noise(NoiseError::Security(e)))?;
    let compact = viewer_env.compact_encoding();
    if let Some(url) = fixture_url.as_deref() {
        let _ = crate::fixture::client::post_envelope(url, crate::fixture::client::FixtureEnvelope { session_id: &session_id, handshake_b64: &handshake.to_base64(), envelope: &compact }).map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?;
        Ok("posted-viewer-answer".into())
    } else {
        println!("Viewer sealed answer:\n{}", compact);
        Ok(compact)
    }
}

#[cfg(target_os = "macos")]
fn to_rgba(frame: &mut Frame) { if let PixelFormat::Bgra8888 = frame.pixel_format { for chunk in frame.data.chunks_mut(4) { chunk.swap(0, 2); } frame.pixel_format = PixelFormat::Rgba8888; } }
#[cfg(target_os = "macos")]
fn png_bytes_from_frame(frame: &Frame) -> anyhow::Result<Vec<u8>> { use image::{codecs::png::PngEncoder, ColorType, ImageBuffer}; let Some(buffer) = ImageBuffer::<image::Rgba<u8>, _>::from_vec(frame.width, frame.height, frame.data.clone()) else { anyhow::bail!("failed to build RGBA buffer for PNG"); }; let mut bytes = Vec::with_capacity((frame.width * frame.height) as usize); let encoder = PngEncoder::new(&mut bytes); encoder.encode(&buffer, frame.width, frame.height, ColorType::Rgba8)?; Ok(bytes) }
#[cfg(target_os = "macos")]
fn resize_frame(frame: &mut Frame, max_width: Option<u32>) -> anyhow::Result<()> { if let Some(max_w) = max_width { if max_w > 0 && frame.width > max_w { let Some(buffer) = image::ImageBuffer::<image::Rgba<u8>, _>::from_vec(frame.width, frame.height, frame.data.clone()) else { anyhow::bail!("failed to create buffer for resize"); }; let new_h = ((frame.height as f32 * (max_w as f32 / frame.width as f32)).round() as u32).max(1); let resized = image::DynamicImage::ImageRgba8(buffer).resize(max_w, new_h, image::imageops::FilterType::Lanczos3).to_rgba8(); frame.width = resized.width(); frame.height = resized.height(); frame.data = resized.into_raw(); } } Ok(()) }
#[cfg(target_os = "macos")]
fn encode_png_message(png: &[u8]) -> Vec<u8> { let mut out = Vec::with_capacity(1 + 4 + png.len()); out.push(1u8); out.extend_from_slice(&(png.len() as u32).to_be_bytes()); out.extend_from_slice(png); out }

#[cfg(all(target_os = "macos", feature = "webrtc"))]
async fn stream_png_frames(transport: &DataChannelSecureTransport, target: &str, frames: u32, interval: Duration, max_width: Option<u32>) -> Result<(), NoiseDriverError> {
    let mut producer = match capture::create_producer(target) { Ok(p) => p, Err(err) => { return Err(NoiseDriverError::ChannelSend(format!("capture init failed: {}", err))); } };
    producer.start().map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?;
    for index in 0..frames { let mut frame = match producer.next_frame() { Ok(f) => f, Err(err) => { producer.stop(); return Err(NoiseDriverError::ChannelSend(format!("capture frame {} failed: {}", index, err))); } }; to_rgba(&mut frame); if let Err(e) = resize_frame(&mut frame, max_width) { producer.stop(); return Err(NoiseDriverError::ChannelSend(format!("resize failed: {}", e))); } let png = png_bytes_from_frame(&frame).map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; let msg = encode_png_message(&png); transport.send_media(&msg).await.map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; if index + 1 < frames && !interval.is_zero() { std::thread::sleep(interval); } } producer.stop(); Ok(()) }

#[cfg(all(target_os = "macos", feature = "cabana_sck", feature = "webrtc"))]
async fn stream_h264_frames(transport: &DataChannelSecureTransport, target: &str, frames: u32, interval: Duration, max_width: Option<u32>, fps: u32) -> Result<(), NoiseDriverError> { use crossbeam_channel as cb; use crate::encoder::VideoEncoder as _; use crate::encoder::VideoToolboxEncoder; let mut producer = match capture::create_producer(target) { Ok(p) => p, Err(err) => return Err(NoiseDriverError::ChannelSend(err.to_string())), }; producer.start().map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; let mut first = producer.next_frame().map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; to_rgba(&mut first); resize_frame(&mut first, max_width).map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; let (tx, rx) = cb::unbounded::<Vec<u8>>(); let mut vt = VideoToolboxEncoder::new_with_chunks(None, first.width, first.height, fps.max(1), Some(tx)).map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; vt.write_frame(&first).map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; let mut sent = 1u32; while let Ok(chunk) = rx.try_recv() { let mut msg = Vec::with_capacity(1 + 4 + chunk.len()); msg.push(2u8); msg.extend_from_slice(&(chunk.len() as u32).to_be_bytes()); msg.extend_from_slice(&chunk); transport.send_media(&msg).await.map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; } for _ in sent..frames { let mut frame = match producer.next_frame() { Ok(f) => f, Err(err) => { producer.stop(); return Err(NoiseDriverError::ChannelSend(err.to_string())); } }; to_rgba(&mut frame); if let Err(e) = resize_frame(&mut frame, max_width) { producer.stop(); return Err(NoiseDriverError::ChannelSend(e.to_string())); } vt.write_frame(&frame).map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; while let Ok(chunk) = rx.try_recv() { let mut msg = Vec::with_capacity(1 + 4 + chunk.len()); msg.push(2u8); msg.extend_from_slice(&(chunk.len() as u32).to_be_bytes()); msg.extend_from_slice(&chunk); transport.send_media(&msg).await.map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; } if !interval.is_zero() { std::thread::sleep(interval); } } vt.finish().map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; while let Ok(chunk) = rx.try_recv() { let mut msg = Vec::with_capacity(1 + 4 + chunk.len()); msg.push(2u8); msg.extend_from_slice(&(chunk.len() as u32).to_be_bytes()); msg.extend_from_slice(&chunk); transport.send_media(&msg).await.map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; } producer.stop(); Ok(()) }

pub async fn viewer_run(
    session_id: String,
    passcode: String,
    host_envelope: String,
    fixture_url: Option<String>,
    prologue: Vec<u8>,
    recv_frames: u32,
    output_dir: Option<std::path::PathBuf>,
    road_url: Option<String>,
    from_id: String,
    to_id: String,
) -> Result<std::path::PathBuf, NoiseDriverError> {
    let api = build_api().await?;
    let config = RTCConfiguration { ice_servers: vec![], ..Default::default() };
    let pc = Arc::new(api.new_peer_connection(config).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?);
    let material = SessionMaterial::derive(&session_id, &passcode).map_err(|e| NoiseDriverError::Noise(NoiseError::Security(e)))?;
    let env = if host_envelope.is_empty() { let road_base = default_road_base(&road_url); let get_url = format!("{}/sessions/{}/webrtc/offer?peer_id={}", road_base.trim_end_matches('/'), session_id, urlencoding::encode(&from_id)); let resp = reqwest::blocking::get(&get_url).map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; if !resp.status().is_success() { return Err(NoiseDriverError::ChannelSend(format!("road offer GET failed: {}", resp.status()))); } let obj: serde_json::Value = resp.json().map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; let sealed = obj.get("sealed").ok_or_else(|| NoiseDriverError::UnexpectedFrame)?; let nonce = sealed.get("nonce").and_then(|v| v.as_str()).unwrap_or(""); let ciphertext = sealed.get("ciphertext").and_then(|v| v.as_str()).unwrap_or(""); let version = sealed.get("version").and_then(|v| v.as_u64()).unwrap_or(1) as u8; let handshake_id = obj.get("handshake_id").and_then(|v| v.as_str()).unwrap_or(""); crate::security::SealedEnvelope { version, handshake_b64: handshake_id.to_string(), nonce_b64: nonce.to_string(), ciphertext_b64: ciphertext.to_string() } } else { crate::security::SealedEnvelope::from_compact(&host_envelope).map_err(|e| NoiseDriverError::Noise(NoiseError::Security(e)))? };
    let host_sdp = crate::security::open_signaling_payload(&material, &env).map_err(|e| NoiseDriverError::Noise(NoiseError::Security(e)))?;
    let host_sdp = String::from_utf8_lossy(&host_sdp).to_string();
    let desc = RTCSessionDescription::offer(host_sdp).map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    pc.set_remote_description(desc).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    let answer = pc.create_answer(None).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    pc.set_local_description(answer).await.map_err(|e| NoiseDriverError::Noise(NoiseError::Handshake(e.to_string())))?;
    wait_ice_complete(&pc, Duration::from_secs(10)).await?;
    let sdp = pc.local_description().await.ok_or_else(|| NoiseDriverError::UnexpectedFrame)?.sdp;
    let handshake = HandshakeId::from_base64(&env.handshake_b64).map_err(|e| NoiseDriverError::Noise(NoiseError::Security(e)))?;
    let viewer_env = crate::security::seal_signaling_payload(&material, &handshake, sdp.as_bytes()).map_err(|e| NoiseDriverError::Noise(NoiseError::Security(e)))?;
    let compact = viewer_env.compact_encoding();
    let road_base = default_road_base(&road_url);
    let payload = RoadSdpPayload { sdp: "", typ: "answer", handshake_id: &handshake.to_base64(), from_peer: &from_id, to_peer: &to_id, sealed: Some(RoadSealedEnvelope { version: viewer_env.version as u32, nonce: &viewer_env.nonce_b64, ciphertext: &viewer_env.ciphertext_b64 }) };
    let post_url = format!("{}/sessions/{}/webrtc/answer", road_base.trim_end_matches('/'), session_id);
    let res = reqwest::blocking::Client::new().post(post_url).json(&payload).send().map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?;
    if !res.status().is_success() { return Err(NoiseDriverError::ChannelSend(format!("road answer POST failed: {}", res.status()))); }
    let (viewer_dc_tx, mut viewer_dc_rx) = mpsc::unbounded_channel::<Arc<RTCDataChannel>>();
    pc.on_data_channel(Box::new(move |dc: Arc<RTCDataChannel>| { let _ = viewer_dc_tx.send(dc); Box::pin(async {}) }));
    let channel = viewer_dc_rx.recv().await.ok_or_else(|| NoiseDriverError::ChannelClosed)?;
    let transport = negotiate_data_channel(channel, HandshakeConfig { material: &material, handshake_id: &handshake, role: crate::noise::HandshakeRole::Responder, local_id: "viewer", remote_id: "host", prologue_context: &prologue }, Duration::from_secs(10)).await?;
    let base_dir = if let Some(d) = output_dir { d } else { let ts = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs(); std::env::temp_dir().join(format!("cabana-viewer-{}", ts)) };
    std::fs::create_dir_all(&base_dir).map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?;
    let mut received = 0u32;
    while received < recv_frames { let payload = transport.recv_media(Duration::from_secs(10)).await.map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; if payload.len() < 5 { continue; } match payload[0] { 1 => { let mut len_bytes = [0u8; 4]; len_bytes.copy_from_slice(&payload[1..5]); let plen = u32::from_be_bytes(len_bytes) as usize; if payload.len() < 5 + plen { continue; } let data = &payload[5..5 + plen]; let path = base_dir.join(format!("frame_{:03}.png", received)); std::fs::write(&path, data).map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; received += 1; } 2 => { let mut len_bytes = [0u8; 4]; len_bytes.copy_from_slice(&payload[1..5]); let plen = u32::from_be_bytes(len_bytes) as usize; if payload.len() < 5 + plen { continue; } let data = &payload[5..5 + plen]; let path = base_dir.join("out.h264"); use std::io::Write as _; let mut f = std::fs::OpenOptions::new().create(true).append(true).open(&path).map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; f.write_all(data).map_err(|e| NoiseDriverError::ChannelSend(e.to_string()))?; received += 1; } _ => {} } }
    Ok(base_dir)
}

pub async fn negotiate_data_channel(channel: Arc<RTCDataChannel>, config: HandshakeConfig<'_>, timeout: Duration) -> Result<DataChannelSecureTransport, NoiseDriverError> {
    let remaining = wait_for_channel_open(&channel, timeout).await?;
    let adapter = DataChannelAdapter::new(channel, Handle::current());
    let mut driver = NoiseDriver::new(adapter, config)?;
    let driver = task::spawn_blocking(move || -> Result<NoiseDriver<DataChannelAdapter>, NoiseDriverError> { driver.run_handshake(remaining)?; Ok(driver) }).await.map_err(|err| NoiseDriverError::Join(err.to_string()))??;
    let verification_code = driver.verification_code().map(|code| code.to_string());
    let driver = Arc::new(Mutex::new(driver));
    Ok(DataChannelSecureTransport { driver, verification_code })
}
