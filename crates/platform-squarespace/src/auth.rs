use nisaba_core::error::SyncError;
use nisaba_core::types::Platform;

/// Squarespace uses simple API key auth — no token refresh needed.
pub struct SquarespaceAuth {
    api_key: String,
}

impl SquarespaceAuth {
    pub fn new(api_key: String) -> Self {
        Self { api_key }
    }

    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    pub fn validate(&self) -> Result<(), SyncError> {
        if self.api_key.is_empty() {
            return Err(SyncError::AuthError {
                platform: Platform::Squarespace,
                message: "Squarespace API key is not configured".into(),
            });
        }
        Ok(())
    }
}
