use std::sync::Arc;

use bytes::Bytes;
use hkdf::Hkdf;
use sha2::Sha256;
use snow::Builder as NoiseBuilder;
use snow::params::NoiseParams;
use tokio::sync::{Notify, mpsc as tokio_mpsc};
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_init::RTCDataChannelInit;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::data_channel::data_channel_state::RTCDataChannelState;

use crate::transport::TransportError;

use super::secure_signaling::derive_pre_shared_key;

pub const HANDSHAKE_CHANNEL_LABEL: &str = "beach-secure-handshake";
const INSECURE_OVERRIDE_TOKEN: &str = "I_KNOW_THIS_IS_UNSAFE";

#[derive(Clone, Copy, Debug)]
pub enum HandshakeRole {
    Initiator,
    Responder,
}

#[derive(Clone, Debug)]
pub struct HandshakeResult {
    pub send_key: [u8; 32],
    pub recv_key: [u8; 32],
    pub verification_code: String,
}

#[derive(Clone, Debug)]
pub struct HandshakeParams {
    pub passphrase: String,
    pub handshake_id: String,
    pub local_peer_id: String,
    pub remote_peer_id: String,
    pub prologue_context: Vec<u8>,
}

pub fn secure_transport_enabled() -> bool {
    !insecure_transport_override()
}

pub fn handshake_channel_init() -> RTCDataChannelInit {
    let mut init = RTCDataChannelInit::default();
    init.ordered = Some(true);
    init
}

pub async fn run_handshake(
    role: HandshakeRole,
    channel: Arc<RTCDataChannel>,
    params: HandshakeParams,
) -> Result<HandshakeResult, TransportError> {
    tracing::info!(
        target = "webrtc",
        ?role,
        handshake_id = %params.handshake_id,
        local_peer = %params.local_peer_id,
        remote_peer = %params.remote_peer_id,
        channel_state = ?channel.ready_state(),
        "starting secure handshake, waiting for channel open"
    );
    wait_for_channel_open(&channel).await?;
    tracing::info!(
        target = "webrtc",
        ?role,
        handshake_id = %params.handshake_id,
        "handshake channel is now open, proceeding with noise protocol"
    );

    let (incoming_tx, mut incoming_rx) = tokio_mpsc::unbounded_channel::<Vec<u8>>();
    channel.on_message(Box::new(move |msg: DataChannelMessage| {
        let sender = incoming_tx.clone();
        Box::pin(async move {
            if sender.send(msg.data.to_vec()).is_err() {
                tracing::debug!(target = "webrtc", "secure handshake message channel closed");
            }
        })
    }));

    let psk = derive_pre_shared_key(&params.passphrase, &params.handshake_id)?;
    let mut prologue = Vec::with_capacity(params.prologue_context.len() + 32);
    prologue.extend_from_slice(b"beach:secure-handshake:v1");
    prologue.push(0x1f);
    prologue.extend_from_slice(params.prologue_context.as_slice());

    let noise_params: NoiseParams = "Noise_XXpsk2_25519_ChaChaPoly_BLAKE2s"
        .parse()
        .map_err(|err| TransportError::Setup(format!("invalid noise params: {err}")))?;
    let builder = NoiseBuilder::new(noise_params)
        .psk(2, &psk)
        .prologue(&prologue);
    let keypair = builder.generate_keypair().map_err(map_noise_error)?;
    let builder = builder.local_private_key(&keypair.private);
    let mut state = match role {
        HandshakeRole::Initiator => builder.build_initiator().map_err(map_noise_error)?,
        HandshakeRole::Responder => builder.build_responder().map_err(map_noise_error)?,
    };

    let mut send_buf = vec![0u8; 1024];
    if matches!(role, HandshakeRole::Initiator) {
        let len = state
            .write_message(&[], &mut send_buf)
            .map_err(map_noise_error)?;
        let payload = Bytes::copy_from_slice(&send_buf[..len]);
        channel
            .send(&payload)
            .await
            .map_err(|err| TransportError::Setup(format!("secure handshake send failed: {err}")))?;
    }

    while !state.is_handshake_finished() {
        let incoming = incoming_rx
            .recv()
            .await
            .ok_or_else(|| TransportError::Setup("secure handshake channel closed".into()))?;
        state
            .read_message(&incoming, &mut send_buf)
            .map_err(map_noise_error)?;
        if state.is_handshake_finished() {
            break;
        }
        let len = state
            .write_message(&[], &mut send_buf)
            .map_err(map_noise_error)?;
        let payload = Bytes::copy_from_slice(&send_buf[..len]);
        channel
            .send(&payload)
            .await
            .map_err(|err| TransportError::Setup(format!("secure handshake send failed: {err}")))?;
    }

    let handshake_hash = state.get_handshake_hash().to_vec();
    state.into_transport_mode().map_err(map_noise_error)?;

    channel.on_message(Box::new(|_| Box::pin(async {})));
    let result = derive_session_material(
        &psk,
        &handshake_hash,
        &params.local_peer_id,
        &params.remote_peer_id,
        role,
    )?;

    tracing::info!(
        target = "webrtc",
        handshake_id = %params.handshake_id,
        peer = %params.remote_peer_id,
        verification = %result.verification_code,
        "secure transport handshake established"
    );

    Ok(result)
}

pub fn build_prologue_context(handshake_id: &str, local_peer: &str, remote_peer: &str) -> Vec<u8> {
    let mut peers = [local_peer.to_string(), remote_peer.to_string()];
    peers.sort();
    let mut context = Vec::with_capacity(handshake_id.len() + peers[0].len() + peers[1].len() + 2);
    context.extend_from_slice(handshake_id.as_bytes());
    context.push(0x1f);
    context.extend_from_slice(peers[0].as_bytes());
    context.push(0x1f);
    context.extend_from_slice(peers[1].as_bytes());
    context
}

fn derive_session_material(
    psk: &[u8],
    handshake_hash: &[u8],
    local_peer: &str,
    remote_peer: &str,
    role: HandshakeRole,
) -> Result<HandshakeResult, TransportError> {
    let hkdf = Hkdf::<Sha256>::new(Some(psk), handshake_hash);

    let send_label = format!(
        "beach:secure-transport:direction:{}->{}",
        local_peer, remote_peer
    );
    let recv_label = format!(
        "beach:secure-transport:direction:{}->{}",
        remote_peer, local_peer
    );
    let mut send_material = [0u8; 32];
    let mut recv_material = [0u8; 32];
    hkdf.expand(send_label.as_bytes(), &mut send_material)
        .map_err(|err| TransportError::Setup(format!("secure transport hkdf failed: {err}")))?;
    hkdf.expand(recv_label.as_bytes(), &mut recv_material)
        .map_err(|err| TransportError::Setup(format!("secure transport hkdf failed: {err}")))?;

    let mut verify_pair = [local_peer.to_string(), remote_peer.to_string()];
    verify_pair.sort();
    let verify_label = format!(
        "beach:secure-transport:verify:{}|{}",
        verify_pair[0], verify_pair[1]
    );
    let mut verify_bytes = [0u8; 4];
    hkdf.expand(verify_label.as_bytes(), &mut verify_bytes)
        .map_err(|err| TransportError::Setup(format!("secure transport hkdf failed: {err}")))?;
    let code = u32::from_le_bytes(verify_bytes) % 1_000_000;
    let verification_code = format!("{code:06}");

    let (send_key, recv_key) = match role {
        HandshakeRole::Initiator => (send_material, recv_material),
        HandshakeRole::Responder => (send_material, recv_material),
    };

    Ok(HandshakeResult {
        send_key,
        recv_key,
        verification_code,
    })
}

async fn wait_for_channel_open(channel: &RTCDataChannel) -> Result<(), TransportError> {
    let initial_state = channel.ready_state();
    tracing::debug!(
        target = "webrtc",
        ?initial_state,
        "wait_for_channel_open called"
    );
    if initial_state == RTCDataChannelState::Open {
        tracing::debug!(
            target = "webrtc",
            "channel already open, returning immediately"
        );
        return Ok(());
    }
    let notify = Arc::new(Notify::new());
    let notify_clone = Arc::clone(&notify);
    channel.on_open(Box::new(move || {
        let notify = Arc::clone(&notify_clone);
        Box::pin(async move {
            tracing::debug!(
                target = "webrtc",
                "handshake channel on_open callback fired"
            );
            notify.notify_waiters();
            notify.notify_one();
        })
    }));
    tracing::debug!(
        target = "webrtc",
        "registered on_open handler, awaiting notification"
    );
    notify.notified().await;
    let final_state = channel.ready_state();
    tracing::debug!(
        target = "webrtc",
        ?final_state,
        "channel open notification received"
    );
    Ok(())
}

fn map_noise_error(err: snow::Error) -> TransportError {
    TransportError::Setup(format!("secure handshake noise error: {err}"))
}

fn insecure_transport_override() -> bool {
    matches!(
        std::env::var("BEACH_INSECURE_TRANSPORT")
            .ok()
            .map(|value| value.trim().eq(INSECURE_OVERRIDE_TOKEN)),
        Some(true)
    )
}
