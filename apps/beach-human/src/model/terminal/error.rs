use std::error::Error;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TerminalStateError {
    SerializationError(String),
}

impl fmt::Display for TerminalStateError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TerminalStateError::SerializationError(msg) => write!(f, "Serialization error: {msg}"),
        }
    }
}

impl Error for TerminalStateError {}
