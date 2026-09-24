use crate::types::Platform;
use thiserror::Error;

#[derive(Error, Debug)]
pub enum SyncError {
    #[error("Platform {platform} authentication failed: {message}")]
    AuthError { platform: Platform, message: String },

    #[error("Platform {platform} API error: {message}")]
    ApiError { platform: Platform, message: String },

    #[error("Platform {platform} rate limited, retry after {retry_after_secs}s")]
    RateLimited {
        platform: Platform,
        retry_after_secs: u64,
    },

    #[error("Platform {platform} network error: {message}")]
    NetworkError { platform: Platform, message: String },

    #[error("Database error: {0}")]
    DatabaseError(String),

    #[error("Config error: {0}")]
    ConfigError(String),

    #[error("Conflict resolution failed for product {product_id}: {message}")]
    ConflictError { product_id: String, message: String },

    #[error("Platform {platform} is not enabled")]
    PlatformDisabled { platform: Platform },

    #[error("eBay revision limit reached for SKU {sku} ({count}/250 today)")]
    EbayRevisionLimit { sku: String, count: u32 },

    #[error("Parse error: {0}")]
    ParseError(String),

    #[error("{0}")]
    Other(String),
}

impl From<turso::Error> for SyncError {
    fn from(err: turso::Error) -> Self {
        SyncError::DatabaseError(err.to_string())
    }
}
