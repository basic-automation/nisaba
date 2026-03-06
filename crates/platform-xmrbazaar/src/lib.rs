pub mod auth;
pub mod client;
pub mod endpoints;
pub mod scraper;
pub mod types;

use std::collections::{HashMap, HashSet};
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

pub struct XmrBazaarAdapter {
    auth: XmrBazaarAuth,
    client: XmrBazaarClient,
    endpoints: Endpoints,
    monero_address: String,
    /// Cached stock mode field discovery result.
    stock_mode_field: Mutex<Option<StockModeField>>,
    /// Session-level deduplication of seen order IDs.
    seen_orders: Mutex<HashSet<String>>,
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
            seen_orders: Mutex::new(HashSet::new()),
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
        self.auth
            .check_session(&self.client, &self.endpoints)
            .await
    }

    async fn refresh_auth(&self) -> Result<(), SyncError> {
        self.auth
            .login(&self.client, &self.endpoints)
            .await
    }

    async fn detect_sales(&self) -> Result<Vec<SaleDetection>, SyncError> {
        let records = self.client.fetch_recent_sales(&self.endpoints).await?;

        let mut sales = Vec::new();
        let mut seen = self.seen_orders.lock().unwrap();

        for record in records {
            // Session-level deduplication
            if seen.contains(&record.order_id) {
                continue;
            }
            seen.insert(record.order_id.clone());

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

    async fn fetch_full_listing(
        &self,
        platform_item_id: &str,
    ) -> Result<FullListing, SyncError> {
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
        let price = detail
            .price
            .or(listing.price)
            .map(|amount| ListingPrice {
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

    async fn fetch_photos(
        &self,
        platform_item_id: &str,
    ) -> Result<Vec<ListingPhoto>, SyncError> {
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

    async fn fetch_price(
        &self,
        platform_item_id: &str,
    ) -> Result<Option<ListingPrice>, SyncError> {
        // First try to get price from the edit page detail (more accurate)
        let detail = self
            .client
            .fetch_listing_detail(&self.endpoints, platform_item_id)
            .await?;

        let currency = detail.currency.clone().unwrap_or_else(|| "USD".to_string());
        if let Some(amount) = detail.price {
            return Ok(Some(ListingPrice {
                amount,
                currency,
            }));
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
        let price_str = price.amount.to_string();
        self.client
            .update_listing_field(&self.endpoints, platform_item_id, "price", &price_str)
            .await
    }

    async fn set_description(
        &self,
        platform_item_id: &str,
        html: &str,
    ) -> Result<(), SyncError> {
        // Try the most common field name first; update_listing_field will
        // fall back to adding the field if it doesn't already exist.
        self.client
            .update_listing_field(&self.endpoints, platform_item_id, "description", html)
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

    async fn create_listing(
        &self,
        request: CreateListingRequest,
    ) -> Result<String, SyncError> {
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
