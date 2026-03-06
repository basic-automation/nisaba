use thiserror::Error;

#[derive(Error, Debug)]
pub enum P2PError {
    #[error("Not in a company")]
    NoCompany,

    #[error("Peer not authorized: {0}")]
    PeerNotAuthorized(String),

    #[error("Authentication failed: invalid company secret")]
    AuthFailed,

    #[error("Onion service not ready")]
    OnionNotReady,

    #[error("Network error: {0}")]
    NetworkError(String),

    #[error("Serialization error: {0}")]
    SerializationError(String),

    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Protocol version mismatch: expected {expected}, got {got}")]
    VersionMismatch { expected: u32, got: u32 },

    #[error("Peer offline: {0}")]
    PeerOffline(String),

    #[error("{0}")]
    Other(String),
}

impl From<nisaba_core::error::SyncError> for P2PError {
    fn from(err: nisaba_core::error::SyncError) -> Self {
        P2PError::DatabaseError(err.to_string())
    }
}

impl From<serde_json::Error> for P2PError {
    fn from(err: serde_json::Error) -> Self {
        P2PError::SerializationError(err.to_string())
    }
}

impl From<anyhow::Error> for P2PError {
    fn from(err: anyhow::Error) -> Self {
        P2PError::NetworkError(err.to_string())
    }
}

impl From<turso::Error> for P2PError {
    fn from(err: turso::Error) -> Self {
        P2PError::DatabaseError(err.to_string())
    }
}
