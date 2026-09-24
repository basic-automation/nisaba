use nisaba_core::error::SyncError;
use nisaba_core::types::Platform;
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
    pub fn authorization_url(&self) -> String {
        format!(
            "{}?client_id={}&redirect_uri={}&response_type=code&scope={}",
            self.auth_url(),
            urlencoding(&self.client_id),
            urlencoding(&self.redirect_uri),
            urlencoding(SCOPES),
        )
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
