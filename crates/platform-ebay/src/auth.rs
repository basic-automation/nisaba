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
    /// Base URL of the Nisaba OAuth broker. When set, the authorize redirect, the code
    /// exchange and every refresh go through it and this struct's own keys go unused.
    broker: Option<String>,
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
            broker: None,
            pending_state: Mutex::new(None),
        }
    }

    /// Sign in through a broker instead of directly with this struct's keys.
    pub fn with_broker(mut self, broker: Option<String>) -> Self {
        self.broker = broker;
        self
    }

    /// POST a JSON body to one of the broker's token endpoints. `what` names the step
    /// for error messages. The broker never returns the eBay client secret.
    async fn broker_call(
        &self,
        http: &Client,
        broker: &str,
        path: &str,
        body: serde_json::Value,
        what: &str,
    ) -> Result<EbayTokenResponse, SyncError> {
        let resp = http
            .post(format!("{broker}/ebay/{path}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Ebay,
                message: format!("Could not reach the Nisaba eBay broker: {e}"),
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::AuthError {
                platform: Platform::Ebay,
                message: format!("{what} through the Nisaba eBay broker failed ({status}): {body}"),
            });
        }

        resp.json().await.map_err(|e| SyncError::ApiError {
            platform: Platform::Ebay,
            message: format!("Failed to parse token response from the broker: {e}"),
        })
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

    /// Whether sign-in goes through the Nisaba broker (and so can run in the user's own
    /// browser) instead of directly with this struct's own keys.
    pub fn uses_broker(&self) -> bool {
        self.broker.is_some()
    }

    /// Issue and store a fresh state for this attempt. A loopback sign-in prefixes it with
    /// the port of the listener waiting in the app (`<port>.<random>`), which is how the
    /// broker knows where to send the browser back to without storing anything.
    fn issue_state(&self, loopback_port: Option<u16>) -> String {
        let token = CsrfToken::new_random().secret().to_owned();
        let state = match loopback_port {
            Some(port) => format!("{port}.{token}"),
            None => token,
        };
        *self
            .pending_state
            .lock()
            .expect("eBay OAuth state lock poisoned") = Some(state.clone());
        state
    }

    /// The broker URL to open in the user's own browser, whose callback the broker forwards
    /// to `127.0.0.1:<port>`. `None` unless sign-in goes through a broker.
    pub fn authorization_url_for_loopback(&self, port: u16) -> Option<String> {
        let broker = self.broker.as_ref()?;
        let state = self.issue_state(Some(port));
        Some(format!(
            "{broker}/ebay/authorize?state={}",
            urlencoding(&state)
        ))
    }

    /// Generate the URL the user must visit for initial OAuth authorization.
    ///
    /// Each call issues a fresh, unguessable `state` token (128 bits of
    /// entropy), stores it as the expected value for this attempt, and
    /// includes it in the URL. The redirect callback must echo the same
    /// `state`; see [`EbayAuth::verify_callback_state`].
    pub fn authorization_url(&self) -> String {
        let state_str = self.issue_state(None);
        if let Some(broker) = &self.broker {
            return format!("{broker}/ebay/authorize?state={}", urlencoding(&state_str));
        }
        format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}",
            self.auth_url(),
            urlencoding(&self.client_id),
            urlencoding(&self.redirect_uri),
            urlencoding(SCOPES),
            urlencoding(&state_str),
        )
    }

    /// Whether `returned` is the `state` issued for the attempt in progress. Unlike
    /// [`EbayAuth::verify_callback_state`] this does not consume the stored value, so
    /// the auth window can ask it of every page that carries a `code`: eBay's sign-in
    /// can bounce through Google, Apple or Facebook, whose own callbacks carry a `code`
    /// and a `state` of their own and must be ignored, not rejected.
    pub fn state_matches_pending(&self, returned: Option<&str>) -> bool {
        let pending = self
            .pending_state
            .lock()
            .expect("eBay OAuth state lock poisoned");
        match (pending.as_deref(), returned) {
            (Some(expected), Some(returned)) => {
                constant_time_eq(expected.as_bytes(), returned.as_bytes())
            }
            _ => false,
        }
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

        if let Some(broker) = &self.broker {
            return self
                .broker_call(
                    http,
                    broker,
                    "token",
                    serde_json::json!({ "code": code }),
                    "Token exchange",
                )
                .await;
        }

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

        if let Some(broker) = &self.broker {
            return self
                .broker_call(
                    http,
                    broker,
                    "refresh",
                    serde_json::json!({ "refresh_token": refresh_token }),
                    "Token refresh",
                )
                .await;
        }

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
    fn a_loopback_sign_in_encodes_its_port_in_the_state_and_still_verifies_once() {
        let auth = broker_auth("https://broker.example");
        assert!(auth.uses_broker());
        let url = auth
            .authorization_url_for_loopback(54321)
            .expect("broker mode");
        assert!(
            url.starts_with("https://broker.example/ebay/authorize?state=54321."),
            "got {url}"
        );
        let issued = state_param(&url).expect("state param");
        assert!(auth.verify_callback_state(Some(&issued)).is_ok());
        assert!(
            auth.verify_callback_state(Some(&issued)).is_err(),
            "single use"
        );
    }

    #[test]
    fn direct_mode_has_no_loopback_sign_in() {
        let auth = test_auth();
        assert!(!auth.uses_broker());
        assert!(auth.authorization_url_for_loopback(54321).is_none());
    }

    #[test]
    fn a_foreign_state_is_not_ours_and_asking_does_not_consume_ours() {
        let auth = test_auth();
        let issued = state_param(&auth.authorization_url()).expect("state param");
        // A third-party sign-in (Google) callback carries its own state: not ours.
        assert!(!auth.state_matches_pending(Some("a-state-issued-by-google-not-by-us")));
        assert!(!auth.state_matches_pending(None));
        // Looking is not consuming: ours still matches, and still verifies once.
        assert!(auth.state_matches_pending(Some(&issued)));
        assert!(auth.state_matches_pending(Some(&issued)));
        assert!(auth.verify_callback_state(Some(&issued)).is_ok());
        assert!(
            !auth.state_matches_pending(Some(&issued)),
            "consumed by verify"
        );
    }

    #[test]
    fn state_matching_needs_an_attempt_in_progress() {
        assert!(!test_auth().state_matches_pending(Some("anything")));
    }

    #[test]
    fn state_is_single_use() {
        let auth = test_auth();
        let issued = state_param(&auth.authorization_url()).expect("state param");
        assert!(auth.verify_callback_state(Some(&issued)).is_ok());
        // Replaying the same callback URL must fail.
        assert!(auth.verify_callback_state(Some(&issued)).is_err());
    }

    /// Serve one canned HTTP response on a local port; returns the base URL and a handle
    /// that resolves to the raw request text the server received.
    async fn mock_broker(
        status_line: &str,
        body: &str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let base = format!("http://{}", listener.local_addr().expect("addr"));
        let response = format!(
            "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let handle = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.expect("accept");
            let mut received = Vec::new();
            let mut chunk = [0u8; 4096];
            loop {
                let n = socket.read(&mut chunk).await.expect("read");
                received.extend_from_slice(&chunk[..n]);
                let text = String::from_utf8_lossy(&received).to_string();
                if let Some(split) = text.find("\r\n\r\n") {
                    let length = text[..split]
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("content-length:")
                                .map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);
                    if received.len() >= split + 4 + length {
                        break;
                    }
                }
                if n == 0 {
                    break;
                }
            }
            socket.write_all(response.as_bytes()).await.expect("write");
            String::from_utf8_lossy(&received).to_string()
        });
        (base, handle)
    }

    fn broker_auth(base: &str) -> EbayAuth {
        EbayAuth::new(String::new(), String::new(), String::new(), "production")
            .with_broker(Some(base.to_string()))
    }

    const TOKEN_JSON: &str = r#"{"access_token":"AT","expires_in":7200,"token_type":"User Access Token","refresh_token":"RT","refresh_token_expires_in":47304000}"#;

    #[test]
    fn broker_authorize_url_points_at_the_broker_with_a_checkable_state() {
        let auth = broker_auth("https://broker.example");
        let url = auth.authorization_url();
        assert!(
            url.starts_with("https://broker.example/ebay/authorize?state="),
            "got {url}"
        );
        assert!(
            !url.contains("client_id"),
            "the app holds no client id in broker mode"
        );
        let issued = state_param(&url).expect("state param");
        assert!(auth.verify_callback_state(Some(&issued)).is_ok());
        assert!(
            auth.verify_callback_state(Some(&issued)).is_err(),
            "single use"
        );
    }

    #[tokio::test]
    async fn broker_code_exchange_posts_the_code_and_parses_the_tokens() {
        let (base, request) = mock_broker("200 OK", TOKEN_JSON).await;
        let token = broker_auth(&base)
            .exchange_code(&Client::new(), "the-code")
            .await
            .expect("exchange");
        assert_eq!(token.access_token, "AT");
        assert_eq!(token.refresh_token.as_deref(), Some("RT"));
        assert_eq!(token.refresh_token_expires_in, Some(47304000));
        let raw = request.await.expect("request");
        assert!(raw.starts_with("POST /ebay/token "), "got {raw}");
        assert!(raw.contains(r#""code":"the-code""#));
    }

    #[tokio::test]
    async fn broker_refresh_posts_the_refresh_token() {
        let (base, request) = mock_broker("200 OK", TOKEN_JSON).await;
        let token = broker_auth(&base)
            .refresh_token(&Client::new(), "my-refresh-token")
            .await
            .expect("refresh");
        assert_eq!(token.access_token, "AT");
        let raw = request.await.expect("request");
        assert!(raw.starts_with("POST /ebay/refresh "), "got {raw}");
        assert!(raw.contains(r#""refresh_token":"my-refresh-token""#));
    }

    #[tokio::test]
    async fn a_broker_rejection_surfaces_as_an_auth_error_with_the_reason() {
        let (base, _request) = mock_broker(
            "502 Bad Gateway",
            r#"{"error":"upstream_rejected","upstream_error":"invalid_grant"}"#,
        )
        .await;
        let err = broker_auth(&base)
            .exchange_code(&Client::new(), "expired")
            .await
            .expect_err("must fail");
        let text = err.to_string();
        assert!(
            text.contains("502") && text.contains("invalid_grant"),
            "got {text}"
        );
    }
}
