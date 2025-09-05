use thiserror::Error;

#[derive(Debug, Error)]
pub enum TerminalStateError {
    #[error("Grid lookup failed: {0}")]
    LookupError(String),
    
    #[error("Delta application failed: {0}")]
    ApplyError(String),
    
    #[error("Serialization error: {0}")]
    SerializationError(String),
    
    #[error("Memory limit exceeded")]
    MemoryLimitExceeded,
    
    #[error("Invalid dimensions: {width}x{height}")]
    InvalidDimensions { width: u16, height: u16 },
    
    #[error("Parse error: {0}")]
    ParseError(String),
    
    #[error("Line {0} not found in terminal history")]
    LineNotFound(u64),
    
    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),
}