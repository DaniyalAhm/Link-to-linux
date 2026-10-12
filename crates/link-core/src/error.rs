use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("codec error: {0}")]
    Codec(String),
    #[error("invalid data: {0}")]
    InvalidData(String),
    #[error("protocol version mismatch: expected {expected}, got {got}")]
    VersionMismatch { expected: u16, got: u16 },
    #[error("transport error: {0}")]
    Transport(String),
    #[error("discovery error: {0}")]
    Discovery(String),
    #[error("storage error: {0}")]
    Storage(String),
    #[error(transparent)]
    Other(#[from] anyhow::Error),
}
