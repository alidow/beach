use crate::session::SessionError;
use std::io;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum CliError {
    #[error("{0}")]
    Session(#[from] SessionError),
    #[error("io error: {0}")]
    Io(#[from] io::Error),
    #[error("unable to determine session id from '{target}'")]
    InvalidSessionTarget { target: String },
    #[error("no executable command available; set $SHELL or pass '-- command'")]
    MissingCommand,
    #[error("session requires a six character alphanumeric passcode")]
    MissingPasscode,
    #[error("transport negotiation failed: {0}")]
    TransportNegotiation(String),
    #[error("session did not provide a supported transport offer")]
    NoUsableTransport,
    #[error("terminal runtime error: {0}")]
    Runtime(String),
    #[error("auth error: {0}")]
    Auth(String),
    #[error("logging initialization failed: {0}")]
    Logging(String),
    #[error("bootstrap output failed: {0}")]
    BootstrapOutput(String),
    #[error("bootstrap handshake failed: {0}")]
    BootstrapHandshake(String),
    #[error("binary copy failed: {0}")]
    CopyBinary(String),
    #[error("remote architecture detection failed: {0}")]
    RemoteArchDetection(String),
    #[error("cross-compilation failed: {0}")]
    CrossCompile(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}
