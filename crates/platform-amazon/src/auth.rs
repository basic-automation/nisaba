use std::sync::Mutex;

use nisaba_core::error::SyncError;
use nisaba_core::types::Platform;
use reqwest::Client;
use tracing::{debug, info};

use crate::types::AmazonTokenResponse;

const LWA_TOKEN_URL: &str = "https://api.amazon.com/auth/o2/token";
const SELLER_CENTRAL_AUTH_URL: &str = "https://sellercentral.amazon.com/apps/authorize/consent";

pub struct AmazonAuth {
    client_id: String,
    client_secret: String,
    /// Base URL of the Nisaba OAuth broker. When set, the consent redirect, the code
    /// exchange and every refresh go through it and this struct's own app goes unused.
    broker: Option<String>,
    /// `state` issued with the most recent loopback sign-in, awaiting the answer.
    pending_state: Mutex<Option<String>>,
}

impl AmazonAuth {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client_id,
            client_secret,
            broker: None,
            pending_state: Mutex::new(None),
        }
    }

    /// Sign in through a broker instead of directly with this struct's own app.
    pub fn with_broker(mut self, broker: Option<String>) -> Self {
        self.broker = broker;
        self
    }

    /// Whether sign-in goes through the Nisaba broker (and so runs in the user's own
    /// browser) instead of directly with the user's own app.
    pub fn uses_broker(&self) -> bool {
        self.broker.is_some()
    }

    /// The broker URL to open in the user's own browser, whose callback the broker forwards
    /// to `127.0.0.1:<port>`. The `state` is `<port>.<random>`, which is how the stateless
    /// broker knows where to send the browser back to. `None` unless a broker is in use.
    pub fn authorization_url_for_loopback(&self, port: u16) -> Option<String> {
        let broker = self.broker.as_ref()?;
        let state = format!("{port}.{}", random_token());
        *self
            .pending_state
            .lock()
            .expect("Amazon OAuth state lock poisoned") = Some(state.clone());
        Some(format!(
            "{broker}/amazon/authorize?state={}",
            urlencoding(&state)
        ))
    }

    /// Whether `returned` is the state issued for the pending sign-in. Does not consume
    /// it, so a stray local request with another state is refused without ending the wait.
    pub fn state_matches_pending(&self, returned: Option<&str>) -> bool {
        let pending = self
            .pending_state
            .lock()
            .expect("Amazon OAuth state lock poisoned");
        match (pending.as_deref(), returned) {
            (Some(expected), Some(returned)) => {
                constant_time_eq(expected.as_bytes(), returned.as_bytes())
            }
            _ => false,
        }
    }

    /// POST a JSON body to one of the broker's token endpoints. The broker never returns
    /// the Amazon client secret.
    async fn broker_call(
        &self,
        http: &Client,
        broker: &str,
        path: &str,
        body: serde_json::Value,
        what: &str,
    ) -> Result<AmazonTokenResponse, SyncError> {
        let resp = http
            .post(format!("{broker}/amazon/{path}"))
            .json(&body)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Amazon,
                message: format!("Could not reach the Nisaba sign-in service: {e}"),
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::AuthError {
                platform: Platform::Amazon,
                message: format!(
                    "{what} through the Nisaba sign-in service failed ({status}): {body}"
                ),
            });
        }

        resp.json().await.map_err(|e| SyncError::ApiError {
            platform: Platform::Amazon,
            message: format!("Failed to parse token response from the sign-in service: {e}"),
        })
    }

    /// Build the URL the user must visit to authorize the app via Seller Central.
    pub fn authorization_url(&self) -> String {
        format!(
            "{}?application_id={}&version=beta",
            SELLER_CENTRAL_AUTH_URL,
            urlencoding(&self.client_id),
        )
    }

    /// Exchange an authorization code (spapi_oauth_code) for tokens.
    pub async fn exchange_code(
        &self,
        http: &Client,
        code: &str,
    ) -> Result<AmazonTokenResponse, SyncError> {
        info!("Exchanging Amazon SP-API authorization code for tokens");

        if let Some(broker) = &self.broker {
            return self
                .broker_call(
                    http,
                    broker,
                    "token",
                    serde_json::json!({ "code": code }),
                    "Code exchange",
                )
                .await;
        }

        let resp = http
            .post(LWA_TOKEN_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!(
                "grant_type=authorization_code&code={}&client_id={}&client_secret={}",
                urlencoding(code),
                urlencoding(&self.client_id),
                urlencoding(&self.client_secret),
            ))
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Amazon,
                message: e.to_string(),
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::AuthError {
                platform: Platform::Amazon,
                message: format!("Token exchange failed ({status}): {body}"),
            });
        }

        let token: AmazonTokenResponse = resp.json().await.map_err(|e| SyncError::ApiError {
            platform: Platform::Amazon,
            message: format!("Failed to parse token response: {e}"),
        })?;

        debug!(
            "Amazon token exchange successful, expires in {}s",
            token.expires_in
        );
        Ok(token)
    }

    /// Refresh an expired access token using the refresh token.
    pub async fn refresh_token(
        &self,
        http: &Client,
        refresh_token: &str,
    ) -> Result<AmazonTokenResponse, SyncError> {
        debug!("Refreshing Amazon access token");

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
            .post(LWA_TOKEN_URL)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(format!(
                "grant_type=refresh_token&refresh_token={}&client_id={}&client_secret={}",
                urlencoding(refresh_token),
                urlencoding(&self.client_id),
                urlencoding(&self.client_secret),
            ))
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Amazon,
                message: e.to_string(),
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::AuthError {
                platform: Platform::Amazon,
                message: format!("Token refresh failed ({status}): {body}"),
            });
        }

        let token: AmazonTokenResponse = resp.json().await.map_err(|e| SyncError::ApiError {
            platform: Platform::Amazon,
            message: format!("Failed to parse token response: {e}"),
        })?;

        Ok(token)
    }
}

fn urlencoding(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                String::from(b as char)
            }
            _ => format!("%{:02X}", b),
        })
        .collect()
}

/// 128 random bits, URL-safe, for the sign-in `state`.
fn random_token() -> String {
    oauth2::CsrfToken::new_random().secret().to_owned()
}

/// Compare two byte strings without short-circuiting, so the comparison time does not
/// leak how much of the state value an attacker guessed.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKEN_JSON: &str =
        r#"{"access_token":"AT","expires_in":3600,"token_type":"bearer","refresh_token":"RT"}"#;

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

    fn broker_auth(base: &str) -> AmazonAuth {
        AmazonAuth::new(String::new(), String::new()).with_broker(Some(base.to_string()))
    }

    #[test]
    fn loopback_url_points_at_the_broker_with_a_port_prefixed_state() {
        let auth = broker_auth("https://broker.example");
        assert!(auth.uses_broker());
        let url = auth
            .authorization_url_for_loopback(54321)
            .expect("broker mode");
        assert!(
            url.starts_with("https://broker.example/amazon/authorize?state=54321."),
            "{url}"
        );
        assert!(!url.contains("client"), "the app holds no client id: {url}");
    }

    #[test]
    fn without_a_broker_there_is_no_loopback_url() {
        let auth = AmazonAuth::new("id".into(), "secret".into());
        assert!(!auth.uses_broker());
        assert!(auth.authorization_url_for_loopback(54321).is_none());
    }

    #[test]
    fn only_the_issued_state_matches_and_it_is_not_consumed() {
        let auth = broker_auth("https://broker.example");
        assert!(
            !auth.state_matches_pending(Some("anything")),
            "nothing issued yet"
        );
        let url = auth.authorization_url_for_loopback(54321).expect("url");
        let issued = url.split("state=").nth(1).expect("state").to_string();
        let issued = issued.replace("%2E", ".");
        assert!(auth.state_matches_pending(Some(&issued)));
        assert!(auth.state_matches_pending(Some(&issued)), "not consumed");
        assert!(!auth.state_matches_pending(Some("54321.somebody-elses-state")));
        assert!(!auth.state_matches_pending(None));
    }

    #[test]
    fn each_attempt_issues_a_fresh_state() {
        let auth = broker_auth("https://broker.example");
        let a = auth.authorization_url_for_loopback(54321).expect("a");
        let b = auth.authorization_url_for_loopback(54321).expect("b");
        assert_ne!(a, b);
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
        let sent = request.await.expect("request");
        assert!(sent.starts_with("POST /amazon/token "), "{sent}");
        assert!(sent.contains(r#""code":"the-code""#), "{sent}");
    }

    #[tokio::test]
    async fn broker_refresh_posts_the_refresh_token() {
        let (base, request) = mock_broker("200 OK", TOKEN_JSON).await;
        let token = broker_auth(&base)
            .refresh_token(&Client::new(), "old-rt")
            .await
            .expect("refresh");
        assert_eq!(token.access_token, "AT");
        let sent = request.await.expect("request");
        assert!(sent.starts_with("POST /amazon/refresh "), "{sent}");
        assert!(sent.contains(r#""refresh_token":"old-rt""#), "{sent}");
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
            .expect_err("rejected");
        let message = err.to_string();
        assert!(
            message.contains("502") && message.contains("invalid_grant"),
            "{message}"
        );
    }
}
