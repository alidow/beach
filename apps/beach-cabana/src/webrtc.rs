use std::time::{Duration, Instant};

use thiserror::Error;

use crate::noise::{HandshakeConfig, NoiseController, NoiseError};

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
}

pub trait CabanaChannel: Send {
    fn send(&mut self, payload: &[u8]) -> Result<(), NoiseDriverError>;
    fn recv(&mut self, timeout: Duration) -> Result<Vec<u8>, NoiseDriverError>;
}

pub struct NoiseDriver<C: CabanaChannel> {
    controller: NoiseController,
    channel: C,
}

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
