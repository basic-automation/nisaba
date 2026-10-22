pub mod auth;
pub mod client;
pub mod endpoints;
pub mod scraper;
pub mod types;

use std::collections::HashMap;
use std::sync::Mutex;

use async_trait::async_trait;
use chrono::Utc;
use nisaba_core::config::XmrBazaarConfig;
use nisaba_core::error::SyncError;
use nisaba_core::traits::PlatformAdapter;
use nisaba_core::types::{
    CreateListingRequest, FullListing, ListingDescription, ListingPhoto, ListingPrice, Platform,
    PlatformCapabilities, PlatformInventoryItem, PlatformListing, SaleDetection,
    UpdateListingRequest,
};
use tracing::debug;

use crate::auth::XmrBazaarAuth;
use crate::client::XmrBazaarClient;
use crate::endpoints::Endpoints;
use crate::types::StockModeField;

/// The form fields a price write submits: the amount *and* its currency
/// (upper-cased, matching `create_listing`), in one submission.
fn price_update_fields(price: &ListingPrice) -> Vec<(String, String)> {
    vec![
        ("price".to_string(), price.amount.to_string()),
        ("currency".to_string(), price.currency.to_uppercase()),
    ]
}

/// The value a description write submits: plain text with HTML tags stripped,
/// matching `create_listing`'s handling of `description_html`.
fn description_update_value(html: &str) -> String {
    crate::scraper::strip_html_tags(html)
}

pub struct XmrBazaarAdapter {
    auth: XmrBazaarAuth,
    client: XmrBazaarClient,
    endpoints: Endpoints,
    monero_address: String,
    /// Cached stock mode field discovery result.
    stock_mode_field: Mutex<Option<StockModeField>>,
}

impl XmrBazaarAdapter {
    pub fn new(config: &XmrBazaarConfig) -> Self {
        let auth = XmrBazaarAuth::new(config.username.clone(), config.password.clone());
        let client = XmrBazaarClient::new();
        let endpoints = Endpoints::new(config.base_url.clone(), config.endpoints.clone());

        Self {
            auth,
            client,
            endpoints,
            monero_address: config.monero_address.clone(),
            stock_mode_field: Mutex::new(None),
        }
    }
}

#[async_trait]
impl PlatformAdapter for XmrBazaarAdapter {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn platform(&self) -> Platform {
        Platform::XmrBazaar
    }

    async fn fetch_inventory(&self) -> Result<Vec<PlatformInventoryItem>, SyncError> {
        // XMR Bazaar does not participate in normal poll-delta cycle.
        // Sales are detected via detect_sales() instead.
        Ok(Vec::new())
    }

    async fn fetch_all_listings(&self) -> Result<Vec<PlatformListing>, SyncError> {
        let listings = self.client.fetch_listings(&self.endpoints).await?;
        Ok(listings
            .iter()
            .map(|l| PlatformListing {
                platform_item_id: l.id.clone(),
                title: l.title.clone(),
                sku: None,
                quantity: l.quantity,
                price: l.price,
                image_url: l.image_url.clone(),
                group_key: None,
                variant_attributes: HashMap::new(),
            })
            .collect())
    }

    async fn set_quantity(&self, platform_item_id: &str, qty: i64) -> Result<(), SyncError> {
        // XMR Bazaar uses stock-mode: toggle between unlimited (active) and deactivated.
        let active = qty > 0;
        let cached_field = self.stock_mode_field.lock().unwrap().clone();
        self.client
            .set_stock_mode(
                &self.endpoints,
                platform_item_id,
                active,
                cached_field.as_ref(),
            )
            .await
    }

    async fn is_authenticated(&self) -> bool {
        self.auth.check_session(&self.client, &self.endpoints).await
    }

    async fn test_connection(&self) -> Result<String, SyncError> {
        // XMR Bazaar is session-cookie based: the only real credential check
        // is a login attempt. `login` verifies the session by fetching a
        // protected page afterwards, so success means the username/password
        // (and base URL) work.
        self.auth.login(&self.client, &self.endpoints).await?;
        Ok("Connected to XMR Bazaar — login succeeded".to_string())
    }

    async fn refresh_auth(&self) -> Result<(), SyncError> {
        self.auth.login(&self.client, &self.endpoints).await
    }

    async fn detect_sales(&self) -> Result<Vec<SaleDetection>, SyncError> {
        // No in-memory deduplication here: the sync engine's
        // `xmr_processed_orders` table is the only authority on which orders
        // have been consumed. A session-level set used to mark orders seen
        // before the engine processed them, so a dry run — or any cycle that
        // failed after the scrape — made the next real cycle skip the sale.
        let records = self.client.fetch_recent_sales(&self.endpoints).await?;

        let mut sales = Vec::new();

        for record in records {
            debug!(
                order_id = %record.order_id,
                listing_id = %record.listing_id,
                quantity = record.quantity,
                "Detected XMR Bazaar sale"
            );

            sales.push(SaleDetection {
                platform_item_id: record.listing_id,
                units_sold: record.quantity,
                order_id: Some(record.order_id),
                sold_at: record.completed_at,
            });
        }

        Ok(sales)
    }

    async fn fetch_full_listing(&self, platform_item_id: &str) -> Result<FullListing, SyncError> {
        // Fetch all listings to find the matching one for basic info
        let listings = self.client.fetch_listings(&self.endpoints).await?;
        let listing = listings
            .iter()
            .find(|l| l.id == platform_item_id)
            .ok_or_else(|| SyncError::ApiError {
                platform: Platform::XmrBazaar,
                message: format!("Listing {platform_item_id} not found"),
            })?;

        // Fetch detailed info from the edit page
        let detail = self
            .client
            .fetch_listing_detail(&self.endpoints, platform_item_id)
            .await?;

        let now = Utc::now().to_rfc3339();

        // Build price, preferring detail price over listing price
        let currency = detail.currency.clone().unwrap_or_else(|| "USD".to_string());
        let price = detail.price.or(listing.price).map(|amount| ListingPrice {
            amount,
            currency: currency.clone(),
        });

        // Build description
        let description = detail.description.as_ref().map(|desc| ListingDescription {
            platform: Platform::XmrBazaar,
            platform_item_id: platform_item_id.to_string(),
            html: Some(desc.clone()),
            plain_text: Some(desc.clone()),
            fetched_at: now.clone(),
        });

        // Build photos list from detail, falling back to listing photos
        let photo_urls = if !detail.photos.is_empty() {
            &detail.photos
        } else {
            &listing.photos
        };

        let photos: Vec<ListingPhoto> = photo_urls
            .iter()
            .enumerate()
            .map(|(i, url)| ListingPhoto {
                url: url.clone(),
                position: i as i32,
                width: None,
                height: None,
                alt_text: None,
            })
            .collect();

        let listing_url = format!(
            "{}/listing/{}/",
            self.endpoints.base_url().trim_end_matches('/'),
            platform_item_id
        );

        Ok(FullListing {
            platform: Platform::XmrBazaar,
            platform_item_id: platform_item_id.to_string(),
            title: listing.title.clone(),
            sku: None,
            quantity: listing.quantity,
            price,
            description,
            photos,
            url: Some(listing_url),
            fetched_at: now,
            extras: std::collections::HashMap::new(),
            variant_id: None,
        })
    }

    async fn fetch_description(
        &self,
        platform_item_id: &str,
    ) -> Result<Option<ListingDescription>, SyncError> {
        let detail = self
            .client
            .fetch_listing_detail(&self.endpoints, platform_item_id)
            .await?;

        let now = Utc::now().to_rfc3339();

        Ok(detail.description.map(|desc| ListingDescription {
            platform: Platform::XmrBazaar,
            platform_item_id: platform_item_id.to_string(),
            html: Some(desc.clone()),
            plain_text: Some(desc),
            fetched_at: now,
        }))
    }

    async fn fetch_photos(&self, platform_item_id: &str) -> Result<Vec<ListingPhoto>, SyncError> {
        let detail = self
            .client
            .fetch_listing_detail(&self.endpoints, platform_item_id)
            .await?;

        let photos = detail
            .photos
            .iter()
            .enumerate()
            .map(|(i, url)| ListingPhoto {
                url: url.clone(),
                position: i as i32,
                width: None,
                height: None,
                alt_text: None,
            })
            .collect();

        Ok(photos)
    }

    async fn fetch_price(&self, platform_item_id: &str) -> Result<Option<ListingPrice>, SyncError> {
        // First try to get price from the edit page detail (more accurate)
        let detail = self
            .client
            .fetch_listing_detail(&self.endpoints, platform_item_id)
            .await?;

        let currency = detail.currency.clone().unwrap_or_else(|| "USD".to_string());
        if let Some(amount) = detail.price {
            return Ok(Some(ListingPrice { amount, currency }));
        }

        // Fall back to listings page price
        let listings = self.client.fetch_listings(&self.endpoints).await?;
        let price = listings
            .iter()
            .find(|l| l.id == platform_item_id)
            .and_then(|l| l.price)
            .map(|amount| ListingPrice {
                amount,
                currency: currency.clone(),
            });

        Ok(price)
    }

    async fn set_price(
        &self,
        platform_item_id: &str,
        price: ListingPrice,
    ) -> Result<(), SyncError> {
        // The currency travels with the amount in a single form submission:
        // writing `price` alone left the listing's old currency in place, so
        // a USD price could silently become an EUR (or XMR) one.
        let fields = price_update_fields(&price);
        let updates: Vec<(&str, &str)> = fields
            .iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect();
        self.client
            .update_listing_fields(&self.endpoints, platform_item_id, &updates)
            .await
    }

    async fn set_description(&self, platform_item_id: &str, html: &str) -> Result<(), SyncError> {
        // XMR Bazaar description fields are plain text: strip tags exactly as
        // `create_listing` does, instead of posting raw HTML that renders as
        // literal markup on the listing.
        let plain_text = description_update_value(html);
        // Try the most common field name first; update_listing_field will
        // fall back to adding the field if it doesn't already exist.
        self.client
            .update_listing_field(
                &self.endpoints,
                platform_item_id,
                "description",
                &plain_text,
            )
            .await
    }

    async fn update_listing(
        &self,
        platform_item_id: &str,
        request: UpdateListingRequest,
    ) -> Result<(), SyncError> {
        if let Some(desc) = &request.description_html {
            self.set_description(platform_item_id, desc).await?;
        }
        if let Some(price) = &request.price {
            self.set_price(platform_item_id, price.clone()).await?;
        }
        Ok(())
    }

    async fn create_listing(&self, request: CreateListingRequest) -> Result<String, SyncError> {
        self.client
            .create_listing(&self.endpoints, &request, &self.monero_address)
            .await
    }

    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            can_fetch_full_listing: true,
            can_fetch_description: true,
            can_fetch_photos: true,
            can_fetch_price: true,
            can_set_price: true,
            can_set_description: true,
            can_upload_photos: false,
            can_create_listing: true,
            has_stock_mode_inventory: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{description_update_value, price_update_fields, XmrBazaarAdapter};
    use nisaba_core::config::XmrBazaarConfig;
    use nisaba_core::traits::PlatformAdapter;
    use nisaba_core::types::ListingPrice;
    use rust_decimal::Decimal;
    use std::collections::HashMap;
    use std::str::FromStr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Minimal login page the form parser accepts: a form with a password
    /// field, a text username field, and a long hidden validation token.
    const LOGIN_PAGE: &str = r#"<html><body><form action="/login/" method="post"><input type="hidden" name="validation" value="abcdefghijklmnopqrstuvwxyz0123456789ABCDEFGHIJKL"><input type="text" name="username" value=""><input type="password" name="password" value=""><input type="submit" value="Login"></form></body></html>"#;

    async fn read_request(socket: &mut tokio::net::TcpStream) -> (String, String) {
        let mut buf = vec![0u8; 8192];
        let mut req = Vec::new();
        let mut content_length = 0usize;
        // Read headers first.
        loop {
            let n = socket.read(&mut buf).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            req.extend_from_slice(&buf[..n]);
            if let Some(pos) = req.windows(4).position(|w| w == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&req[..pos]).to_string();
                for line in headers.lines().skip(1) {
                    if let Some(v) = line.strip_prefix("content-length:") {
                        content_length = v.trim().parse().unwrap_or(0);
                    } else if let Some(v) = line.strip_prefix("Content-Length:") {
                        content_length = v.trim().parse().unwrap_or(0);
                    }
                }
                let body_read = req.len() - (pos + 4);
                let mut remaining = content_length.saturating_sub(body_read);
                while remaining > 0 {
                    let n = socket.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    remaining = remaining.saturating_sub(n);
                }
                break;
            }
        }
        let head = String::from_utf8_lossy(&req).to_string();
        let mut parts = head.split_whitespace();
        (
            parts.next().unwrap_or("").to_string(),
            parts.next().unwrap_or("").to_string(),
        )
    }

    /// Fake XMR Bazaar site. `login_post_status` controls what POST /login/
    /// returns: 302 for a successful login, anything else for a failure.
    async fn spawn_fake(login_post_status: u16) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let (method, path) = read_request(&mut socket).await;
                    let (status, headers, body): (u16, Vec<(&str, &str)>, &str) =
                        match (method.as_str(), path.as_str()) {
                            ("GET", "/login/") => (200, vec![], LOGIN_PAGE),
                            ("POST", "/login/") => {
                                if login_post_status == 302 {
                                    (
                                        302,
                                        vec![
                                            ("location", "/"),
                                            ("set-cookie", "PHPSESSID=fake-session; Path=/"),
                                        ],
                                        "",
                                    )
                                } else {
                                    (login_post_status, vec![], "<html>login failed</html>")
                                }
                            }
                            ("GET", "/my-listings/") => {
                                (200, vec![], "<html><body>my listings</body></html>")
                            }
                            _ => (404, vec![], ""),
                        };
                    let reason = match status {
                        200 => "OK",
                        302 => "Found",
                        _ => "Status",
                    };
                    let mut resp = format!(
                        "HTTP/1.1 {status} {reason}\r\ncontent-length: {}\r\nconnection: close\r\n",
                        body.len()
                    );
                    for (k, v) in headers {
                        resp.push_str(&format!("{k}: {v}\r\n"));
                    }
                    resp.push_str("\r\n");
                    resp.push_str(body);
                    let _ = socket.write_all(resp.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn adapter_for_server(base: String) -> XmrBazaarAdapter {
        XmrBazaarAdapter::new(&XmrBazaarConfig {
            enabled: true,
            username: "seller".to_string(),
            password: "s3cret".to_string(),
            base_url: base,
            monero_address: String::new(),
            endpoints: HashMap::new(),
        })
    }

    /// N-11.1: test_connection performs a real login against the fake site.
    #[tokio::test]
    async fn test_connection_login_succeeds() {
        let base = spawn_fake(302).await;
        let adapter = adapter_for_server(base);
        let summary = adapter
            .test_connection()
            .await
            .expect("a 302 login must succeed");
        assert!(summary.contains("XMR Bazaar"), "got: {summary}");
        assert!(summary.to_lowercase().contains("login"), "got: {summary}");
    }

    /// N-11.1: a rejected login surfaces as an auth error.
    #[tokio::test]
    async fn test_connection_login_failure_is_an_auth_error() {
        let base = spawn_fake(200).await;
        let adapter = adapter_for_server(base);
        let err = adapter
            .test_connection()
            .await
            .expect_err("a failed login must be Err");
        assert!(err.to_string().contains("Login failed"), "got: {err}");
    }

    #[test]
    fn a_price_write_keeps_the_currency_with_the_amount() {
        let fields = price_update_fields(&ListingPrice {
            amount: Decimal::from_str("48.5").unwrap(),
            currency: "eur".to_string(),
        });
        assert_eq!(
            fields,
            vec![
                ("price".to_string(), "48.5".to_string()),
                ("currency".to_string(), "EUR".to_string()),
            ],
            "dropping the currency left the listing's old currency in place"
        );
    }

    #[test]
    fn a_computed_price_is_serialized_exactly_not_as_f64_artifact() {
        // N-6.1: 0.1 + 0.2 in f64 is 0.30000000000000004; the XMR form must
        // receive "0.3".
        let amount = Decimal::from_str("0.1").unwrap() + Decimal::from_str("0.2").unwrap();
        let fields = price_update_fields(&ListingPrice {
            amount,
            currency: "USD".to_string(),
        });
        let price_field = fields
            .iter()
            .find(|(k, _)| k == "price")
            .map(|(_, v)| v.as_str())
            .expect("price field");
        assert_eq!(price_field, "0.3");
        assert!(
            !price_field.contains("0.30000000000000004"),
            "f64 artifact leaked into the XMR price field: {price_field}"
        );
    }

    #[test]
    fn a_description_write_strips_html_like_create_listing_does() {
        let value = description_update_value("<p>Six-pocket <b>ripstop</b> pants.</p>");
        assert!(!value.contains('<'), "raw HTML must not be posted: {value}");
        assert!(value.contains("Six-pocket"));
        assert!(value.contains("ripstop"));
        assert!(value.contains("pants."));
    }
}
