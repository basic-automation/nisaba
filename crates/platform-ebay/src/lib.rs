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
    CreateListingRequest, FullListing, ListingDescription, ListingPhoto, ListingPrice,
    Platform, PlatformCapabilities, PlatformInventoryItem, PlatformListing, UpdateListingRequest,
};
use tracing::{debug, info, warn};

use crate::types::{
    EbayCreateItemAvailability, EbayCreateItemProduct, EbayCreateItemRequest,
    EbayCreateOfferRequest, EbayCreateOfferPricing, EbayCreateAmount, EbayListingPolicies,
    EbayCategorySuggestion, EbayAspectMetadata, EbayBusinessPolicy,
    ShipToLocationUpdate,
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
        let auth = EbayAuth::new(
            config.client_id.clone(),
            config.client_secret.clone(),
            config.redirect_uri.clone(),
            &config.environment,
        );

        let client = EbayClient::new(config.environment != "production");

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

    /// Get the authorization URL for initial OAuth setup.
    pub fn authorization_url(&self) -> String {
        self.auth.authorization_url()
    }

    /// Complete the OAuth flow by exchanging an authorization code.
    pub async fn complete_auth(&self, code: &str) -> Result<(), SyncError> {
        let token = self.auth.exchange_code(self.client.http(), code).await?;

        let expires_at = Utc::now()
            + chrono::Duration::seconds(token.expires_in);
        let expires_str = expires_at.format("%Y-%m-%d %H:%M:%S").to_string();

        self.db
            .upsert_auth_token(
                "ebay",
                &token.access_token,
                token.refresh_token.as_deref(),
                Some(&expires_str),
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
                Some(exp) => Utc::now() + chrono::Duration::seconds(TOKEN_REFRESH_MARGIN_SECS) >= exp,
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
        let tree_id = self.client.get_default_category_tree_id(&token, marketplace_id).await?;
        self.client.get_category_suggestions(&token, &tree_id, query).await
    }

    /// Get all business policies (fulfillment, payment, return) for the seller.
    pub async fn get_business_policies(
        &self,
        marketplace_id: &str,
    ) -> Result<(Vec<EbayBusinessPolicy>, Vec<EbayBusinessPolicy>, Vec<EbayBusinessPolicy>), SyncError> {
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
        let tree_id = self.client.get_default_category_tree_id(&token, marketplace_id).await?;
        self.client.get_item_aspects_for_category(&token, &tree_id, category_id).await
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

        let expires_at = Utc::now()
            + chrono::Duration::seconds(token.expires_in);
        let expires_str = expires_at.format("%Y-%m-%d %H:%M:%S").to_string();

        self.db
            .upsert_auth_token(
                "ebay",
                &token.access_token,
                token.refresh_token.as_deref().or(Some(&refresh)),
                Some(&expires_str),
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
                let mut sku_prices: std::collections::HashMap<String, f64> =
                    std::collections::HashMap::new();
                for offer in &offers {
                    if let (Some(sku), Some(pricing)) =
                        (&offer.sku, &offer.pricing_summary)
                    {
                        if let Some(amount) = &pricing.price {
                            if let Some(val) = amount.value.as_ref().and_then(|v| v.parse::<f64>().ok()) {
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
                let existing_skus: std::collections::HashSet<String> =
                    listings.iter().map(|l| l.platform_item_id.clone()).collect();

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

    async fn refresh_auth(&self) -> Result<(), SyncError> {
        self.do_refresh().await?;
        info!("eBay access token refreshed");
        Ok(())
    }

    async fn fetch_full_listing(
        &self,
        platform_item_id: &str,
    ) -> Result<FullListing, SyncError> {
        let token = self.get_access_token().await?;

        // Try Inventory API first (SKU-based items)
        match self.client.fetch_inventory_item(&token, platform_item_id).await {
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
        let item = self.client.fetch_inventory_item(&token, platform_item_id).await?;

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

    async fn fetch_photos(
        &self,
        platform_item_id: &str,
    ) -> Result<Vec<ListingPhoto>, SyncError> {
        let token = self.get_access_token().await?;
        let item = self.client.fetch_inventory_item(&token, platform_item_id).await?;

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

    async fn fetch_price(
        &self,
        platform_item_id: &str,
    ) -> Result<Option<ListingPrice>, SyncError> {
        let token = self.get_access_token().await?;
        let offers = self.client.fetch_offers(&token, platform_item_id).await?;

        let price = offers
            .iter()
            .filter_map(|offer| {
                let pricing = offer.pricing_summary.as_ref()?;
                let amount = pricing.price.as_ref()?;
                let value_str = amount.value.as_ref()?;
                let value: f64 = value_str.parse().ok()?;
                let currency = amount
                    .currency
                    .clone()
                    .unwrap_or_else(|| "USD".to_string());
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

        // Fetch current quantity so we don't change it
        let item = self.client.fetch_inventory_item(&token, platform_item_id).await?;
        let quantity = item
            .availability
            .as_ref()
            .and_then(|a| a.ship_to_location_availability.as_ref())
            .and_then(|s| s.quantity)
            .unwrap_or(0);

        self.client
            .bulk_update_price_quantity(
                &token,
                platform_item_id,
                price.amount,
                &price.currency,
                quantity,
            )
            .await
    }

    async fn set_description(
        &self,
        platform_item_id: &str,
        html: &str,
    ) -> Result<(), SyncError> {
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
        let token = self.get_access_token().await?;
        let extras = request.extras.as_ref();

        // Check if we need to update inventory item fields (description, aspects, condition)
        let has_inv_changes = request.description_html.is_some()
            || extras.is_some_and(|e| {
                e.contains_key("ebay_aspects")
                    || e.contains_key("ebay_condition")
                    || e.contains_key("ebay_condition_description")
            });

        if has_inv_changes {
            let aspects: Option<std::collections::HashMap<String, Vec<String>>> =
                extras.and_then(|e| e.get("ebay_aspects"))
                    .and_then(|json| serde_json::from_str(json).ok());
            let condition = extras.and_then(|e| e.get("ebay_condition")).map(|s| s.as_str());
            let condition_desc = extras.and_then(|e| e.get("ebay_condition_description")).map(|s| s.as_str());

            self.client.update_inventory_item_full(
                &token,
                platform_item_id,
                request.description_html.as_deref(),
                aspects.as_ref(),
                condition,
                condition_desc,
            ).await?;
        }

        if let Some(price) = &request.price {
            self.set_price(platform_item_id, price.clone()).await?;
        }

        // Check if we need to update offer fields (category, policies)
        let has_offer_changes = extras.is_some_and(|e| {
            e.contains_key("ebay_category_id")
                || e.contains_key("ebay_fulfillment_policy_id")
                || e.contains_key("ebay_payment_policy_id")
                || e.contains_key("ebay_return_policy_id")
        });

        if has_offer_changes {
            let offers = self.client.fetch_offers(&token, platform_item_id).await?;
            if let Some(offer) = offers.first() {
                if let Some(offer_id) = &offer.offer_id {
                    let extras = extras.unwrap();
                    let mut body = serde_json::json!({});

                    if let Some(cat_id) = extras.get("ebay_category_id") {
                        body["categoryId"] = serde_json::json!(cat_id);
                    }

                    // Build listing policies if any policy ID is provided
                    let has_policies = extras.contains_key("ebay_fulfillment_policy_id")
                        || extras.contains_key("ebay_payment_policy_id")
                        || extras.contains_key("ebay_return_policy_id");

                    if has_policies {
                        let mut policies = serde_json::json!({});
                        if let Some(id) = extras.get("ebay_fulfillment_policy_id") {
                            policies["fulfillmentPolicyId"] = serde_json::json!(id);
                        }
                        if let Some(id) = extras.get("ebay_payment_policy_id") {
                            policies["paymentPolicyId"] = serde_json::json!(id);
                        }
                        if let Some(id) = extras.get("ebay_return_policy_id") {
                            policies["returnPolicyId"] = serde_json::json!(id);
                        }
                        body["listingPolicies"] = policies;
                    }

                    self.client.update_offer(&token, offer_id, &body).await?;
                }
            }
        }

        Ok(())
    }

    async fn create_listing(
        &self,
        request: CreateListingRequest,
    ) -> Result<String, SyncError> {
        let token = self.get_access_token().await?;

        // Use the provided SKU, or generate one from the title
        let sku = request
            .sku
            .clone()
            .unwrap_or_else(|| {
                let sanitized: String = request
                    .title
                    .chars()
                    .filter(|c| c.is_alphanumeric() || *c == '-' || *c == '_')
                    .take(50)
                    .collect();
                format!("{}-{}", sanitized, chrono::Utc::now().timestamp_millis())
            });

        // Parse extras for eBay-specific fields
        let condition = request.extras.get("ebay_condition")
            .cloned()
            .unwrap_or_else(|| "NEW".to_string());
        let condition_description = request.extras.get("ebay_condition_description").cloned();
        let category_id = request.extras.get("ebay_category_id").cloned();

        let aspects: Option<std::collections::HashMap<String, Vec<String>>> =
            request.extras.get("ebay_aspects")
                .and_then(|json| serde_json::from_str(json).ok());

        let fulfillment_policy_id = request.extras.get("ebay_fulfillment_policy_id")
            .cloned().unwrap_or_default();
        let payment_policy_id = request.extras.get("ebay_payment_policy_id")
            .cloned().unwrap_or_default();
        let return_policy_id = request.extras.get("ebay_return_policy_id")
            .cloned().unwrap_or_default();

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
            amount: 0.0,
            currency: "USD".to_string(),
        });

        let offer_request = EbayCreateOfferRequest {
            sku: sku.clone(),
            marketplace_id: "EBAY_US".to_string(),
            format: request.extras.get("ebay_format")
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

        let offer_id = self
            .client
            .create_offer(&token, &offer_request)
            .await?;

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
