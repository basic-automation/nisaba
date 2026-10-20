use std::sync::Mutex;

use nisaba_core::error::SyncError;
use nisaba_core::types::Platform;
use oauth2::CsrfToken;
use reqwest::Client;
use tracing::{debug, info};

use crate::types::EbayTokenResponse;

const SANDBOX_AUTH_URL: &str = "https://auth.sandbox.ebay.com/oauth2/authorize";
const SANDBOX_TOKEN_URL: &str = "https://api.sandbox.ebay.com/identity/v1/oauth2/token";
const PRODUCTION_AUTH_URL: &str = "https://auth.ebay.com/oauth2/authorize";
const PRODUCTION_TOKEN_URL: &str = "https://api.ebay.com/identity/v1/oauth2/token";

const SCOPES: &str = "https://api.ebay.com/oauth/api_scope https://api.ebay.com/oauth/api_scope/sell.inventory https://api.ebay.com/oauth/api_scope/sell.account.readonly";

pub struct EbayAuth {
    client_id: String,
    client_secret: String,
    redirect_uri: String,
    is_sandbox: bool,
    /// `state` issued with the most recent authorize URL, awaiting the
    /// redirect callback. Consumed single-use by [`EbayAuth::verify_callback_state`].
    pending_state: Mutex<Option<String>>,
}

impl EbayAuth {
    pub fn new(
        client_id: String,
        client_secret: String,
        redirect_uri: String,
        environment: &str,
    ) -> Self {
        Self {
            client_id,
            client_secret,
            redirect_uri,
            is_sandbox: environment != "production",
            pending_state: Mutex::new(None),
        }
    }

    fn auth_url(&self) -> &str {
        if self.is_sandbox {
            SANDBOX_AUTH_URL
        } else {
            PRODUCTION_AUTH_URL
        }
    }

    fn token_url(&self) -> &str {
        if self.is_sandbox {
            SANDBOX_TOKEN_URL
        } else {
            PRODUCTION_TOKEN_URL
        }
    }

    /// Generate the URL the user must visit for initial OAuth authorization.
    ///
    /// Each call issues a fresh, unguessable `state` token (128 bits of
    /// entropy), stores it as the expected value for this attempt, and
    /// includes it in the URL. The redirect callback must echo the same
    /// `state`; see [`EbayAuth::verify_callback_state`].
    pub fn authorization_url(&self) -> String {
        let state = CsrfToken::new_random();
        let state_str = state.secret().to_owned();
        *self
            .pending_state
            .lock()
            .expect("eBay OAuth state lock poisoned") = Some(state_str.clone());
        format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}",
            self.auth_url(),
            urlencoding(&self.client_id),
            urlencoding(&self.redirect_uri),
            urlencoding(SCOPES),
            urlencoding(&state_str),
        )
    }

    /// Check the `state` value eBay returned on the redirect callback against
    /// the value stored when the authorize URL was issued for this attempt.
    ///
    /// The stored value is consumed single-use — it is taken on every call,
    /// match or not — so a captured callback URL cannot be replayed against a
    /// later attempt. Returns `Ok` only when a state was issued and the
    /// returned value equals it (constant-time comparison).
    pub fn verify_callback_state(&self, returned: Option<&str>) -> Result<(), SyncError> {
        let expected = self
            .pending_state
            .lock()
            .expect("eBay OAuth state lock poisoned")
            .take();
        let matches = match (expected.as_deref(), returned) {
            (Some(expected), Some(returned)) => {
                constant_time_eq(expected.as_bytes(), returned.as_bytes())
            }
            _ => false,
        };
        if matches {
            Ok(())
        } else {
            Err(SyncError::AuthError {
                platform: Platform::Ebay,
                message: "eBay OAuth state mismatch: the authorization response did not match the issued request"
                    .to_string(),
            })
        }
    }

    /// Exchange an authorization code for an access + refresh token.
    pub async fn exchange_code(
        &self,
        http: &Client,
        code: &str,
    ) -> Result<EbayTokenResponse, SyncError> {
        info!("Exchanging eBay authorization code for tokens");

        let resp = http
            .post(self.token_url())
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!(
                "grant_type=authorization_code&code={}&redirect_uri={}",
                urlencoding(code),
                urlencoding(&self.redirect_uri),
            ))
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Ebay,
                message: e.to_string(),
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::AuthError {
                platform: Platform::Ebay,
                message: format!("Token exchange failed ({status}): {body}"),
            });
        }

        let token: EbayTokenResponse = resp.json().await.map_err(|e| SyncError::ApiError {
            platform: Platform::Ebay,
            message: format!("Failed to parse token response: {e}"),
        })?;

        debug!(
            "eBay token exchange successful, expires in {}s",
            token.expires_in
        );
        Ok(token)
    }

    /// Refresh an expired access token using the refresh token.
    pub async fn refresh_token(
        &self,
        http: &Client,
        refresh_token: &str,
    ) -> Result<EbayTokenResponse, SyncError> {
        debug!("Refreshing eBay access token");

        let resp = http
            .post(self.token_url())
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!(
                "grant_type=refresh_token&refresh_token={}&scope={}",
                urlencoding(refresh_token),
                urlencoding(SCOPES),
            ))
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Ebay,
                message: e.to_string(),
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::AuthError {
                platform: Platform::Ebay,
                message: format!("Token refresh failed ({status}): {body}"),
            });
        }

        let token: EbayTokenResponse = resp.json().await.map_err(|e| SyncError::ApiError {
            platform: Platform::Ebay,
            message: format!("Failed to parse token response: {e}"),
        })?;

        Ok(token)
    }
}

fn urlencoding(s: &str) -> String {
    // Simple percent-encoding for URL parameters
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                String::from(b as char)
            }
            _ => format!("%{:02X}", b),
        })
        .collect()
}

/// Compare two byte strings without short-circuiting, so the comparison
/// time does not leak how much of the state value an attacker guessed.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_auth() -> EbayAuth {
        EbayAuth::new(
            "client-id".to_string(),
            "client-secret".to_string(),
            "https://example.com/callback".to_string(),
            "sandbox",
        )
    }

    /// Extract the `state` query parameter from an authorize URL.
    fn state_param(url: &str) -> Option<String> {
        url.split('?')
            .nth(1)?
            .split('&')
            .filter_map(|pair| pair.split_once('='))
            .find(|(key, _)| *key == "state")
            .map(|(_, v)| v.to_string())
    }

    #[test]
    fn authorize_url_carries_state_stored_for_the_attempt() {
        let auth = test_auth();
        let url = auth.authorization_url();
        let issued = state_param(&url).expect("authorize URL must carry a state parameter");
        assert!(!issued.is_empty(), "state must not be empty");
        // The callback carrying the issued state verifies against the stored attempt.
        assert!(auth.verify_callback_state(Some(&issued)).is_ok());
    }

    #[test]
    fn state_is_unguessable_across_attempts() {
        let auth = test_auth();
        let first = state_param(&auth.authorization_url()).expect("state param");
        let second = state_param(&auth.authorization_url()).expect("state param");
        assert_ne!(first, second, "each attempt must issue a fresh state");
        // CsrfToken::new_random carries 128 bits of entropy, base64-encoded.
        assert!(first.len() >= 22, "state too short for 128-bit entropy");
    }

    #[test]
    fn callback_rejects_mismatched_state() {
        let auth = test_auth();
        auth.authorization_url();
        let err = auth
            .verify_callback_state(Some("attacker-forged-state"))
            .expect_err("a forged state must be rejected");
        assert!(
            err.to_string().contains("state"),
            "error should mention the state check, got: {err}"
        );
    }

    #[test]
    fn callback_rejects_missing_state() {
        let auth = test_auth();
        auth.authorization_url();
        assert!(auth.verify_callback_state(None).is_err());
        // And a callback with no issued attempt at all is rejected too.
        let fresh = test_auth();
        assert!(fresh.verify_callback_state(Some("anything")).is_err());
    }

    #[test]
    fn state_is_single_use() {
        let auth = test_auth();
        let issued = state_param(&auth.authorization_url()).expect("state param");
        assert!(auth.verify_callback_state(Some(&issued)).is_ok());
        // Replaying the same callback URL must fail.
        assert!(auth.verify_callback_state(Some(&issued)).is_err());
    }
}
