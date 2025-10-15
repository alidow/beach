use std::time::{Duration, Instant};

use thiserror::Error;

use crate::noise::{HandshakeConfig, NoiseController, NoiseError};

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum NoiseDriverError {
    #[error("noise handshake error: {0}")]
    Noise(#[from] NoiseError),
    #[error("channel send failed: {0}")]
    ChannelSend(String),
    #[error("channel closed")]
    ChannelClosed,
    #[error("handshake timeout exceeded")]
    Timeout,
    #[error("handshake not complete")]
    HandshakeIncomplete,
    #[error("unexpected plaintext during handshake")]
    UnexpectedPlaintext,
    #[error("received non-media frame post-handshake")]
    UnexpectedFrame,
    #[error("noise worker join failed: {0}")]
    Join(String),
}

#[allow(dead_code)]
pub trait CabanaChannel: Send {
    fn send(&mut self, payload: &[u8]) -> Result<(), NoiseDriverError>;
    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, NoiseDriverError>;
}

#[allow(dead_code)]
pub struct NoiseDriver<C: CabanaChannel> {
    controller: NoiseController,
    channel: C,
}

#[allow(dead_code)]
impl<C: CabanaChannel> NoiseDriver<C> {
    pub fn new(channel: C, config: HandshakeConfig) -> Result<Self, NoiseDriverError> {
        let controller = NoiseController::new(config)?;
        Ok(Self { controller, channel })
    }

    fn flush_outgoing(&mut self) -> Result<(), NoiseDriverError> {
        while let Some(message) = self.controller.take_outgoing() {
            self.channel.send(&message)?;
        }
        Ok(())
    }

    pub fn run_handshake(&mut self, timeout: Duration) -> Result<(), NoiseDriverError> {
        let start = Instant::now();
        self.flush_outgoing()?;
        while !self.controller.handshake_complete() {
            let elapsed = start.elapsed();
            if elapsed >= timeout {
                return Err(NoiseDriverError::Timeout);
            }
            let remaining = timeout.saturating_sub(elapsed);
            let payload = self.channel.recv(remaining)?;
            if self.controller.process_incoming(&payload)?.is_some() {
                return Err(NoiseDriverError::UnexpectedPlaintext);
            }
            self.flush_outgoing()?;
        }
        Ok(())
    }

    pub fn verification_code(&self) -> Option<&str> {
        self.controller.verification_code()
    }

    pub fn send_media(&mut self, plaintext: &[u8]) -> Result<(), NoiseDriverError> {
        if !self.controller.handshake_complete() {
            return Err(NoiseDriverError::HandshakeIncomplete);
        }
        let frame = self.controller.seal_media(plaintext)?;
        self.channel.send(&frame)?;
        Ok(())
    }

    pub fn recv_media(&mut self, timeout: Duration) -> Result<Vec<u8>, NoiseDriverError> {
        if !self.controller.handshake_complete() {
            return Err(NoiseDriverError::HandshakeIncomplete);
        }
        let payload = self.channel.recv(timeout)?;
        match self.controller.process_incoming(&payload)? {
            Some(plaintext) => Ok(plaintext),
            None => Err(NoiseDriverError::UnexpectedFrame),
        }
    }
}

#[cfg(feature = "webrtc")]
use std::sync::{Arc, Mutex};

#[cfg(feature = "webrtc")]
use bytes::Bytes;

#[cfg(feature = "webrtc")]
use tokio::{
    runtime::Handle,
    sync::{mpsc, oneshot},
    task,
    time,
};

#[cfg(feature = "webrtc")]
use webrtc::data_channel::{
    data_channel_message::DataChannelMessage,
    data_channel_state::RTCDataChannelState,
    RTCDataChannel,
};

#[cfg(feature = "webrtc")]
#[allow(dead_code)]
struct DataChannelAdapter {
    channel: Arc<RTCDataChannel>,
    handle: Handle,
    receiver: mpsc::UnboundedReceiver<Option<Vec<u8>>>,
}

#[cfg(feature = "webrtc")]
#[allow(dead_code)]
impl DataChannelAdapter {
    fn new(channel: Arc<RTCDataChannel>, handle: Handle) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let message_tx = tx.clone();
        channel.on_message(Box::new(move |msg: DataChannelMessage| {
            let sender = message_tx.clone();
            Box::pin(async move {
                if sender.send(Some(msg.data.to_vec())).is_err() {
                    tracing::debug!("cabana noise message dropped: receiver gone");
                }
            })
        }));

        let close_tx = tx.clone();
        channel.on_close(Box::new(move || {
            let _ = close_tx.send(None);
            Box::pin(async {})
        }));

        Self {
            channel,
            handle,
            receiver: rx,
        }
    }
}

#[cfg(feature = "webrtc")]
impl CabanaChannel for DataChannelAdapter {
    fn send(&mut self, payload: &[u8]) -> Result<(), NoiseDriverError> {
        let channel = self.channel.clone();
        let data = Bytes::from(payload.to_vec());
        self.handle.block_on(async move {
            channel
                .send(&data)
                .await
                .map(|_| ())
                .map_err(|err| NoiseDriverError::ChannelSend(err.to_string()))
        })
    }

    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, NoiseDriverError> {
        self.handle.block_on(async {
            match time::timeout(timeout, self.receiver.recv()).await {
                Ok(Some(Some(payload))) => Ok(payload),
                Ok(Some(None)) => Err(NoiseDriverError::ChannelClosed),
                Ok(None) => Err(NoiseDriverError::ChannelClosed),
                Err(_) => Err(NoiseDriverError::Timeout),
            }
        })
    }
}

#[cfg(feature = "webrtc")]
#[allow(dead_code)]
pub struct DataChannelSecureTransport {
    driver: Arc<Mutex<NoiseDriver<DataChannelAdapter>>>,
    verification_code: Option<String>,
}

#[cfg(feature = "webrtc")]
#[allow(dead_code)]
impl DataChannelSecureTransport {
    pub fn verification_code(&self) -> Option<&str> {
        self.verification_code.as_deref()
    }

    pub async fn send_media(&self, payload: &[u8]) -> Result<(), NoiseDriverError> {
        let driver = self.driver.clone();
        let data = payload.to_vec();
        task::spawn_blocking(move || {
            let mut guard = driver.lock().unwrap();
            guard.send_media(&data)
        })
        .await
        .map_err(|err| NoiseDriverError::Join(err.to_string()))?
    }

    pub async fn recv_media(&self, timeout: Duration) -> Result<Vec<u8>, NoiseDriverError> {
        let driver = self.driver.clone();
        task::spawn_blocking(move || {
            let mut guard = driver.lock().unwrap();
            guard.recv_media(timeout)
        })
        .await
        .map_err(|err| NoiseDriverError::Join(err.to_string()))?
    }
}

#[cfg(feature = "webrtc")]
#[allow(dead_code)]
async fn wait_for_channel_open(
    channel: &Arc<RTCDataChannel>,
    timeout: Duration,
) -> Result<Duration, NoiseDriverError> {
    if channel.ready_state() == RTCDataChannelState::Open {
        return Ok(timeout);
    }

    let (tx, rx) = oneshot::channel();
    channel.on_open(Box::new(move || {
        let _ = tx.send(());
        Box::pin(async {})
    }));

    let start = Instant::now();
    time::timeout(timeout, rx)
        .await
        .map_err(|_| NoiseDriverError::Timeout)?
        .map_err(|_| NoiseDriverError::ChannelClosed)?;
    let elapsed = start.elapsed();
    Ok(timeout.saturating_sub(elapsed))
}

#[cfg(feature = "webrtc")]
#[allow(dead_code)]
pub async fn negotiate_data_channel(
    channel: Arc<RTCDataChannel>,
    config: HandshakeConfig<'_>,
    timeout: Duration,
) -> Result<DataChannelSecureTransport, NoiseDriverError> {
    let remaining = wait_for_channel_open(&channel, timeout).await?;
    let adapter = DataChannelAdapter::new(channel, Handle::current());
    let mut driver = NoiseDriver::new(adapter, config)?;

    let driver = task::spawn_blocking(move || -> Result<NoiseDriver<DataChannelAdapter>, NoiseDriverError> {
        driver.run_handshake(remaining)?;
        Ok(driver)
    })
    .await
    .map_err(|err| NoiseDriverError::Join(err.to_string()))??;

    let verification_code = driver.verification_code().map(|code| code.to_string());
    let driver = Arc::new(Mutex::new(driver));
    Ok(DataChannelSecureTransport {
        driver,
        verification_code,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::security::{HandshakeId, SessionMaterial};
    use crossbeam_channel::{Receiver, RecvTimeoutError, Sender};
    use std::thread;

    struct MemoryChannel {
        inbox: Receiver<Vec<u8>>,
        outbox: Sender<Vec<u8>>,
    }

    impl CabanaChannel for MemoryChannel {
        fn send(&mut self, payload: &[u8]) -> Result<(), NoiseDriverError> {
            self.outbox
                .send(payload.to_vec())
                .map_err(|err| NoiseDriverError::ChannelSend(err.to_string()))
        }

        fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, NoiseDriverError> {
            match self.inbox.recv_timeout(timeout) {
                Ok(payload) => Ok(payload),
                Err(RecvTimeoutError::Timeout) => Err(NoiseDriverError::Timeout),
                Err(RecvTimeoutError::Disconnected) => Err(NoiseDriverError::ChannelClosed),
            }
        }
    }

    fn channel_pair() -> (MemoryChannel, MemoryChannel) {
        let (host_tx, host_rx) = crossbeam_channel::unbounded();
        let (viewer_tx, viewer_rx) = crossbeam_channel::unbounded();
        (
            MemoryChannel {
                inbox: host_rx,
                outbox: viewer_tx,
            },
            MemoryChannel {
                inbox: viewer_rx,
                outbox: host_tx,
            },
        )
    }

    fn material() -> SessionMaterial {
        SessionMaterial::derive("cabana-webrtc-session", "super-secret-passcode").unwrap()
    }

    fn handshake_id() -> HandshakeId {
        HandshakeId::from_base64("AAAAAAAAAAAAAAAAAAAAAA==").unwrap()
    }

    #[test]
    fn driver_completes_handshake_and_transports_media() {
        let material = material();
        let handshake_id = handshake_id();
        let context = b"cabana-driver-demo".to_vec();

        let (host_channel, viewer_channel) = channel_pair();
        let host_config = HandshakeConfig {
            material: &material,
            handshake_id: &handshake_id,
            role: crate::noise::HandshakeRole::Initiator,
            local_id: "host",
            remote_id: "viewer",
            prologue_context: &context,
        };
        let viewer_config = HandshakeConfig {
            material: &material,
            handshake_id: &handshake_id,
            role: crate::noise::HandshakeRole::Responder,
            local_id: "viewer",
            remote_id: "host",
            prologue_context: &context,
        };

        let mut host_driver = NoiseDriver::new(host_channel, host_config).unwrap();
        let mut viewer_driver = NoiseDriver::new(viewer_channel, viewer_config).unwrap();

        let host_handle = thread::spawn(move || -> Result<(), NoiseDriverError> {
            host_driver.run_handshake(Duration::from_secs(2))?;
            assert!(host_driver.verification_code().is_some());
            host_driver.send_media(b"cabana-host-frame")?;
            Ok(())
        });

        let viewer_handle = thread::spawn(move || -> Result<Vec<u8>, NoiseDriverError> {
            viewer_driver.run_handshake(Duration::from_secs(2))?;
            assert!(viewer_driver.verification_code().is_some());
            viewer_driver.recv_media(Duration::from_secs(2))
        });

        host_handle.join().unwrap().unwrap();
        let plaintext = viewer_handle.join().unwrap().unwrap();
        assert_eq!(plaintext, b"cabana-host-frame");
    }
}
