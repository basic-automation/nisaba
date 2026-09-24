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
}

impl AmazonAuth {
    pub fn new(client_id: String, client_secret: String) -> Self {
        Self {
            client_id,
            client_secret,
        }
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
