use std::sync::Arc;

use bytes::Bytes;
use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use rand::RngCore;
use rand::rngs::OsRng;
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
const TRANSPORT_DIRECTION_PREFIX: &str = "beach:secure-transport:direction:";
const TRANSPORT_VERIFY_PREFIX: &str = "beach:secure-transport:verify:";
const TRANSPORT_CHALLENGE_KEY_PREFIX: &str = "beach:secure-transport:challenge-key:";
const TRANSPORT_CHALLENGE_MAC_PREFIX: &str = "beach:secure-transport:challenge-mac:";
const CHALLENGE_FRAME_VERSION: u8 = 1;
const CHALLENGE_NONCE_LENGTH: usize = 16;
const CHALLENGE_MAC_LENGTH: usize = 32;
const CHALLENGE_FRAME_LENGTH: usize = 1 + 1 + 6 + CHALLENGE_NONCE_LENGTH + CHALLENGE_MAC_LENGTH;

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

    let noise_params: NoiseParams = "Noise_XX_25519_ChaChaPoly_BLAKE2s"
        .parse()
        .map_err(|err| TransportError::Setup(format!("invalid noise params: {err}")))?;
    let builder = NoiseBuilder::new(noise_params).prologue(&prologue);
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

    let (result, challenge_key, challenge_context) = derive_session_material(
        &psk,
        &handshake_hash,
        &params.local_peer_id,
        &params.remote_peer_id,
        &params.handshake_id,
        role,
    )?;

    perform_verification_exchange(
        &channel,
        &mut incoming_rx,
        &params,
        role,
        &result.verification_code,
        &challenge_key,
        &challenge_context,
    )
    .await?;

    channel.on_message(Box::new(|_| Box::pin(async {})));

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
    handshake_id: &str,
    role: HandshakeRole,
) -> Result<(HandshakeResult, [u8; 32], Vec<u8>), TransportError> {
    let hkdf = Hkdf::<Sha256>::new(Some(psk), handshake_hash);

    let send_label = format!("{TRANSPORT_DIRECTION_PREFIX}{local_peer}->{remote_peer}");
    let recv_label = format!("{TRANSPORT_DIRECTION_PREFIX}{remote_peer}->{local_peer}");
    let mut send_material = [0u8; 32];
    let mut recv_material = [0u8; 32];
    hkdf.expand(send_label.as_bytes(), &mut send_material)
        .map_err(|err| TransportError::Setup(format!("secure transport hkdf failed: {err}")))?;
    hkdf.expand(recv_label.as_bytes(), &mut recv_material)
        .map_err(|err| TransportError::Setup(format!("secure transport hkdf failed: {err}")))?;

    let mut sorted_peers = [local_peer.to_string(), remote_peer.to_string()];
    sorted_peers.sort();
    let verify_label = format!(
        "{TRANSPORT_VERIFY_PREFIX}{}|{}",
        sorted_peers[0], sorted_peers[1]
    );
    let mut verify_bytes = [0u8; 4];
    hkdf.expand(verify_label.as_bytes(), &mut verify_bytes)
        .map_err(|err| TransportError::Setup(format!("secure transport hkdf failed: {err}")))?;
    let code = u32::from_le_bytes(verify_bytes) % 1_000_000;
    let verification_code = format!("{code:06}");

    let challenge_info = format!(
        "{TRANSPORT_CHALLENGE_KEY_PREFIX}{handshake_id}|{}|{}",
        sorted_peers[0], sorted_peers[1]
    );
    let mut challenge_key = [0u8; 32];
    hkdf.expand(challenge_info.as_bytes(), &mut challenge_key)
        .map_err(|err| TransportError::Setup(format!("secure transport hkdf failed: {err}")))?;
    let challenge_context = format!(
        "{TRANSPORT_CHALLENGE_MAC_PREFIX}{handshake_id}|{}|{}",
        sorted_peers[0], sorted_peers[1]
    )
    .into_bytes();

    let (send_key, recv_key) = match role {
        HandshakeRole::Initiator => (send_material, recv_material),
        HandshakeRole::Responder => (send_material, recv_material),
    };

    Ok((
        HandshakeResult {
            send_key,
            recv_key,
            verification_code,
        },
        challenge_key,
        challenge_context,
    ))
}

async fn perform_verification_exchange(
    channel: &Arc<RTCDataChannel>,
    incoming_rx: &mut tokio_mpsc::UnboundedReceiver<Vec<u8>>,
    params: &HandshakeParams,
    role: HandshakeRole,
    verification_code: &str,
    challenge_key: &[u8; 32],
    challenge_context: &[u8],
) -> Result<(), TransportError> {
    if verification_code.len() != 6 {
        return Err(TransportError::Setup(
            "secure handshake verification code invalid".into(),
        ));
    }

    let role_byte = match role {
        HandshakeRole::Initiator => 0u8,
        HandshakeRole::Responder => 1u8,
    };
    let expected_remote_role = match role {
        HandshakeRole::Initiator => 1u8,
        HandshakeRole::Responder => 0u8,
    };

    let mut frame = [0u8; CHALLENGE_FRAME_LENGTH];
    frame[0] = CHALLENGE_FRAME_VERSION;
    frame[1] = role_byte;
    frame[2..8].copy_from_slice(verification_code.as_bytes());
    let mut nonce = [0u8; CHALLENGE_NONCE_LENGTH];
    OsRng.fill_bytes(&mut nonce);
    frame[8..8 + CHALLENGE_NONCE_LENGTH].copy_from_slice(&nonce);

    let outbound_mac = compute_challenge_mac(
        challenge_key,
        challenge_context,
        role_byte,
        &frame[2..8],
        &frame[8..8 + CHALLENGE_NONCE_LENGTH],
    )?;
    frame[8 + CHALLENGE_NONCE_LENGTH..].copy_from_slice(&outbound_mac);

    let payload = Bytes::copy_from_slice(&frame);
    channel
        .send(&payload)
        .await
        .map_err(|err| TransportError::Setup(format!("secure handshake send failed: {err}")))?;

    let remote_payload = match incoming_rx.recv().await {
        Some(payload) => payload,
        None => {
            tracing::warn!(
                target = "webrtc",
                handshake_id = %params.handshake_id,
                local_peer = %params.local_peer_id,
                remote_peer = %params.remote_peer_id,
                ?role,
                "secure handshake verification failed: channel closed prematurely"
            );
            let _ = channel.close().await;
            return Err(TransportError::Setup(
                "secure handshake verification failed".into(),
            ));
        }
    };

    if remote_payload.len() != CHALLENGE_FRAME_LENGTH {
        tracing::warn!(
            target = "webrtc",
            handshake_id = %params.handshake_id,
            local_peer = %params.local_peer_id,
            remote_peer = %params.remote_peer_id,
            ?role,
            observed = remote_payload.len(),
            expected = CHALLENGE_FRAME_LENGTH,
            "secure handshake verification failed: frame length mismatch"
        );
        let _ = channel.close().await;
        return Err(TransportError::Setup(
            "secure handshake verification failed".into(),
        ));
    }

    if remote_payload[0] != CHALLENGE_FRAME_VERSION {
        tracing::warn!(
            target = "webrtc",
            handshake_id = %params.handshake_id,
            local_peer = %params.local_peer_id,
            remote_peer = %params.remote_peer_id,
            ?role,
            observed = remote_payload[0],
            expected = CHALLENGE_FRAME_VERSION,
            "secure handshake verification failed: version mismatch"
        );
        let _ = channel.close().await;
        return Err(TransportError::Setup(
            "secure handshake verification failed".into(),
        ));
    }

    let remote_role = remote_payload[1];
    if remote_role != expected_remote_role {
        tracing::warn!(
            target = "webrtc",
            handshake_id = %params.handshake_id,
            local_peer = %params.local_peer_id,
            remote_peer = %params.remote_peer_id,
            ?role,
            observed = remote_role,
            expected = expected_remote_role,
            "secure handshake verification failed: role mismatch"
        );
        let _ = channel.close().await;
        return Err(TransportError::Setup(
            "secure handshake verification failed".into(),
        ));
    }

    let remote_code = &remote_payload[2..8];
    let remote_nonce = &remote_payload[8..8 + CHALLENGE_NONCE_LENGTH];
    let remote_mac = &remote_payload[8 + CHALLENGE_NONCE_LENGTH..];

    let expected_mac = compute_challenge_mac(
        challenge_key,
        challenge_context,
        remote_role,
        remote_code,
        remote_nonce,
    )?;

    if !timing_safe_equal(remote_mac, &expected_mac) {
        tracing::warn!(
            target = "webrtc",
            handshake_id = %params.handshake_id,
            local_peer = %params.local_peer_id,
            remote_peer = %params.remote_peer_id,
            ?role,
            expected_mac = %hex::encode(expected_mac),
            observed_mac = %hex::encode(remote_mac),
            "secure handshake verification failed: mac mismatch"
        );
        let _ = channel.close().await;
        return Err(TransportError::Setup(
            "secure handshake verification failed".into(),
        ));
    }

    let remote_code_str = match std::str::from_utf8(remote_code) {
        Ok(code) => code,
        Err(_) => {
            tracing::warn!(
                target = "webrtc",
                handshake_id = %params.handshake_id,
                local_peer = %params.local_peer_id,
                remote_peer = %params.remote_peer_id,
                ?role,
                "secure handshake verification failed: remote code not valid utf8"
            );
            let _ = channel.close().await;
            return Err(TransportError::Setup(
                "secure handshake verification failed".into(),
            ));
        }
    };

    if remote_code_str != verification_code {
        tracing::warn!(
            target = "webrtc",
            handshake_id = %params.handshake_id,
            local_peer = %params.local_peer_id,
            remote_peer = %params.remote_peer_id,
            ?role,
            local_code = %verification_code,
            remote_code = %remote_code_str,
            "secure handshake verification failed: code mismatch"
        );
        let _ = channel.close().await;
        return Err(TransportError::Setup(
            "secure handshake verification failed".into(),
        ));
    }

    Ok(())
}

fn compute_challenge_mac(
    challenge_key: &[u8],
    challenge_context: &[u8],
    role_byte: u8,
    code_bytes: &[u8],
    nonce: &[u8],
) -> Result<[u8; CHALLENGE_MAC_LENGTH], TransportError> {
    let mut mac = Hmac::<Sha256>::new_from_slice(challenge_key)
        .map_err(|err| TransportError::Setup(format!("challenge mac init failed: {err}")))?;
    mac.update(challenge_context);
    mac.update(&[role_byte]);
    mac.update(code_bytes);
    mac.update(nonce);
    let tag = mac.finalize().into_bytes();
    let mut output = [0u8; CHALLENGE_MAC_LENGTH];
    output.copy_from_slice(&tag);
    Ok(output)
}

fn timing_safe_equal(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
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
