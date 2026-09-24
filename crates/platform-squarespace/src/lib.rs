pub mod auth;
pub mod client;
pub mod mapping;
pub mod types;

use async_trait::async_trait;
use nisaba_core::config::SquarespaceConfig;
use nisaba_core::error::SyncError;
use nisaba_core::traits::PlatformAdapter;
use nisaba_core::types::{
    CreateListingRequest, FullListing, ListingDescription, ListingPhoto, ListingPrice, Platform,
    PlatformCapabilities, PlatformInventoryItem, PlatformListing, UpdateListingRequest,
};
use tracing::info;

use crate::auth::SquarespaceAuth;
use crate::client::SquarespaceClient;

pub struct SquarespaceAdapter {
    auth: SquarespaceAuth,
    client: SquarespaceClient,
    store_page_id: String,
}

impl SquarespaceAdapter {
    pub fn new(config: &SquarespaceConfig) -> Self {
        let auth = SquarespaceAuth::new(config.api_key.clone());
        let client = SquarespaceClient::new();
        Self {
            auth,
            client,
            store_page_id: config.store_page_id.clone(),
        }
    }

    /// Resolve variant_id → product_id by scanning all products.
    async fn find_product_id_for_variant(&self, variant_id: &str) -> Result<String, SyncError> {
        let products = self.client.fetch_products(self.auth.api_key()).await?;
        for product in &products {
            let has_variant = product
                .variants
                .as_ref()
                .map(|vs| vs.iter().any(|v| v.id == variant_id))
                .unwrap_or(false);
            if has_variant {
                return Ok(product.id.clone());
            }
        }
        Err(SyncError::ApiError {
            platform: Platform::Squarespace,
            message: format!(
                "Variant {} not found in any Squarespace product",
                variant_id
            ),
        })
    }
}

#[async_trait]
impl PlatformAdapter for SquarespaceAdapter {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn platform(&self) -> Platform {
        Platform::Squarespace
    }

    async fn fetch_inventory(&self) -> Result<Vec<PlatformInventoryItem>, SyncError> {
        self.auth.validate()?;
        let items = self.client.fetch_inventory(self.auth.api_key()).await?;
        Ok(items
            .iter()
            .filter_map(mapping::to_inventory_item)
            .collect())
    }

    async fn fetch_all_listings(&self) -> Result<Vec<PlatformListing>, SyncError> {
        self.auth.validate()?;
        let key = self.auth.api_key();
        let products = self.client.fetch_products(key).await?;
        let inventory = self.client.fetch_inventory(key).await?;
        Ok(mapping::to_listings(&products, &inventory))
    }

    async fn set_quantity(&self, platform_item_id: &str, qty: i64) -> Result<(), SyncError> {
        self.auth.validate()?;
        self.client
            .set_quantity(self.auth.api_key(), platform_item_id, qty)
            .await
    }

    async fn is_authenticated(&self) -> bool {
        self.auth.validate().is_ok()
    }

    async fn refresh_auth(&self) -> Result<(), SyncError> {
        // Squarespace uses static API keys — no refresh needed.
        self.auth.validate()
    }

    async fn fetch_full_listing(&self, platform_item_id: &str) -> Result<FullListing, SyncError> {
        self.auth.validate()?;
        let key = self.auth.api_key();
        let products = self.client.fetch_products(key).await?;
        let inventory = self.client.fetch_inventory(key).await?;

        // Search all products for the variant matching platform_item_id
        for product in &products {
            if let Some(listing) = mapping::to_full_listing(product, platform_item_id, &inventory) {
                return Ok(listing);
            }
        }

        Err(SyncError::ApiError {
            platform: Platform::Squarespace,
            message: format!(
                "Variant {} not found in any Squarespace product",
                platform_item_id
            ),
        })
    }

    async fn fetch_description(
        &self,
        platform_item_id: &str,
    ) -> Result<Option<ListingDescription>, SyncError> {
        self.auth.validate()?;
        let products = self.client.fetch_products(self.auth.api_key()).await?;

        // Find the product containing this variant
        for product in &products {
            let has_variant = product
                .variants
                .as_ref()
                .map(|vs| vs.iter().any(|v| v.id == platform_item_id))
                .unwrap_or(false);

            if has_variant {
                let description = product.description.as_ref().map(|desc| ListingDescription {
                    platform: Platform::Squarespace,
                    platform_item_id: platform_item_id.to_string(),
                    html: Some(desc.clone()),
                    plain_text: None,
                    fetched_at: chrono::Utc::now().to_rfc3339(),
                });
                return Ok(description);
            }
        }

        Err(SyncError::ApiError {
            platform: Platform::Squarespace,
            message: format!(
                "Variant {} not found in any Squarespace product",
                platform_item_id
            ),
        })
    }

    async fn fetch_photos(&self, platform_item_id: &str) -> Result<Vec<ListingPhoto>, SyncError> {
        self.auth.validate()?;
        let products = self.client.fetch_products(self.auth.api_key()).await?;

        // Find the product containing this variant
        for product in &products {
            let has_variant = product
                .variants
                .as_ref()
                .map(|vs| vs.iter().any(|v| v.id == platform_item_id))
                .unwrap_or(false);

            if has_variant {
                let photos = product
                    .images
                    .as_ref()
                    .map(|imgs| {
                        imgs.iter()
                            .enumerate()
                            .filter_map(|(i, img)| {
                                let url = img.url.as_ref()?;
                                Some(ListingPhoto {
                                    url: url.clone(),
                                    position: i as i32,
                                    width: None,
                                    height: None,
                                    alt_text: None,
                                })
                            })
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                return Ok(photos);
            }
        }

        Err(SyncError::ApiError {
            platform: Platform::Squarespace,
            message: format!(
                "Variant {} not found in any Squarespace product",
                platform_item_id
            ),
        })
    }

    async fn fetch_price(&self, platform_item_id: &str) -> Result<Option<ListingPrice>, SyncError> {
        self.auth.validate()?;
        let products = self.client.fetch_products(self.auth.api_key()).await?;

        // Find the variant matching platform_item_id across all products
        for product in &products {
            if let Some(variants) = &product.variants {
                if let Some(variant) = variants.iter().find(|v| v.id == platform_item_id) {
                    let price = variant
                        .pricing
                        .as_ref()
                        .and_then(|p| p.base_price.as_ref())
                        .and_then(|bp| {
                            let amount = bp.value.as_ref()?.parse::<f64>().ok()?;
                            let currency = bp.currency.clone().unwrap_or_else(|| "USD".to_string());
                            Some(ListingPrice { amount, currency })
                        });
                    return Ok(price);
                }
            }
        }

        Err(SyncError::ApiError {
            platform: Platform::Squarespace,
            message: format!(
                "Variant {} not found in any Squarespace product",
                platform_item_id
            ),
        })
    }

    async fn set_description(&self, platform_item_id: &str, html: &str) -> Result<(), SyncError> {
        self.auth.validate()?;
        let product_id = self.find_product_id_for_variant(platform_item_id).await?;
        let update_req = crate::types::SquarespaceUpdateProductRequest {
            name: None,
            description: Some(html.to_string()),
            tags: None,
            is_visible: None,
        };
        self.client
            .update_product(self.auth.api_key(), &product_id, &update_req)
            .await
    }

    async fn create_listing(&self, request: CreateListingRequest) -> Result<String, SyncError> {
        self.auth.validate()?;
        let key = self.auth.api_key();

        // Resolve store page ID
        let store_page_id = if self.store_page_id.is_empty() {
            let pages = self.client.fetch_store_pages(key).await?;
            let page = pages
                .iter()
                .find(|p| p.is_enabled)
                .or(pages.first())
                .ok_or_else(|| SyncError::ApiError {
                    platform: Platform::Squarespace,
                    message: "No store pages found. Create a store page in Squarespace first."
                        .into(),
                })?;
            info!(page_id = %page.id, page_title = %page.title, "Auto-selected store page");
            page.id.clone()
        } else {
            self.store_page_id.clone()
        };

        // Build pricing
        let (price_value, price_currency) = match &request.price {
            Some(p) => (p.amount.to_string(), p.currency.clone()),
            None => ("0".to_string(), "USD".to_string()),
        };

        let create_req = crate::types::SquarespaceCreateProductRequest {
            product_type: "PHYSICAL".to_string(),
            store_page_id,
            name: Some(request.title.clone()),
            description: request.description_html.clone(),
            tags: None,
            is_visible: Some(true),
            variants: vec![crate::types::SquarespaceCreateVariant {
                sku: request.sku.clone(),
                pricing: crate::types::SquarespaceCreatePricing {
                    base_price: crate::types::SquarespaceCreatePrice {
                        currency: price_currency,
                        value: price_value,
                    },
                },
                stock: Some(crate::types::SquarespaceStock {
                    quantity: Some(request.quantity),
                    unlimited: None,
                }),
            }],
        };

        let response = self.client.create_product(key, &create_req).await?;

        // Return the variant ID as platform_item_id (consistent with how we map Squarespace items)
        let variant_id = response
            .variants
            .as_ref()
            .and_then(|vs| vs.first())
            .map(|v| v.id.clone())
            .unwrap_or(response.id.clone());

        info!(
            product_id = %response.id,
            variant_id = %variant_id,
            title = %request.title,
            "Created Squarespace listing"
        );

        Ok(variant_id)
    }

    async fn update_listing(
        &self,
        platform_item_id: &str,
        request: UpdateListingRequest,
    ) -> Result<(), SyncError> {
        self.auth.validate()?;
        let product_id = self.find_product_id_for_variant(platform_item_id).await?;

        let update_req = crate::types::SquarespaceUpdateProductRequest {
            name: request.title,
            description: request.description_html,
            tags: request.tags,
            is_visible: None,
        };

        self.client
            .update_product(self.auth.api_key(), &product_id, &update_req)
            .await
    }

    fn capabilities(&self) -> PlatformCapabilities {
        PlatformCapabilities {
            can_fetch_full_listing: true,
            can_fetch_description: true,
            can_fetch_photos: true,
            can_fetch_price: true,
            can_set_price: false,
            can_set_description: true,
            can_upload_photos: false,
            can_create_listing: true,
            has_stock_mode_inventory: false,
        }
    }
}
