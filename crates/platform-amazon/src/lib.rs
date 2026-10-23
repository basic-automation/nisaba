pub mod auth;
pub mod client;
pub mod mapping;
pub mod types;

use std::sync::Arc;
use tokio::sync::RwLock;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use nisaba_core::config::AmazonConfig;
use nisaba_core::db::Db;
use nisaba_core::error::SyncError;
use nisaba_core::traits::PlatformAdapter;
use nisaba_core::types::{
    CreateListingRequest, FullListing, ListingPrice, Platform, PlatformCapabilities,
    PlatformInventoryItem, PlatformListing,
};
use rust_decimal::Decimal;
use serde_json::json;
use tracing::{debug, info, warn};

use crate::auth::AmazonAuth;
use crate::client::{check_submission, AmazonClient};

/// Refresh the access token when it expires within this many seconds.
const TOKEN_REFRESH_MARGIN_SECS: i64 = 300; // 5 minutes

pub struct AmazonAdapter {
    auth: AmazonAuth,
    client: AmazonClient,
    db: Arc<Db>,
    seller_id: String,
    marketplace_ids: Vec<String>,
    access_token: RwLock<Option<String>>,
    refresh_token_val: RwLock<Option<String>>,
    expires_at: RwLock<Option<DateTime<Utc>>>,
}

impl AmazonAdapter {
    pub async fn new(config: &AmazonConfig, db: Arc<Db>) -> Result<Self, SyncError> {
        let auth = AmazonAuth::new(config.client_id.clone(), config.client_secret.clone());
        let client = AmazonClient::new(&config.region);

        // Try to load existing tokens from DB
        let (access, refresh, expires_at) = if let Some(token) = db.get_auth_token("amazon").await?
        {
            let expires = token.expires_at.as_deref().and_then(|s| {
                chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                    .ok()
                    .map(|naive| naive.and_utc())
            });
            (Some(token.access_token), token.refresh_token, expires)
        } else if !config.refresh_token.is_empty() {
            // Use refresh_token from config as fallback for initial setup
            (None, Some(config.refresh_token.clone()), None)
        } else {
            (None, None, None)
        };

        Ok(Self {
            auth,
            client,
            db,
            seller_id: config.seller_id.clone(),
            marketplace_ids: config.marketplace_ids.clone(),
            access_token: RwLock::new(access),
            refresh_token_val: RwLock::new(refresh),
            expires_at: RwLock::new(expires_at),
        })
    }

    /// Get the authorization URL for initial OAuth setup.
    pub fn authorization_url(&self) -> String {
        self.auth.authorization_url()
    }

    /// Complete the OAuth flow by exchanging an authorization code.
    pub async fn complete_auth(&self, code: &str) -> Result<(), SyncError> {
        let token = self.auth.exchange_code(self.client.http(), code).await?;

        let expires_at = Utc::now() + chrono::Duration::seconds(token.expires_in);
        let expires_str = expires_at.format("%Y-%m-%d %H:%M:%S").to_string();

        self.db
            .upsert_auth_token(
                "amazon",
                &token.access_token,
                token.refresh_token.as_deref(),
                Some(&expires_str),
                // N-11.2: LWA does not expose a refresh-token expiry.
                None,
                None,
            )
            .await?;

        *self.access_token.write().await = Some(token.access_token);
        *self.refresh_token_val.write().await = token.refresh_token;
        *self.expires_at.write().await = Some(expires_at);

        info!("Amazon SP-API OAuth setup complete");
        Ok(())
    }

    /// Returns a valid access token, auto-refreshing if expired or about to expire.
    async fn get_access_token(&self) -> Result<String, SyncError> {
        let has_token = self.access_token.read().await.is_some();

        // If no access token but we have a refresh token, try to get one
        let needs_refresh = if !has_token {
            let has_refresh = self.refresh_token_val.read().await.is_some();
            if !has_refresh {
                return Err(SyncError::AuthError {
                    platform: Platform::Amazon,
                    message:
                        "No access token or refresh token available. Run Amazon OAuth setup first."
                            .into(),
                });
            }
            true
        } else {
            let expires = self.expires_at.read().await;
            match *expires {
                Some(exp) => {
                    Utc::now() + chrono::Duration::seconds(TOKEN_REFRESH_MARGIN_SECS) >= exp
                }
                None => false,
            }
        };

        if needs_refresh {
            debug!("Amazon access token expired or missing, auto-refreshing");
            match self.do_refresh().await {
                Ok(()) => {
                    info!("Amazon access token auto-refreshed successfully");
                }
                Err(e) => {
                    warn!("Amazon auto-refresh failed: {e}");
                    return Err(e);
                }
            }
        }

        self.access_token
            .read()
            .await
            .clone()
            .ok_or_else(|| SyncError::AuthError {
                platform: Platform::Amazon,
                message: "No access token available after refresh attempt.".into(),
            })
    }

    /// Internal refresh logic.
    async fn do_refresh(&self) -> Result<(), SyncError> {
        let refresh = self.refresh_token_val.read().await.clone();
        let refresh = refresh.ok_or_else(|| SyncError::AuthError {
            platform: Platform::Amazon,
            message: "No refresh token available".into(),
        })?;

        let token = self
            .auth
            .refresh_token(self.client.http(), &refresh)
            .await?;

        let expires_at = Utc::now() + chrono::Duration::seconds(token.expires_in);
        let expires_str = expires_at.format("%Y-%m-%d %H:%M:%S").to_string();

        self.db
            .upsert_auth_token(
                "amazon",
                &token.access_token,
                token.refresh_token.as_deref().or(Some(&refresh)),
                Some(&expires_str),
                // N-11.2: LWA does not expose a refresh-token expiry.
                None,
                None,
            )
            .await?;

        *self.access_token.write().await = Some(token.access_token);
        if let Some(new_refresh) = token.refresh_token {
            *self.refresh_token_val.write().await = Some(new_refresh);
        }
        *self.expires_at.write().await = Some(expires_at);

        Ok(())
    }
}

#[async_trait]
impl PlatformAdapter for AmazonAdapter {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn platform(&self) -> Platform {
        Platform::Amazon
    }

    async fn fetch_inventory(&self) -> Result<Vec<PlatformInventoryItem>, SyncError> {
        let token = self.get_access_token().await?;
        let items = self
            .client
            .fetch_listings(&token, &self.seller_id, &self.marketplace_ids)
            .await?;
        Ok(items.iter().map(mapping::to_inventory_item).collect())
    }

    async fn fetch_all_listings(&self) -> Result<Vec<PlatformListing>, SyncError> {
        let token = self.get_access_token().await?;
        let items = self
            .client
            .fetch_listings(&token, &self.seller_id, &self.marketplace_ids)
            .await?;
        Ok(items
            .iter()
            .map(|i| mapping::to_listing_for_marketplaces(i, &self.marketplace_ids))
            .collect())
    }

    async fn set_quantity(&self, platform_item_id: &str, qty: i64) -> Result<(), SyncError> {
        let token = self.get_access_token().await?;
        self.client
            .update_quantity(
                &token,
                &self.seller_id,
                platform_item_id,
                &self.marketplace_ids,
                qty,
            )
            .await
    }

    async fn is_authenticated(&self) -> bool {
        self.access_token.read().await.is_some() || self.refresh_token_val.read().await.is_some()
    }

    async fn test_connection(&self) -> Result<String, SyncError> {
        // `getMarketplaceParticipations` needs no SP-API role, so this probe
        // works before any role approvals. `get_access_token` exchanges the
        // refresh token first, so this also validates the LWA credentials.
        let token = self.get_access_token().await?;
        let participations = self.client.fetch_marketplace_participations(&token).await?;
        let names: Vec<&str> = participations
            .iter()
            .map(|p| p.marketplace.name.as_str())
            .filter(|n| !n.is_empty())
            .collect();
        let detail = if names.is_empty() {
            String::new()
        } else {
            format!(" ({})", names.join(", "))
        };
        Ok(format!(
            "Connected to Amazon SP-API — {} marketplace(s){}",
            participations.len(),
            detail
        ))
    }

    async fn refresh_auth(&self) -> Result<(), SyncError> {
        self.do_refresh().await?;
        info!("Amazon access token refreshed");
        Ok(())
    }

    async fn fetch_full_listing(&self, platform_item_id: &str) -> Result<FullListing, SyncError> {
        let token = self.get_access_token().await?;
        let item = self
            .client
            .fetch_listing(
                &token,
                &self.seller_id,
                platform_item_id,
                &self.marketplace_ids,
            )
            .await?;
        Ok(mapping::to_full_listing_for_marketplaces(
            &item,
            &self.marketplace_ids,
        ))
    }

    async fn fetch_price(&self, platform_item_id: &str) -> Result<Option<ListingPrice>, SyncError> {
        let token = self.get_access_token().await?;
        let item = self
            .client
            .fetch_listing(
                &token,
                &self.seller_id,
                platform_item_id,
                &self.marketplace_ids,
            )
            .await?;

        Ok(mapping::selected_price(&item, &self.marketplace_ids))
    }

    async fn set_price(
        &self,
        platform_item_id: &str,
        price: ListingPrice,
    ) -> Result<(), SyncError> {
        let token = self.get_access_token().await?;
        self.client
            .update_price(
                &token,
                &self.seller_id,
                platform_item_id,
                &self.marketplace_ids,
                price.amount,
                &price.currency,
            )
            .await
    }

    async fn create_listing(&self, request: CreateListingRequest) -> Result<String, SyncError> {
        let token = self.get_access_token().await?;

        let sku = request.sku.clone().unwrap_or_else(|| {
            let sanitized: String = request
                .title
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .take(50)
                .collect();
            format!("{}-{}", sanitized, chrono::Utc::now().timestamp_millis())
        });

        let product_type = request
            .extras
            .get("product_type")
            .cloned()
            .unwrap_or_else(|| "PRODUCT".to_string());

        let price_amount = request
            .price
            .as_ref()
            .map(|p| p.amount)
            .unwrap_or(Decimal::ZERO);
        let price_currency = request
            .price
            .as_ref()
            .map(|p| p.currency.clone())
            .unwrap_or_else(|| "USD".to_string());

        let marketplace_id = self
            .marketplace_ids
            .first()
            .cloned()
            .unwrap_or_else(|| "ATVPDKIKX0DER".to_string());

        let attributes = json!({
            "item_name": [{
                "value": request.title,
                "marketplace_id": marketplace_id,
            }],
            "fulfillment_availability": [{
                "fulfillment_channel_code": "DEFAULT",
                "quantity": request.quantity,
                "marketplace_id": marketplace_id,
            }],
            "purchasable_offer": [{
                "marketplace_id": marketplace_id,
                "currency": price_currency,
                "our_price": [{
                    "schedule": [{
                        "value_with_tax": format!("{:.2}", price_amount),
                    }]
                }]
            }],
        });

        let submission = self
            .client
            .create_listing(
                &token,
                &self.seller_id,
                &sku,
                &self.marketplace_ids,
                &product_type,
                attributes,
            )
            .await?;

        // The client already rejects INVALID submissions; check again here
        // so this adapter path never reports success for a rejected write
        // by discarding the submission unread (N-5.3).
        check_submission(&submission, &format!("Create listing failed for {sku}"))?;

        info!(sku = %sku, "Created Amazon listing");
        Ok(sku)
    }

    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            can_fetch_full_listing: true,
            can_fetch_description: false,
            can_fetch_photos: true,
            can_fetch_price: true,
            can_set_price: true,
            can_set_description: false,
            can_upload_photos: false,
            can_create_listing: true,
            has_stock_mode_inventory: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nisaba_core::config::AmazonConfig;
    use nisaba_core::db::Db;
    use nisaba_core::traits::PlatformAdapter;
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn test_config() -> AmazonConfig {
        AmazonConfig {
            enabled: true,
            client_id: "amzn-client-id".to_string(),
            client_secret: "amzn-client-secret".to_string(),
            refresh_token: String::new(),
            seller_id: "SELLER123".to_string(),
            region: "NA".to_string(),
            marketplace_ids: vec!["ATVPDKIKX0DER".to_string()],
            client_secret_updated_at: None,
        }
    }

    /// Fake SP-API server routing on the request path.
    async fn spawn_fake(status: u16, body: &'static str) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 8192];
                    let mut req = Vec::new();
                    loop {
                        let n = socket.read(&mut buf).await.unwrap_or(0);
                        if n == 0 {
                            break;
                        }
                        req.extend_from_slice(&buf[..n]);
                        if req.windows(4).any(|w| w == b"\r\n\r\n") {
                            break;
                        }
                    }
                    let head = String::from_utf8_lossy(&req).to_string();
                    let path_ok = head.contains("/sellers/v1/marketplaceParticipations");
                    let (code, payload) = if path_ok { (status, body) } else { (404, "{}") };
                    let reason = if code == 200 { "OK" } else { "Status" };
                    let resp = format!(
                        "HTTP/1.1 {code} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}",
                        payload.len()
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    async fn adapter_with_token(base: String) -> (AmazonAdapter, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let db = Arc::new(Db::open(db_path.to_str().unwrap()).await.expect("open db"));
        db.migrate().await.expect("migrate db");
        let mut adapter = AmazonAdapter::new(&test_config(), db)
            .await
            .expect("adapter");
        adapter.client = AmazonClient::with_api_base(base);
        // A live access token with a future expiry skips the refresh path
        // (which would call the real LWA endpoint).
        *adapter.access_token.write().await = Some("test-token".to_string());
        *adapter.expires_at.write().await = Some(Utc::now() + chrono::Duration::hours(1));
        (adapter, dir)
    }

    const PARTICIPATIONS: &str = r#"{"payload":[{"marketplace":{"id":"ATVPDKIKX0DER","name":"Amazon.com"}},{"marketplace":{"id":"A2EUQ1WTGCTBG2","name":"Amazon.ca"}}]}"#;

    /// N-11.1: test_connection reports the marketplace participations.
    #[tokio::test]
    async fn test_connection_reports_marketplaces() {
        let base = spawn_fake(200, PARTICIPATIONS).await;
        let (adapter, _dir) = adapter_with_token(base).await;
        let summary = adapter
            .test_connection()
            .await
            .expect("a 200 probe must succeed");
        assert!(summary.contains("Amazon"), "got: {summary}");
        assert!(summary.contains('2'), "got: {summary}");
        assert!(summary.contains("Amazon.com"), "got: {summary}");
    }

    /// N-11.1: a 403 surfaces as an auth error naming the token problem.
    #[tokio::test]
    async fn test_connection_surfaces_access_denied() {
        let base = spawn_fake(403, r#"{"errors":[{"code":"Unauthorized"}]}"#).await;
        let (adapter, _dir) = adapter_with_token(base).await;
        let err = adapter
            .test_connection()
            .await
            .expect_err("a 403 probe must be Err");
        assert!(err.to_string().contains("Access denied"), "got: {err}");
    }
}
