use nisaba_core::error::SyncError;
use nisaba_core::types::Platform;
use reqwest::Client;
use serde_json::json;
use tracing::{debug, info, warn};

use crate::types::*;

/// Percent-encode a string for use in URL path segments.
fn url_encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                String::from(b as char)
            }
            _ => format!("%{:02X}", b),
        })
        .collect()
}

const SANDBOX_API_BASE: &str = "https://api.sandbox.ebay.com";
const PRODUCTION_API_BASE: &str = "https://api.ebay.com";

pub struct EbayClient {
    http: Client,
    api_base: String,
}

impl EbayClient {
    pub fn new(is_sandbox: bool) -> Self {
        let http = Client::builder()
            .user_agent("Nisaba/0.1.0")
            .build()
            .expect("Failed to build HTTP client");

        let api_base = if is_sandbox {
            SANDBOX_API_BASE
        } else {
            PRODUCTION_API_BASE
        }
        .to_string();

        Self { http, api_base }
    }

    pub fn http(&self) -> &Client {
        &self.http
    }

    /// Fetch all inventory items from the eBay Inventory API.
    pub async fn fetch_inventory_items(
        &self,
        access_token: &str,
    ) -> Result<Vec<EbayInventoryItem>, SyncError> {
        let mut all_items = Vec::new();
        let mut offset = 0;
        let limit = 100;

        loop {
            let url = format!(
                "{}/sell/inventory/v1/inventory_item?limit={}&offset={}",
                self.api_base, limit, offset
            );

            let resp = self
                .http
                .get(&url)
                .bearer_auth(access_token)
                .header("Content-Language", "en-US")
                .send()
                .await
                .map_err(|e| SyncError::NetworkError {
                    platform: Platform::Ebay,
                    message: e.to_string(),
                })?;

            let status = resp.status();
            if status.as_u16() == 401 {
                return Err(SyncError::AuthError {
                    platform: Platform::Ebay,
                    message: "Access token expired".into(),
                });
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(SyncError::ApiError {
                    platform: Platform::Ebay,
                    message: format!("Inventory fetch failed ({status}): {body}"),
                });
            }

            let response: EbayInventoryItemsResponse =
                resp.json().await.map_err(|e| SyncError::ApiError {
                    platform: Platform::Ebay,
                    message: format!("Failed to parse inventory response: {e}"),
                })?;

            let items = response.inventory_items.unwrap_or_default();
            let count = items.len();
            all_items.extend(items);

            let total = response.total.unwrap_or(0) as usize;
            offset += count;

            if offset >= total || count == 0 {
                break;
            }
        }

        debug!(count = all_items.len(), "Fetched eBay inventory items");
        Ok(all_items)
    }

    /// Update quantities for up to 25 SKUs in a single bulk request.
    pub async fn bulk_update_quantity(
        &self,
        access_token: &str,
        updates: Vec<(String, i64)>, // (sku, quantity)
    ) -> Result<(), SyncError> {
        if updates.is_empty() {
            return Ok(());
        }

        // eBay allows max 25 per request
        for chunk in updates.chunks(25) {
            let request = BulkUpdateRequest {
                requests: chunk
                    .iter()
                    .map(|(sku, qty)| BulkUpdateEntry {
                        sku: sku.clone(),
                        ship_to_location_availability: ShipToLocationUpdate { quantity: *qty },
                    })
                    .collect(),
            };

            let url = format!(
                "{}/sell/inventory/v1/bulk_update_price_quantity",
                self.api_base
            );

            let resp = self
                .http
                .post(&url)
                .bearer_auth(access_token)
                .header("Content-Type", "application/json")
                .header("Content-Language", "en-US")
                .json(&request)
                .send()
                .await
                .map_err(|e| SyncError::NetworkError {
                    platform: Platform::Ebay,
                    message: e.to_string(),
                })?;

            let status = resp.status();
            if status.as_u16() == 401 {
                return Err(SyncError::AuthError {
                    platform: Platform::Ebay,
                    message: "Access token expired".into(),
                });
            }

            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(SyncError::ApiError {
                    platform: Platform::Ebay,
                    message: format!("Bulk update failed ({status}): {body}"),
                });
            }

            let response: BulkUpdateResponse =
                resp.json().await.map_err(|e| SyncError::ApiError {
                    platform: Platform::Ebay,
                    message: format!("Failed to parse bulk update response: {e}"),
                })?;

            // Check for per-item errors
            if let Some(responses) = response.responses {
                for entry in responses {
                    if let Some(errors) = entry.errors {
                        for err in errors {
                            warn!(
                                sku = entry.sku.as_deref().unwrap_or("?"),
                                error_id = err.error_id,
                                message = err.message.as_deref().unwrap_or(""),
                                "eBay bulk update error for item"
                            );
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Fetch a single inventory item by SKU.
    pub async fn fetch_inventory_item(
        &self,
        access_token: &str,
        sku: &str,
    ) -> Result<EbayInventoryItem, SyncError> {
        let url = format!(
            "{}/sell/inventory/v1/inventory_item/{}",
            self.api_base,
            url_encode(sku)
        );

        let resp = self
            .http
            .get(&url)
            .bearer_auth(access_token)
            .header("Content-Language", "en-US")
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Ebay,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(SyncError::AuthError {
                platform: Platform::Ebay,
                message: "Access token expired".into(),
            });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Fetch inventory item failed ({status}): {body}"),
            });
        }

        let item: EbayInventoryItem =
            resp.json().await.map_err(|e| SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Failed to parse inventory item response: {e}"),
            })?;

        debug!(sku = sku, "Fetched eBay inventory item");
        Ok(item)
    }

    /// Fetch offers for a given SKU from the eBay Offer API.
    pub async fn fetch_offers(
        &self,
        access_token: &str,
        sku: &str,
    ) -> Result<Vec<EbayOffer>, SyncError> {
        let mut all_offers = Vec::new();
        let mut offset = 0;
        let limit = 100;

        loop {
            let url = format!(
                "{}/sell/inventory/v1/offer?sku={}&limit={}&offset={}",
                self.api_base,
                url_encode(sku),
                limit,
                offset
            );

            let resp = self
                .http
                .get(&url)
                .bearer_auth(access_token)
                .header("Content-Language", "en-US")
                .send()
                .await
                .map_err(|e| SyncError::NetworkError {
                    platform: Platform::Ebay,
                    message: e.to_string(),
                })?;

            let status = resp.status();
            if status.as_u16() == 401 {
                return Err(SyncError::AuthError {
                    platform: Platform::Ebay,
                    message: "Access token expired".into(),
                });
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(SyncError::ApiError {
                    platform: Platform::Ebay,
                    message: format!("Fetch offers failed ({status}): {body}"),
                });
            }

            let response: EbayOffersResponse =
                resp.json().await.map_err(|e| SyncError::ApiError {
                    platform: Platform::Ebay,
                    message: format!("Failed to parse offers response: {e}"),
                })?;

            let offers = response.offers.unwrap_or_default();
            let count = offers.len();
            all_offers.extend(offers);

            let total = response.total.unwrap_or(0) as usize;
            offset += count;

            if offset >= total || count == 0 {
                break;
            }
        }

        debug!(sku = sku, count = all_offers.len(), "Fetched eBay offers");
        Ok(all_offers)
    }

    /// Fetch all offers across all SKUs (for bulk price lookup).
    pub async fn fetch_all_offers(
        &self,
        access_token: &str,
    ) -> Result<Vec<EbayOffer>, SyncError> {
        let mut all_offers = Vec::new();
        let mut offset = 0;
        let limit = 100;

        loop {
            let url = format!(
                "{}/sell/inventory/v1/offer?limit={}&offset={}",
                self.api_base, limit, offset
            );

            let resp = self
                .http
                .get(&url)
                .bearer_auth(access_token)
                .header("Content-Language", "en-US")
                .send()
                .await
                .map_err(|e| SyncError::NetworkError {
                    platform: Platform::Ebay,
                    message: e.to_string(),
                })?;

            let status = resp.status();
            if status.as_u16() == 401 {
                return Err(SyncError::AuthError {
                    platform: Platform::Ebay,
                    message: "Access token expired".into(),
                });
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(SyncError::ApiError {
                    platform: Platform::Ebay,
                    message: format!("Fetch all offers failed ({status}): {body}"),
                });
            }

            let response: EbayOffersResponse =
                resp.json().await.map_err(|e| SyncError::ApiError {
                    platform: Platform::Ebay,
                    message: format!("Failed to parse offers response: {e}"),
                })?;

            let offers = response.offers.unwrap_or_default();
            let count = offers.len();
            all_offers.extend(offers);

            let total = response.total.unwrap_or(0) as usize;
            offset += count;

            if offset >= total || count == 0 {
                break;
            }
        }

        debug!(count = all_offers.len(), "Fetched all eBay offers");
        Ok(all_offers)
    }

    /// Update an inventory item's description via PUT (full replace).
    /// The eBay Inventory API requires a full PUT, so we fetch the item first,
    /// merge the new description, and PUT back.
    pub async fn update_inventory_item(
        &self,
        access_token: &str,
        sku: &str,
        description: &str,
    ) -> Result<(), SyncError> {
        // Fetch existing item to preserve all other fields
        let existing = self.fetch_inventory_item(access_token, sku).await?;

        let product = existing.product.unwrap_or(EbayProductInfo {
            title: None,
            description: None,
            image_urls: None,
            aspects: None,
        });

        let mut product_json = json!({
            "title": product.title.unwrap_or_default(),
            "description": description,
            "imageUrls": product.image_urls.unwrap_or_default(),
        });

        // Preserve existing aspects
        if let Some(aspects) = &product.aspects {
            product_json["aspects"] = json!(aspects);
        }

        let body = json!({
            "product": product_json,
            "availability": {
                "shipToLocationAvailability": {
                    "quantity": existing.availability
                        .as_ref()
                        .and_then(|a| a.ship_to_location_availability.as_ref())
                        .and_then(|s| s.quantity)
                        .unwrap_or(0)
                }
            },
            "condition": existing.condition.unwrap_or_else(|| "NEW".to_string()),
        });

        let url = format!(
            "{}/sell/inventory/v1/inventory_item/{}",
            self.api_base,
            url_encode(sku)
        );

        let resp = self
            .http
            .put(&url)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .header("Content-Language", "en-US")
            .json(&body)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Ebay,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(SyncError::AuthError {
                platform: Platform::Ebay,
                message: "Access token expired".into(),
            });
        }
        // eBay returns 204 No Content on successful PUT
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Update inventory item failed ({status}): {body}"),
            });
        }

        debug!(sku = sku, "Updated eBay inventory item description");
        Ok(())
    }

    /// Create a new inventory item via PUT /sell/inventory/v1/inventory_item/{sku}.
    /// Note: eBay uses PUT (not POST) for creating/replacing inventory items.
    pub async fn create_inventory_item(
        &self,
        access_token: &str,
        sku: &str,
        product_info: &EbayCreateItemRequest,
    ) -> Result<(), SyncError> {
        let url = format!(
            "{}/sell/inventory/v1/inventory_item/{}",
            self.api_base,
            url_encode(sku)
        );

        let resp = self
            .http
            .put(&url)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .header("Content-Language", "en-US")
            .json(product_info)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Ebay,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(SyncError::AuthError {
                platform: Platform::Ebay,
                message: "Access token expired".into(),
            });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Create inventory item failed ({status}): {body}"),
            });
        }

        debug!(sku = sku, "Created eBay inventory item");
        Ok(())
    }

    /// Create an offer for an existing inventory item. Returns the offer ID.
    pub async fn create_offer(
        &self,
        access_token: &str,
        offer: &EbayCreateOfferRequest,
    ) -> Result<String, SyncError> {
        let url = format!("{}/sell/inventory/v1/offer", self.api_base);

        let resp = self
            .http
            .post(&url)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .header("Content-Language", "en-US")
            .json(offer)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Ebay,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(SyncError::AuthError {
                platform: Platform::Ebay,
                message: "Access token expired".into(),
            });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Create offer failed ({status}): {body}"),
            });
        }

        let response: EbayCreateOfferResponse =
            resp.json().await.map_err(|e| SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Failed to parse create offer response: {e}"),
            })?;

        let offer_id = response.offer_id.ok_or_else(|| SyncError::ApiError {
            platform: Platform::Ebay,
            message: "Create offer succeeded but no offerId was returned".into(),
        })?;

        debug!(sku = %offer.sku, offer_id = %offer_id, "Created eBay offer");
        Ok(offer_id)
    }

    /// Publish a draft offer, making it live on eBay. Returns the listing ID.
    pub async fn publish_offer(
        &self,
        access_token: &str,
        offer_id: &str,
    ) -> Result<String, SyncError> {
        let url = format!(
            "{}/sell/inventory/v1/offer/{}/publish",
            self.api_base, offer_id
        );

        let resp = self
            .http
            .post(&url)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .header("Content-Language", "en-US")
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Ebay,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(SyncError::AuthError {
                platform: Platform::Ebay,
                message: "Access token expired".into(),
            });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Publish offer failed ({status}): {body}"),
            });
        }

        let response: EbayPublishOfferResponse =
            resp.json().await.map_err(|e| SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Failed to parse publish offer response: {e}"),
            })?;

        let listing_id = response.listing_id.ok_or_else(|| SyncError::ApiError {
            platform: Platform::Ebay,
            message: "Publish offer succeeded but no listingId was returned".into(),
        })?;

        debug!(offer_id = offer_id, listing_id = %listing_id, "Published eBay offer");
        Ok(listing_id)
    }

    /// Update price and quantity using the bulk update endpoint for a single SKU.
    /// Re-uses the existing bulk endpoint since eBay doesn't have a single-item
    /// price update in the Inventory API -- price is set via offer or bulk.
    pub async fn bulk_update_price_quantity(
        &self,
        access_token: &str,
        sku: &str,
        price: f64,
        currency: &str,
        quantity: i64,
    ) -> Result<(), SyncError> {
        let url = format!(
            "{}/sell/inventory/v1/bulk_update_price_quantity",
            self.api_base
        );

        let body = json!({
            "requests": [{
                "sku": sku,
                "shipToLocationAvailability": {
                    "quantity": quantity,
                },
                "offers": [{
                    "price": {
                        "value": format!("{:.2}", price),
                        "currency": currency,
                    }
                }]
            }]
        });

        let resp = self
            .http
            .post(&url)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .header("Content-Language", "en-US")
            .json(&body)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Ebay,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(SyncError::AuthError {
                platform: Platform::Ebay,
                message: "Access token expired".into(),
            });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Bulk update price/quantity failed ({status}): {body}"),
            });
        }

        debug!(sku = sku, price = price, "Updated eBay price via bulk endpoint");
        Ok(())
    }

    /// Update an existing offer by offer ID (full PUT).
    pub async fn update_offer(
        &self,
        access_token: &str,
        offer_id: &str,
        body: &serde_json::Value,
    ) -> Result<(), SyncError> {
        let url = format!(
            "{}/sell/inventory/v1/offer/{}",
            self.api_base, offer_id
        );

        let resp = self
            .http
            .put(&url)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .header("Content-Language", "en-US")
            .json(body)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Ebay,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(SyncError::AuthError {
                platform: Platform::Ebay,
                message: "Access token expired".into(),
            });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Update offer failed ({status}): {body}"),
            });
        }

        debug!(offer_id = offer_id, "Updated eBay offer");
        Ok(())
    }

    /// Update an inventory item via fetch-merge-PUT, supporting optional
    /// description, aspects, and condition overrides.
    pub async fn update_inventory_item_full(
        &self,
        access_token: &str,
        sku: &str,
        description: Option<&str>,
        aspects: Option<&std::collections::HashMap<String, Vec<String>>>,
        condition: Option<&str>,
        condition_description: Option<&str>,
    ) -> Result<(), SyncError> {
        let existing = self.fetch_inventory_item(access_token, sku).await?;

        let product = existing.product.unwrap_or(EbayProductInfo {
            title: None,
            description: None,
            image_urls: None,
            aspects: None,
        });

        let mut product_json = json!({
            "title": product.title.unwrap_or_default(),
            "description": description.unwrap_or_else(|| product.description.as_deref().unwrap_or_default()),
            "imageUrls": product.image_urls.unwrap_or_default(),
        });

        // Use provided aspects, or preserve existing
        if let Some(new_aspects) = aspects {
            product_json["aspects"] = json!(new_aspects);
        } else if let Some(existing_aspects) = &product.aspects {
            product_json["aspects"] = json!(existing_aspects);
        }

        let mut body = json!({
            "product": product_json,
            "availability": {
                "shipToLocationAvailability": {
                    "quantity": existing.availability
                        .as_ref()
                        .and_then(|a| a.ship_to_location_availability.as_ref())
                        .and_then(|s| s.quantity)
                        .unwrap_or(0)
                }
            },
            "condition": condition.unwrap_or_else(|| existing.condition.as_deref().unwrap_or("NEW")),
        });

        if let Some(cd) = condition_description {
            body["conditionDescription"] = json!(cd);
        }

        let url = format!(
            "{}/sell/inventory/v1/inventory_item/{}",
            self.api_base,
            url_encode(sku)
        );

        let resp = self
            .http
            .put(&url)
            .bearer_auth(access_token)
            .header("Content-Type", "application/json")
            .header("Content-Language", "en-US")
            .json(&body)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Ebay,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.as_u16() == 401 {
            return Err(SyncError::AuthError {
                platform: Platform::Ebay,
                message: "Access token expired".into(),
            });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Update inventory item failed ({status}): {body}"),
            });
        }

        debug!(sku = sku, "Updated eBay inventory item (full)");
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Account API — seller business policies
    // -----------------------------------------------------------------------

    /// Fetch all fulfillment policies for the seller.
    pub async fn get_fulfillment_policies(
        &self,
        access_token: &str,
        marketplace_id: &str,
    ) -> Result<Vec<EbayBusinessPolicy>, SyncError> {
        let url = format!(
            "{}/sell/account/v1/fulfillment_policy?marketplace_id={}",
            self.api_base, marketplace_id
        );
        let resp = self.http.get(&url).bearer_auth(access_token).send().await
            .map_err(|e| SyncError::NetworkError { platform: Platform::Ebay, message: e.to_string() })?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError { platform: Platform::Ebay, message: format!("Get fulfillment policies failed: {body}") });
        }

        let data: EbayFulfillmentPoliciesResponse = resp.json().await
            .map_err(|e| SyncError::ApiError { platform: Platform::Ebay, message: format!("Parse fulfillment policies failed: {e}") })?;

        Ok(data.policies.into_iter().filter_map(|p| {
            Some(EbayBusinessPolicy { id: p.id?, name: p.name.unwrap_or_default() })
        }).collect())
    }

    /// Fetch all payment policies for the seller.
    pub async fn get_payment_policies(
        &self,
        access_token: &str,
        marketplace_id: &str,
    ) -> Result<Vec<EbayBusinessPolicy>, SyncError> {
        let url = format!(
            "{}/sell/account/v1/payment_policy?marketplace_id={}",
            self.api_base, marketplace_id
        );
        let resp = self.http.get(&url).bearer_auth(access_token).send().await
            .map_err(|e| SyncError::NetworkError { platform: Platform::Ebay, message: e.to_string() })?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError { platform: Platform::Ebay, message: format!("Get payment policies failed: {body}") });
        }

        let data: EbayPaymentPoliciesResponse = resp.json().await
            .map_err(|e| SyncError::ApiError { platform: Platform::Ebay, message: format!("Parse payment policies failed: {e}") })?;

        Ok(data.policies.into_iter().filter_map(|p| {
            Some(EbayBusinessPolicy { id: p.id?, name: p.name.unwrap_or_default() })
        }).collect())
    }

    /// Fetch all return policies for the seller.
    pub async fn get_return_policies(
        &self,
        access_token: &str,
        marketplace_id: &str,
    ) -> Result<Vec<EbayBusinessPolicy>, SyncError> {
        let url = format!(
            "{}/sell/account/v1/return_policy?marketplace_id={}",
            self.api_base, marketplace_id
        );
        let resp = self.http.get(&url).bearer_auth(access_token).send().await
            .map_err(|e| SyncError::NetworkError { platform: Platform::Ebay, message: e.to_string() })?;

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError { platform: Platform::Ebay, message: format!("Get return policies failed: {body}") });
        }

        let data: EbayReturnPoliciesResponse = resp.json().await
            .map_err(|e| SyncError::ApiError { platform: Platform::Ebay, message: format!("Parse return policies failed: {e}") })?;

        Ok(data.policies.into_iter().filter_map(|p| {
            Some(EbayBusinessPolicy { id: p.id?, name: p.name.unwrap_or_default() })
        }).collect())
    }

    // -----------------------------------------------------------------------
    // Commerce Taxonomy API
    // -----------------------------------------------------------------------

    /// Get the default category tree ID for a marketplace.
    pub async fn get_default_category_tree_id(
        &self,
        access_token: &str,
        marketplace_id: &str,
    ) -> Result<String, SyncError> {
        let url = format!(
            "{}/commerce/taxonomy/v1/get_default_category_tree_id?marketplace_id={}",
            self.api_base, marketplace_id
        );

        let resp = self
            .http
            .get(&url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Ebay,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Get category tree ID failed ({status}): {body}"),
            });
        }

        let response: EbayCategoryTreeIdResponse =
            resp.json().await.map_err(|e| SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Failed to parse category tree ID response: {e}"),
            })?;

        response.category_tree_id.ok_or_else(|| SyncError::ApiError {
            platform: Platform::Ebay,
            message: "No categoryTreeId returned".into(),
        })
    }

    /// Get category suggestions for a query string.
    pub async fn get_category_suggestions(
        &self,
        access_token: &str,
        category_tree_id: &str,
        query: &str,
    ) -> Result<Vec<EbayCategorySuggestion>, SyncError> {
        let url = format!(
            "{}/commerce/taxonomy/v1/category_tree/{}/get_category_suggestions?q={}",
            self.api_base, category_tree_id, url_encode(query)
        );

        let resp = self
            .http
            .get(&url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Ebay,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Get category suggestions failed ({status}): {body}"),
            });
        }

        let response: EbayCategorySuggestionsResponse =
            resp.json().await.map_err(|e| SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Failed to parse category suggestions response: {e}"),
            })?;

        Ok(response.category_suggestions.unwrap_or_default())
    }

    /// Get item aspects (specifics) for a given category.
    pub async fn get_item_aspects_for_category(
        &self,
        access_token: &str,
        category_tree_id: &str,
        category_id: &str,
    ) -> Result<Vec<EbayAspectMetadata>, SyncError> {
        let url = format!(
            "{}/commerce/taxonomy/v1/category_tree/{}/get_item_aspects_for_category?category_id={}",
            self.api_base, category_tree_id, category_id
        );

        let resp = self
            .http
            .get(&url)
            .bearer_auth(access_token)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Ebay,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Get item aspects failed ({status}): {body}"),
            });
        }

        let response: EbayAspectsResponse =
            resp.json().await.map_err(|e| SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Failed to parse aspects response: {e}"),
            })?;

        Ok(response.aspects.unwrap_or_default())
    }

    // -----------------------------------------------------------------------
    // Trading API (XML) — for fetching legacy listings
    // -----------------------------------------------------------------------

    fn trading_api_url(&self) -> String {
        format!("{}/ws/api.dll", self.api_base)
    }

    /// Fetch active listings via the Trading API's GetMyeBaySelling call.
    /// This returns listings created through the eBay website or older APIs
    /// that don't appear in the Inventory API.
    pub async fn fetch_active_listings_trading(
        &self,
        access_token: &str,
    ) -> Result<Vec<TradingItem>, SyncError> {
        let mut all_items = Vec::new();
        let mut page = 1;

        loop {
            let xml_body = format!(
                r#"<?xml version="1.0" encoding="utf-8"?>
<GetMyeBaySellingRequest xmlns="urn:ebay:apis:eBLBaseComponents">
  <ActiveList>
    <Sort>TimeLeft</Sort>
    <Pagination>
      <EntriesPerPage>200</EntriesPerPage>
      <PageNumber>{page}</PageNumber>
    </Pagination>
  </ActiveList>
</GetMyeBaySellingRequest>"#
            );

            let resp = self
                .http
                .post(self.trading_api_url())
                .header("X-EBAY-API-COMPATIBILITY-LEVEL", "967")
                .header("X-EBAY-API-SITEID", "0")
                .header("X-EBAY-API-CALL-NAME", "GetMyeBaySelling")
                .header("X-EBAY-API-IAF-TOKEN", access_token)
                .header("Content-Type", "text/xml")
                .body(xml_body)
                .send()
                .await
                .map_err(|e| SyncError::NetworkError {
                    platform: Platform::Ebay,
                    message: e.to_string(),
                })?;

            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(SyncError::ApiError {
                    platform: Platform::Ebay,
                    message: format!("GetMyeBaySelling failed ({status}): {body}"),
                });
            }

            let body = resp.text().await.map_err(|e| SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Failed to read Trading API response: {e}"),
            })?;

            let parsed: TradingGetMyeBaySellingResponse =
                quick_xml::de::from_str(&body).map_err(|e| SyncError::ApiError {
                    platform: Platform::Ebay,
                    message: format!("Failed to parse GetMyeBaySelling XML: {e}"),
                })?;

            if let Some(ack) = &parsed.ack {
                if ack == "Failure" {
                    let msg = parsed
                        .errors
                        .as_ref()
                        .and_then(|errs| errs.first())
                        .and_then(|e| e.long_message.clone())
                        .unwrap_or_else(|| "Unknown error".into());
                    return Err(SyncError::ApiError {
                        platform: Platform::Ebay,
                        message: format!("GetMyeBaySelling error: {msg}"),
                    });
                }
            }

            let active_list = match parsed.active_list {
                Some(al) => al,
                None => break,
            };

            let items = active_list
                .item_array
                .map(|arr| arr.items)
                .unwrap_or_default();
            all_items.extend(items);

            let total_pages = active_list
                .pagination_result
                .and_then(|p| p.total_pages)
                .unwrap_or(1);

            if page >= total_pages {
                break;
            }
            page += 1;
        }

        info!(count = all_items.len(), "Fetched eBay active listings via Trading API");
        Ok(all_items)
    }

    /// Fetch a single item by ItemID via the Trading API's GetItem call.
    /// Used for legacy listings that aren't in the Inventory API.
    pub async fn fetch_item_trading(
        &self,
        access_token: &str,
        item_id: &str,
    ) -> Result<TradingItem, SyncError> {
        let xml_body = format!(
            r#"<?xml version="1.0" encoding="utf-8"?>
<GetItemRequest xmlns="urn:ebay:apis:eBLBaseComponents">
  <ItemID>{item_id}</ItemID>
  <DetailLevel>ReturnAll</DetailLevel>
</GetItemRequest>"#
        );

        let resp = self
            .http
            .post(self.trading_api_url())
            .header("X-EBAY-API-COMPATIBILITY-LEVEL", "967")
            .header("X-EBAY-API-SITEID", "0")
            .header("X-EBAY-API-CALL-NAME", "GetItem")
            .header("X-EBAY-API-IAF-TOKEN", access_token)
            .header("Content-Type", "text/xml")
            .body(xml_body)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Ebay,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("GetItem failed ({status}): {body}"),
            });
        }

        let body = resp.text().await.map_err(|e| SyncError::ApiError {
            platform: Platform::Ebay,
            message: format!("Failed to read GetItem response: {e}"),
        })?;

        let parsed: TradingGetItemResponse =
            quick_xml::de::from_str(&body).map_err(|e| SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Failed to parse GetItem XML: {e}"),
            })?;

        if let Some(ack) = &parsed.ack {
            if ack == "Failure" {
                let msg = parsed
                    .errors
                    .as_ref()
                    .and_then(|errs| errs.first())
                    .and_then(|e| e.long_message.clone())
                    .unwrap_or_else(|| "Unknown error".into());
                return Err(SyncError::ApiError {
                    platform: Platform::Ebay,
                    message: format!("GetItem error: {msg}"),
                });
            }
        }

        parsed.item.ok_or_else(|| SyncError::ApiError {
            platform: Platform::Ebay,
            message: format!("GetItem returned no item for {item_id}"),
        })
    }
}
