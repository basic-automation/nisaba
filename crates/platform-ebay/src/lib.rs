pub mod auth;
pub mod client;
pub mod mapping;
pub mod types;

use std::sync::Arc;
use tokio::sync::RwLock;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use nisaba_core::config::EbayConfig;
use nisaba_core::db::Db;
use nisaba_core::error::SyncError;
use nisaba_core::traits::PlatformAdapter;
use nisaba_core::types::{
    CreateListingRequest, FullListing, ListingDescription, ListingPhoto, ListingPrice, Platform,
    PlatformCapabilities, PlatformInventoryItem, PlatformListing, UpdateListingRequest,
};
use rust_decimal::Decimal;
use tracing::{debug, info, warn};

use crate::types::{
    EbayAspectMetadata, EbayBusinessPolicy, EbayCategorySuggestion, EbayCreateAmount,
    EbayCreateItemAvailability, EbayCreateItemProduct, EbayCreateItemRequest,
    EbayCreateOfferPricing, EbayCreateOfferRequest, EbayListingPolicies, ShipToLocationUpdate,
};

use crate::auth::EbayAuth;
use crate::client::EbayClient;

/// Refresh the access token when it expires within this many seconds.
const TOKEN_REFRESH_MARGIN_SECS: i64 = 300; // 5 minutes

pub struct EbayAdapter {
    auth: EbayAuth,
    client: EbayClient,
    db: Arc<Db>,
    access_token: RwLock<Option<String>>,
    refresh_token: RwLock<Option<String>>,
    expires_at: RwLock<Option<DateTime<Utc>>>,
}

impl EbayAdapter {
    pub async fn new(config: &EbayConfig, db: Arc<Db>) -> Result<Self, SyncError> {
        let broker = config.broker().map_err(|message| SyncError::AuthError {
            platform: Platform::Ebay,
            message,
        })?;
        let auth = EbayAuth::new(
            config.client_id.clone(),
            config.client_secret.clone(),
            config.redirect_uri.clone(),
            &config.environment,
        )
        .with_broker(broker);

        let client = EbayClient::new(config.is_sandbox());

        // Try to load existing tokens from DB
        let (access, refresh, expires_at) = if let Some(token) = db.get_auth_token("ebay").await? {
            let expires = token.expires_at.as_deref().and_then(|s| {
                chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S")
                    .ok()
                    .map(|naive| naive.and_utc())
            });
            (Some(token.access_token), token.refresh_token, expires)
        } else {
            (None, None, None)
        };

        Ok(Self {
            auth,
            client,
            db,
            access_token: RwLock::new(access),
            refresh_token: RwLock::new(refresh),
            expires_at: RwLock::new(expires_at),
        })
    }

    /// Whether sign-in goes through the Nisaba broker, which allows it to run in the
    /// user's own browser.
    pub fn uses_broker(&self) -> bool {
        self.auth.uses_broker()
    }

    /// The URL to open in the user's own browser, with the app's loopback `port` encoded in
    /// the state. `None` unless sign-in goes through the broker.
    pub fn authorization_url_for_loopback(&self, port: u16) -> Option<String> {
        self.auth.authorization_url_for_loopback(port)
    }

    /// Whether `returned` is the state issued for the sign-in in progress, without
    /// consuming it. Lets the auth window ignore callbacks that are not eBay's.
    pub fn callback_state_matches(&self, returned: Option<&str>) -> bool {
        self.auth.state_matches_pending(returned)
    }

    /// Get the authorization URL for initial OAuth setup.
    pub fn authorization_url(&self) -> String {
        self.auth.authorization_url()
    }

    /// Complete the OAuth flow by exchanging an authorization code.
    ///
    /// `returned_state` is the `state` query parameter eBay sent back on the
    /// redirect callback. It is verified against the value issued with the
    /// authorize URL *before* any token exchange; a missing or mismatched
    /// state rejects the attempt, so a code observed from any other page —
    /// or a replayed callback — cannot be redeemed.
    pub async fn complete_auth(
        &self,
        code: &str,
        returned_state: Option<&str>,
    ) -> Result<(), SyncError> {
        self.auth.verify_callback_state(returned_state)?;

        let token = self.auth.exchange_code(self.client.http(), code).await?;

        let expires_at = Utc::now() + chrono::Duration::seconds(token.expires_in);
        let expires_str = expires_at.format("%Y-%m-%d %H:%M:%S").to_string();
        // N-11.2: persist when the refresh token itself expires so the UI
        // can warn ahead of time (previously parsed but unused).
        let refresh_expires_str = token.refresh_token_expires_at(Utc::now());

        self.db
            .upsert_auth_token(
                "ebay",
                &token.access_token,
                token.refresh_token.as_deref(),
                Some(&expires_str),
                refresh_expires_str.as_deref(),
                None,
            )
            .await?;

        *self.access_token.write().await = Some(token.access_token);
        *self.refresh_token.write().await = token.refresh_token;
        *self.expires_at.write().await = Some(expires_at);

        info!("eBay OAuth setup complete");
        Ok(())
    }

    /// Returns a valid access token, auto-refreshing if expired or about to expire.
    async fn get_access_token(&self) -> Result<String, SyncError> {
        // Check if we have a token at all
        let has_token = self.access_token.read().await.is_some();
        if !has_token {
            return Err(SyncError::AuthError {
                platform: Platform::Ebay,
                message: "No access token available. Run eBay OAuth setup first.".into(),
            });
        }

        // Check if the token is expired or about to expire
        let needs_refresh = {
            let expires = self.expires_at.read().await;
            match *expires {
                Some(exp) => {
                    Utc::now() + chrono::Duration::seconds(TOKEN_REFRESH_MARGIN_SECS) >= exp
                }
                None => false, // No expiry info — assume it's still valid
            }
        };

        if needs_refresh {
            debug!("eBay access token expired or about to expire, auto-refreshing");
            match self.do_refresh().await {
                Ok(()) => {
                    info!("eBay access token auto-refreshed successfully");
                }
                Err(e) => {
                    warn!("eBay auto-refresh failed: {e}");
                    return Err(e);
                }
            }
        }

        self.access_token
            .read()
            .await
            .clone()
            .ok_or_else(|| SyncError::AuthError {
                platform: Platform::Ebay,
                message: "No access token available after refresh attempt.".into(),
            })
    }

    /// Get category suggestions for a search query.
    pub async fn get_category_suggestions(
        &self,
        query: &str,
        marketplace_id: &str,
    ) -> Result<Vec<EbayCategorySuggestion>, SyncError> {
        let token = self.get_access_token().await?;
        let tree_id = self
            .client
            .get_default_category_tree_id(&token, marketplace_id)
            .await?;
        self.client
            .get_category_suggestions(&token, &tree_id, query)
            .await
    }

    /// Get all business policies (fulfillment, payment, return) for the seller.
    pub async fn get_business_policies(
        &self,
        marketplace_id: &str,
    ) -> Result<
        (
            Vec<EbayBusinessPolicy>,
            Vec<EbayBusinessPolicy>,
            Vec<EbayBusinessPolicy>,
        ),
        SyncError,
    > {
        let token = self.get_access_token().await?;
        let (fulfillment, payment, ret) = tokio::try_join!(
            self.client.get_fulfillment_policies(&token, marketplace_id),
            self.client.get_payment_policies(&token, marketplace_id),
            self.client.get_return_policies(&token, marketplace_id),
        )?;
        Ok((fulfillment, payment, ret))
    }

    /// Get item aspects (specifics) for a category.
    pub async fn get_category_aspects(
        &self,
        category_id: &str,
        marketplace_id: &str,
    ) -> Result<Vec<EbayAspectMetadata>, SyncError> {
        let token = self.get_access_token().await?;
        let tree_id = self
            .client
            .get_default_category_tree_id(&token, marketplace_id)
            .await?;
        self.client
            .get_item_aspects_for_category(&token, &tree_id, category_id)
            .await
    }

    /// Internal refresh logic shared by get_access_token and refresh_auth.
    async fn do_refresh(&self) -> Result<(), SyncError> {
        let refresh = self.refresh_token.read().await.clone();
        let refresh = refresh.ok_or_else(|| SyncError::AuthError {
            platform: Platform::Ebay,
            message: "No refresh token available".into(),
        })?;

        let token = self
            .auth
            .refresh_token(self.client.http(), &refresh)
            .await?;

        let expires_at = Utc::now() + chrono::Duration::seconds(token.expires_in);
        let expires_str = expires_at.format("%Y-%m-%d %H:%M:%S").to_string();

        // N-11.2: persist the refresh token's own expiry. `None` when the
        // response omits it — the upsert preserves the stored value.
        let refresh_expires_str = token.refresh_token_expires_at(Utc::now());

        self.db
            .upsert_auth_token(
                "ebay",
                &token.access_token,
                token.refresh_token.as_deref().or(Some(&refresh)),
                Some(&expires_str),
                refresh_expires_str.as_deref(),
                None,
            )
            .await?;

        *self.access_token.write().await = Some(token.access_token);
        if let Some(new_refresh) = token.refresh_token {
            *self.refresh_token.write().await = Some(new_refresh);
        }
        *self.expires_at.write().await = Some(expires_at);

        Ok(())
    }
}

#[async_trait]
impl PlatformAdapter for EbayAdapter {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn platform(&self) -> Platform {
        Platform::Ebay
    }

    async fn fetch_inventory(&self) -> Result<Vec<PlatformInventoryItem>, SyncError> {
        let token = self.get_access_token().await?;
        let items = self.client.fetch_inventory_items(&token).await?;
        Ok(items.iter().map(mapping::to_inventory_item).collect())
    }

    async fn fetch_all_listings(&self) -> Result<Vec<PlatformListing>, SyncError> {
        let token = self.get_access_token().await?;

        // Fetch from Inventory API (items created via API)
        let inv_items = self.client.fetch_inventory_items(&token).await?;
        let mut listings: Vec<PlatformListing> =
            inv_items.iter().map(mapping::to_listing).collect();

        // Fetch all offers to get prices for inventory items
        match self.client.fetch_all_offers(&token).await {
            Ok(offers) => {
                // Build SKU → price map from offers
                let mut sku_prices: std::collections::HashMap<String, Decimal> =
                    std::collections::HashMap::new();
                for offer in &offers {
                    if let (Some(sku), Some(pricing)) = (&offer.sku, &offer.pricing_summary) {
                        if let Some(amount) = &pricing.price {
                            if let Some(val) = amount
                                .value
                                .as_ref()
                                .and_then(|v| v.parse::<Decimal>().ok())
                            {
                                sku_prices.insert(sku.clone(), val);
                            }
                        }
                    }
                }
                // Merge prices into inventory listings
                for listing in &mut listings {
                    if listing.price.is_none() {
                        if let Some(price) = sku_prices.get(&listing.platform_item_id) {
                            listing.price = Some(*price);
                        }
                    }
                }
            }
            Err(e) => {
                tracing::warn!("Failed to fetch offers for pricing: {e}");
            }
        }

        // Also fetch from Trading API (legacy listings from eBay website)
        match self.client.fetch_active_listings_trading(&token).await {
            Ok(trading_items) => {
                // Collect existing SKUs to avoid duplicates
                let existing_skus: std::collections::HashSet<String> = listings
                    .iter()
                    .map(|l| l.platform_item_id.clone())
                    .collect();

                for item in &trading_items {
                    // Skip if we already have this item via Inventory API (matched by SKU)
                    if let Some(sku) = &item.sku {
                        if existing_skus.contains(sku) {
                            continue;
                        }
                    }
                    listings.push(mapping::trading_to_listing(item));
                }
            }
            Err(e) => {
                tracing::warn!("Trading API fetch failed (legacy listings won't appear): {e}");
            }
        }

        Ok(listings)
    }

    async fn set_quantity(&self, platform_item_id: &str, qty: i64) -> Result<(), SyncError> {
        let token = self.get_access_token().await?;
        self.client
            .bulk_update_quantity(&token, vec![(platform_item_id.to_string(), qty)])
            .await
    }

    async fn is_authenticated(&self) -> bool {
        self.access_token.read().await.is_some()
    }

    async fn test_connection(&self) -> Result<String, SyncError> {
        // A single inventory page is the cheapest authenticated read the
        // Inventory API offers; the total tells the user the connection
        // really reaches their seller account. `get_access_token` refreshes
        // an expired token first, so this also validates the refresh token.
        let token = self.get_access_token().await?;
        let page = self.client.fetch_inventory_page(&token, 1, 0).await?;
        let total = page.total.unwrap_or_default();
        Ok(format!(
            "Connected to eBay — credentials accepted, {total} inventory item(s) visible"
        ))
    }

    async fn refresh_auth(&self) -> Result<(), SyncError> {
        self.do_refresh().await?;
        info!("eBay access token refreshed");
        Ok(())
    }

    async fn fetch_full_listing(&self, platform_item_id: &str) -> Result<FullListing, SyncError> {
        let token = self.get_access_token().await?;

        // Try Inventory API first (SKU-based items)
        match self
            .client
            .fetch_inventory_item(&token, platform_item_id)
            .await
        {
            Ok(item) => {
                let offers = self.client.fetch_offers(&token, platform_item_id).await?;
                Ok(mapping::to_full_listing(&item, &offers))
            }
            Err(_) => {
                // Fall back to Trading API (ItemID-based legacy listings)
                let item = self
                    .client
                    .fetch_item_trading(&token, platform_item_id)
                    .await?;
                Ok(mapping::trading_to_full_listing(&item))
            }
        }
    }

    async fn fetch_description(
        &self,
        platform_item_id: &str,
    ) -> Result<Option<ListingDescription>, SyncError> {
        let token = self.get_access_token().await?;
        let item = self
            .client
            .fetch_inventory_item(&token, platform_item_id)
            .await?;

        let description = item
            .product
            .as_ref()
            .and_then(|p| p.description.clone())
            .map(|desc| ListingDescription {
                platform: Platform::Ebay,
                platform_item_id: platform_item_id.to_string(),
                html: Some(desc),
                plain_text: None,
                fetched_at: chrono::Utc::now().to_rfc3339(),
            });

        Ok(description)
    }

    async fn fetch_photos(&self, platform_item_id: &str) -> Result<Vec<ListingPhoto>, SyncError> {
        let token = self.get_access_token().await?;
        let item = self
            .client
            .fetch_inventory_item(&token, platform_item_id)
            .await?;

        let photos = item
            .product
            .as_ref()
            .and_then(|p| p.image_urls.as_ref())
            .map(|urls| {
                urls.iter()
                    .enumerate()
                    .map(|(i, url)| ListingPhoto {
                        url: url.clone(),
                        position: i as i32,
                        width: None,
                        height: None,
                        alt_text: None,
                    })
                    .collect()
            })
            .unwrap_or_default();

        Ok(photos)
    }

    async fn fetch_price(&self, platform_item_id: &str) -> Result<Option<ListingPrice>, SyncError> {
        let token = self.get_access_token().await?;
        let offers = self.client.fetch_offers(&token, platform_item_id).await?;

        let price = offers
            .iter()
            .filter_map(|offer| {
                let pricing = offer.pricing_summary.as_ref()?;
                let amount = pricing.price.as_ref()?;
                let value_str = amount.value.as_ref()?;
                let value: Decimal = value_str.parse().ok()?;
                let currency = amount.currency.clone().unwrap_or_else(|| "USD".to_string());
                Some(ListingPrice {
                    amount: value,
                    currency,
                })
            })
            .next();

        Ok(price)
    }

    async fn set_price(
        &self,
        platform_item_id: &str,
        price: ListingPrice,
    ) -> Result<(), SyncError> {
        let token = self.get_access_token().await?;

        // A price update must target a real offer: without an offerId
        // eBay cannot apply it. Fail loudly instead of sending an
        // untargeted offer entry that silently does nothing.
        let offers = self.client.fetch_offers(&token, platform_item_id).await?;
        let offer_id = offers
            .iter()
            .find_map(|o| o.offer_id.clone())
            .ok_or_else(|| SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!(
                    "No eBay offer found for SKU {platform_item_id}; cannot set price without an offerId"
                ),
            })?;

        // Never fetch or write a quantity on the price path: the bulk
        // request carries only the price for the targeted offer, so a
        // price edit can never zero stock.
        self.client
            .bulk_update_price_quantity(
                &token,
                platform_item_id,
                &offer_id,
                price.amount,
                &price.currency,
            )
            .await
    }

    async fn set_description(&self, platform_item_id: &str, html: &str) -> Result<(), SyncError> {
        let token = self.get_access_token().await?;
        self.client
            .update_inventory_item(&token, platform_item_id, html)
            .await
    }

    async fn upload_photo(
        &self,
        _platform_item_id: &str,
        _url: &str,
        _position: i32,
    ) -> Result<String, SyncError> {
        Err(SyncError::Other(
            "eBay does not support individual photo upload via the Inventory API. \
             Photos must be set as imageUrls when creating or updating an inventory item."
                .to_string(),
        ))
    }

    async fn update_listing(
        &self,
        platform_item_id: &str,
        request: UpdateListingRequest,
    ) -> Result<(), SyncError> {
        // eBay has no tag concept on inventory items or offers; silently
        // dropping a requested tag edit would report success for a change
        // that never happened, so refuse it explicitly.
        if request.tags.as_ref().is_some_and(|t| !t.is_empty()) {
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: "eBay update_listing cannot apply tags: the Inventory \
                          and Offer APIs have no tags field"
                    .into(),
            });
        }

        let token = self.get_access_token().await?;
        let extras = request.extras.as_ref();

        // Check if we need to update inventory item fields (title,
        // description, aspects, condition). The client re-reads the item
        // and rebuilds the full-replace body losslessly, omitting quantity.
        let has_inv_changes = request.title.is_some()
            || request.description_html.is_some()
            || extras.is_some_and(|e| {
                e.contains_key("ebay_aspects")
                    || e.contains_key("ebay_condition")
                    || e.contains_key("ebay_condition_description")
            });

        if has_inv_changes {
            let aspects: Option<std::collections::HashMap<String, Vec<String>>> = extras
                .and_then(|e| e.get("ebay_aspects"))
                .and_then(|json| serde_json::from_str(json).ok());
            let condition = extras
                .and_then(|e| e.get("ebay_condition"))
                .map(|s| s.as_str());
            let condition_desc = extras
                .and_then(|e| e.get("ebay_condition_description"))
                .map(|s| s.as_str());

            self.client
                .update_inventory_item_full(
                    &token,
                    platform_item_id,
                    &crate::client::InventoryItemOverrides {
                        description: request.description_html.as_deref(),
                        title: request.title.as_deref(),
                        aspects: aspects.as_ref(),
                        condition,
                        condition_description: condition_desc,
                    },
                )
                .await?;
        }

        if let Some(price) = &request.price {
            self.set_price(platform_item_id, price.clone()).await?;
        }

        // Check if we need to update offer fields (category, policies).
        // `updateOffer` is a full replace, so this must send a complete
        // offer or fail explicitly — never the partial body this code used
        // to build from just the changed keys (N-5.2).
        let has_offer_changes = extras.is_some_and(|e| {
            e.contains_key("ebay_category_id")
                || e.contains_key("ebay_fulfillment_policy_id")
                || e.contains_key("ebay_payment_policy_id")
                || e.contains_key("ebay_return_policy_id")
        });

        if has_offer_changes {
            let extras = extras.unwrap();
            let offers = self.client.fetch_offers(&token, platform_item_id).await?;
            let offer_id = offers
                .first()
                .and_then(|o| o.offer_id.clone())
                .ok_or_else(|| SyncError::ApiError {
                    platform: Platform::Ebay,
                    message: format!(
                        "cannot update offer fields for {platform_item_id}: \
                         no eBay offer exists for this SKU"
                    ),
                })?;

            let overrides = crate::client::OfferOverrides {
                category_id: extras.get("ebay_category_id").map(|s| s.as_str()),
                fulfillment_policy_id: extras.get("ebay_fulfillment_policy_id").map(|s| s.as_str()),
                payment_policy_id: extras.get("ebay_payment_policy_id").map(|s| s.as_str()),
                return_policy_id: extras.get("ebay_return_policy_id").map(|s| s.as_str()),
                listing_description: None,
            };
            // Fetches the complete offer and PUTs a complete body built
            // from it; errors explicitly if that is impossible.
            self.client
                .update_offer_fields(&token, &offer_id, &overrides)
                .await?;
        }

        Ok(())
    }

    async fn create_listing(&self, request: CreateListingRequest) -> Result<String, SyncError> {
        let token = self.get_access_token().await?;

        // Use the provided SKU, or generate one from the title
        let sku = request.sku.clone().unwrap_or_else(|| {
            let sanitized: String = request
                .title
                .chars()
                .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                .take(50)
                .collect();
            format!("{}-{}", sanitized, chrono::Utc::now().timestamp_millis())
        });

        // Parse extras for eBay-specific fields
        let condition = request
            .extras
            .get("ebay_condition")
            .cloned()
            .unwrap_or_else(|| "NEW".to_string());
        let condition_description = request.extras.get("ebay_condition_description").cloned();
        let category_id = request.extras.get("ebay_category_id").cloned();

        let aspects: Option<std::collections::HashMap<String, Vec<String>>> = request
            .extras
            .get("ebay_aspects")
            .and_then(|json| serde_json::from_str(json).ok());

        let fulfillment_policy_id = request
            .extras
            .get("ebay_fulfillment_policy_id")
            .cloned()
            .unwrap_or_default();
        let payment_policy_id = request
            .extras
            .get("ebay_payment_policy_id")
            .cloned()
            .unwrap_or_default();
        let return_policy_id = request
            .extras
            .get("ebay_return_policy_id")
            .cloned()
            .unwrap_or_default();

        // Step 1: Create the inventory item
        let create_item = EbayCreateItemRequest {
            product: EbayCreateItemProduct {
                title: request.title.clone(),
                description: request.description_html.clone(),
                image_urls: if request.photo_urls.is_empty() {
                    None
                } else {
                    Some(request.photo_urls.clone())
                },
                aspects,
            },
            availability: Some(EbayCreateItemAvailability {
                ship_to_location_availability: ShipToLocationUpdate {
                    quantity: request.quantity,
                },
            }),
            condition: Some(condition),
            condition_description,
        };

        self.client
            .create_inventory_item(&token, &sku, &create_item)
            .await?;

        // Step 2: Create an offer for this item
        let price = request.price.unwrap_or(ListingPrice {
            amount: Decimal::ZERO,
            currency: "USD".to_string(),
        });

        let offer_request = EbayCreateOfferRequest {
            sku: sku.clone(),
            marketplace_id: "EBAY_US".to_string(),
            format: request
                .extras
                .get("ebay_format")
                .cloned()
                .unwrap_or_else(|| "FIXED_PRICE".to_string()),
            available_quantity: request.quantity,
            pricing_summary: EbayCreateOfferPricing {
                price: EbayCreateAmount {
                    value: format!("{:.2}", price.amount),
                    currency: price.currency.clone(),
                },
            },
            listing_policies: EbayListingPolicies {
                fulfillment_policy_id,
                payment_policy_id,
                return_policy_id,
            },
            category_id,
        };

        let offer_id = self.client.create_offer(&token, &offer_request).await?;

        // Step 3: Publish the offer to make it live
        let listing_id = self.client.publish_offer(&token, &offer_id).await?;

        info!(
            sku = %sku,
            offer_id = %offer_id,
            listing_id = %listing_id,
            "Created and published eBay listing"
        );

        Ok(sku)
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
            has_stock_mode_inventory: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> EbayConfig {
        EbayConfig {
            enabled: true,
            client_id: "test-client-id".to_string(),
            client_secret: "test-client-secret".to_string(),
            redirect_uri: "https://localhost/callback".to_string(),
            environment: "sandbox".to_string(),
            broker_url: String::new(),
        }
    }

    async fn test_adapter() -> (EbayAdapter, tempfile::TempDir) {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("test.db");
        let db = Arc::new(Db::open(db_path.to_str().unwrap()).await.expect("open db"));
        db.migrate().await.expect("migrate db");
        let adapter = EbayAdapter::new(&test_config(), db).await.expect("adapter");
        (adapter, dir)
    }

    /// The adapter-level callback rejects a code whose state does not match
    /// the issued attempt — before any network call is made.
    #[tokio::test]
    async fn complete_auth_rejects_mismatched_state() {
        let (adapter, _dir) = test_adapter().await;
        adapter.authorization_url(); // issues and stores a state for this attempt
        let err = adapter
            .complete_auth("attacker-observed-code", Some("forged-state"))
            .await
            .expect_err("a mismatched state must reject the callback");
        assert!(
            err.to_string().contains("state"),
            "error should mention the state check, got: {err}"
        );
    }

    /// A callback with no state at all is rejected as well.
    #[tokio::test]
    async fn complete_auth_rejects_missing_state() {
        let (adapter, _dir) = test_adapter().await;
        adapter.authorization_url();
        assert!(adapter.complete_auth("some-code", None).await.is_err());
    }

    // --- Fake-server helpers for write-path tests ---

    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            if let Some(header_end) = find_subslice(&buf, b"\r\n\r\n") {
                let header_text = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
                let content_length: usize = header_text
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                if buf.len() >= header_end + 4 + content_length {
                    break;
                }
            }
            match socket.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    /// Handler returns (status, body) for a raw request.
    async fn spawn_routing_server<F>(handler: F) -> (String, std::sync::Arc<Mutex<Vec<String>>>)
    where
        F: Fn(&str) -> (u16, String) + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let captured: std::sync::Arc<Mutex<Vec<String>>> =
            std::sync::Arc::new(Mutex::new(Vec::new()));
        let captured_clone = captured.clone();
        let handler = std::sync::Arc::new(handler);
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let captured_inner = captured_clone.clone();
                let handler_inner = handler.clone();
                tokio::spawn(async move {
                    let req = read_request(&mut socket).await;
                    let (status, body) = handler_inner(&req);
                    captured_inner.lock().unwrap().push(req);
                    let reason = match status {
                        200 => "OK",
                        207 => "Multi-Status",
                        _ => "Status",
                    };
                    let resp = format!(
                        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });
        (format!("http://127.0.0.1:{port}"), captured)
    }

    async fn adapter_for_server(base: String) -> (EbayAdapter, tempfile::TempDir) {
        let (mut adapter, dir) = test_adapter().await;
        adapter.client = EbayClient::with_api_base(base);
        *adapter.access_token.write().await = Some("test-token".to_string());
        (adapter, dir)
    }

    const REJECTED_BULK: &str = r#"{"responses":[{"sku":"SKU-1","statusCode":400,"errors":[{"errorId":25021,"message":"Rejected by eBay"}]}]}"#;

    /// set_quantity: a rejected per-SKU write must surface as Err.
    #[tokio::test]
    async fn set_quantity_rejected_write_surfaces_as_err() {
        let (base, _captured) = spawn_routing_server(|_req| (207, REJECTED_BULK.to_string())).await;
        let (adapter, _dir) = adapter_for_server(base).await;
        let err = adapter
            .set_quantity("SKU-1", 3)
            .await
            .expect_err("rejected set_quantity must be Err");
        assert!(err.to_string().contains("SKU-1"), "got: {err}");
    }

    /// set_price: a rejected per-SKU write must surface as Err, the bulk
    /// request must target the fetched offerId, and it must not write
    /// any quantity.
    #[tokio::test]
    async fn set_price_rejected_write_surfaces_as_err() {
        let (base, captured) = spawn_routing_server(|req| {
            if req.starts_with("GET") {
                (
                    200,
                    r#"{"offers":[{"offerId":"offer-99","sku":"SKU-1"}],"total":1}"#.to_string(),
                )
            } else {
                (207, REJECTED_BULK.to_string())
            }
        })
        .await;
        let (adapter, _dir) = adapter_for_server(base).await;
        let err = adapter
            .set_price(
                "SKU-1",
                ListingPrice {
                    amount: Decimal::new(1999, 2),
                    currency: "USD".to_string(),
                },
            )
            .await
            .expect_err("rejected set_price must be Err");
        assert!(err.to_string().contains("SKU-1"), "got: {err}");
        let requests = captured.lock().unwrap();
        let post = requests
            .iter()
            .find(|r| r.starts_with("POST"))
            .expect("bulk POST should have been sent");
        let body = post.split("\r\n\r\n").nth(1).unwrap_or("");
        assert!(body.contains("\"offerId\":\"offer-99\""), "body: {body}");
        assert!(
            !body.contains("quantity"),
            "price path must never write quantity, body: {body}"
        );
        assert!(!body.contains("shipToLocationAvailability"), "body: {body}");
    }

    /// set_price with no offer for the SKU fails instead of sending an
    /// untargeted price (and never touches quantity).
    #[tokio::test]
    async fn set_price_without_offer_returns_err_and_sends_no_write() {
        let (base, captured) =
            spawn_routing_server(|_req| (200, r#"{"offers":[],"total":0}"#.to_string())).await;
        let (adapter, _dir) = adapter_for_server(base).await;
        assert!(adapter
            .set_price(
                "SKU-1",
                ListingPrice {
                    amount: Decimal::new(999, 2),
                    currency: "USD".to_string(),
                },
            )
            .await
            .is_err());
        let requests = captured.lock().unwrap();
        assert!(
            requests.iter().all(|r| !r.starts_with("POST")),
            "no write should be sent without an offerId: {requests:?}"
        );
    }

    /// N-11.1: test_connection probes a single inventory page and reports
    /// the server-side total.
    #[tokio::test]
    async fn test_connection_reports_inventory_total() {
        let (base, _captured) = spawn_routing_server(|req| {
            if req.contains("/sell/inventory/v1/inventory_item") {
                (200, r#"{"inventoryItems":[],"total":7}"#.to_string())
            } else {
                (404, "{}".to_string())
            }
        })
        .await;
        let (adapter, _dir) = adapter_for_server(base).await;
        let summary = adapter
            .test_connection()
            .await
            .expect("a 200 probe must succeed");
        assert!(
            summary.contains('7'),
            "summary should report the total, got: {summary}"
        );
        assert!(summary.contains("eBay"), "got: {summary}");
    }

    /// N-11.1: a revoked token surfaces as an auth error, not a panic.
    #[tokio::test]
    async fn test_connection_surfaces_revoked_token() {
        let (base, _captured) =
            spawn_routing_server(|_req| (401, r#"{"error":"invalid_token"}"#.to_string())).await;
        let (adapter, _dir) = adapter_for_server(base).await;
        let err = adapter
            .test_connection()
            .await
            .expect_err("a 401 probe must be Err");
        assert!(
            err.to_string().to_lowercase().contains("token") || err.to_string().contains("expired"),
            "error should name the token problem, got: {err}"
        );
    }
}
