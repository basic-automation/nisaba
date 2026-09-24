use nisaba_core::error::SyncError;
use nisaba_core::types::Platform;
use reqwest::Client;
use serde_json::json;
use tracing::{debug, warn};

use crate::types::*;

const NA_BASE_URL: &str = "https://sellingpartnerapi-na.amazon.com";
const EU_BASE_URL: &str = "https://sellingpartnerapi-eu.amazon.com";
const FE_BASE_URL: &str = "https://sellingpartnerapi-fe.amazon.com";

const LISTINGS_API_VERSION: &str = "2021-08-01";

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

pub struct AmazonClient {
    http: Client,
    api_base: String,
}

impl AmazonClient {
    pub fn new(region: &str) -> Self {
        let http = Client::builder()
            .user_agent("Nisaba/0.1.0")
            .build()
            .expect("Failed to build HTTP client");

        let api_base = match region.to_uppercase().as_str() {
            "EU" => EU_BASE_URL,
            "FE" => FE_BASE_URL,
            _ => NA_BASE_URL, // Default to NA
        }
        .to_string();

        Self { http, api_base }
    }

    pub fn http(&self) -> &Client {
        &self.http
    }

    /// Build the marketplace_ids query parameter string.
    fn marketplace_query(marketplace_ids: &[String]) -> String {
        marketplace_ids
            .iter()
            .map(|id| format!("marketplaceIds={}", url_encode(id)))
            .collect::<Vec<_>>()
            .join("&")
    }

    /// Fetch all listings for a seller across the given marketplaces.
    pub async fn fetch_listings(
        &self,
        access_token: &str,
        seller_id: &str,
        marketplace_ids: &[String],
    ) -> Result<Vec<AmazonListingItem>, SyncError> {
        let mut all_items = Vec::new();
        let mut seen_skus = std::collections::HashSet::new();

        for marketplace_id in marketplace_ids {
            let mut page_token: Option<String> = None;

            loop {
                let mut url = format!(
                    "{}/listings/{}/items/{}?marketplaceIds={}&includedData=summaries,fulfillmentAvailability,offers&pageSize=20",
                    self.api_base,
                    LISTINGS_API_VERSION,
                    url_encode(seller_id),
                    url_encode(marketplace_id),
                );

                if let Some(ref token) = page_token {
                    url.push_str(&format!("&pageToken={}", url_encode(token)));
                }

                let resp = self
                    .http
                    .get(&url)
                    .header("x-amz-access-token", access_token)
                    .send()
                    .await
                    .map_err(|e| SyncError::NetworkError {
                        platform: Platform::Amazon,
                        message: e.to_string(),
                    })?;

                let status = resp.status();
                if status.as_u16() == 401 || status.as_u16() == 403 {
                    return Err(SyncError::AuthError {
                        platform: Platform::Amazon,
                        message: format!("Access denied ({status})"),
                    });
                }
                if !status.is_success() {
                    let body = resp.text().await.unwrap_or_default();
                    return Err(SyncError::ApiError {
                        platform: Platform::Amazon,
                        message: format!("Listings fetch failed ({status}): {body}"),
                    });
                }

                let response: AmazonListingsResponse =
                    resp.json().await.map_err(|e| SyncError::ApiError {
                        platform: Platform::Amazon,
                        message: format!("Failed to parse listings response: {e}"),
                    })?;

                for item in response.items {
                    if seen_skus.insert(item.sku.clone()) {
                        all_items.push(item);
                    }
                }

                match response.next_token {
                    Some(token) if !token.is_empty() => page_token = Some(token),
                    _ => break,
                }
            }
        }

        debug!(count = all_items.len(), "Fetched Amazon listings");
        Ok(all_items)
    }

    /// Fetch a single listing item by SKU.
    pub async fn fetch_listing(
        &self,
        access_token: &str,
        seller_id: &str,
        sku: &str,
        marketplace_ids: &[String],
    ) -> Result<AmazonListingItem, SyncError> {
        let mkt_query = Self::marketplace_query(marketplace_ids);
        let url = format!(
            "{}/listings/{}/items/{}/{}?{}&includedData=summaries,fulfillmentAvailability,offers,issues",
            self.api_base,
            LISTINGS_API_VERSION,
            url_encode(seller_id),
            url_encode(sku),
            mkt_query,
        );

        let resp = self
            .http
            .get(&url)
            .header("x-amz-access-token", access_token)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Amazon,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(SyncError::AuthError {
                platform: Platform::Amazon,
                message: format!("Access denied ({status})"),
            });
        }
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Amazon,
                message: format!("Fetch listing failed ({status}): {body}"),
            });
        }

        let item: AmazonListingItem = resp.json().await.map_err(|e| SyncError::ApiError {
            platform: Platform::Amazon,
            message: format!("Failed to parse listing response: {e}"),
        })?;

        debug!(sku = sku, "Fetched Amazon listing");
        Ok(item)
    }

    /// Update the quantity for a listing via PATCH on fulfillment_availability.
    pub async fn update_quantity(
        &self,
        access_token: &str,
        seller_id: &str,
        sku: &str,
        marketplace_ids: &[String],
        qty: i64,
    ) -> Result<(), SyncError> {
        for marketplace_id in marketplace_ids {
            let url = format!(
                "{}/listings/{}/items/{}/{}?marketplaceIds={}",
                self.api_base,
                LISTINGS_API_VERSION,
                url_encode(seller_id),
                url_encode(sku),
                url_encode(marketplace_id),
            );

            let body = AmazonPatchRequest {
                product_type: "PRODUCT".to_string(),
                patches: vec![AmazonPatch {
                    op: "replace".to_string(),
                    path: "/attributes/fulfillment_availability".to_string(),
                    value: vec![json!([{
                        "fulfillment_channel_code": "DEFAULT",
                        "quantity": qty,
                        "marketplace_id": marketplace_id,
                    }])],
                }],
            };

            let resp = self
                .http
                .patch(&url)
                .header("x-amz-access-token", access_token)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| SyncError::NetworkError {
                    platform: Platform::Amazon,
                    message: e.to_string(),
                })?;

            let status = resp.status();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(SyncError::AuthError {
                    platform: Platform::Amazon,
                    message: format!("Access denied ({status})"),
                });
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(SyncError::ApiError {
                    platform: Platform::Amazon,
                    message: format!(
                        "Update quantity failed for {sku} on {marketplace_id} ({status}): {body}"
                    ),
                });
            }

            let submission: AmazonListingSubmissionResponse =
                resp.json().await.map_err(|e| SyncError::ApiError {
                    platform: Platform::Amazon,
                    message: format!("Failed to parse patch response: {e}"),
                })?;

            if let Some(issues) = &submission.issues {
                for issue in issues {
                    warn!(
                        sku = sku,
                        code = issue.code.as_deref().unwrap_or("?"),
                        message = issue.message.as_deref().unwrap_or(""),
                        "Amazon listing issue after quantity update"
                    );
                }
            }
        }

        debug!(sku = sku, qty = qty, "Updated Amazon quantity");
        Ok(())
    }

    /// Update the price for a listing via PATCH on purchasable_offer.
    pub async fn update_price(
        &self,
        access_token: &str,
        seller_id: &str,
        sku: &str,
        marketplace_ids: &[String],
        amount: f64,
        currency: &str,
    ) -> Result<(), SyncError> {
        for marketplace_id in marketplace_ids {
            let url = format!(
                "{}/listings/{}/items/{}/{}?marketplaceIds={}",
                self.api_base,
                LISTINGS_API_VERSION,
                url_encode(seller_id),
                url_encode(sku),
                url_encode(marketplace_id),
            );

            let body = AmazonPatchRequest {
                product_type: "PRODUCT".to_string(),
                patches: vec![AmazonPatch {
                    op: "replace".to_string(),
                    path: "/attributes/purchasable_offer".to_string(),
                    value: vec![json!([{
                        "marketplace_id": marketplace_id,
                        "currency": currency,
                        "our_price": [{
                            "schedule": [{
                                "value_with_tax": format!("{:.2}", amount),
                            }]
                        }]
                    }])],
                }],
            };

            let resp = self
                .http
                .patch(&url)
                .header("x-amz-access-token", access_token)
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await
                .map_err(|e| SyncError::NetworkError {
                    platform: Platform::Amazon,
                    message: e.to_string(),
                })?;

            let status = resp.status();
            if status.as_u16() == 401 || status.as_u16() == 403 {
                return Err(SyncError::AuthError {
                    platform: Platform::Amazon,
                    message: format!("Access denied ({status})"),
                });
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(SyncError::ApiError {
                    platform: Platform::Amazon,
                    message: format!(
                        "Update price failed for {sku} on {marketplace_id} ({status}): {body}"
                    ),
                });
            }
        }

        debug!(sku = sku, amount = amount, "Updated Amazon price");
        Ok(())
    }

    /// Create a new listing via PUT.
    pub async fn create_listing(
        &self,
        access_token: &str,
        seller_id: &str,
        sku: &str,
        marketplace_ids: &[String],
        product_type: &str,
        attributes: serde_json::Value,
    ) -> Result<AmazonListingSubmissionResponse, SyncError> {
        let mkt_query = Self::marketplace_query(marketplace_ids);
        let url = format!(
            "{}/listings/{}/items/{}/{}?{}",
            self.api_base,
            LISTINGS_API_VERSION,
            url_encode(seller_id),
            url_encode(sku),
            mkt_query,
        );

        let body = AmazonPutListingRequest {
            product_type: product_type.to_string(),
            requirements: "LISTING".to_string(),
            attributes,
        };

        let resp = self
            .http
            .put(&url)
            .header("x-amz-access-token", access_token)
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Amazon,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if status.as_u16() == 401 || status.as_u16() == 403 {
            return Err(SyncError::AuthError {
                platform: Platform::Amazon,
                message: format!("Access denied ({status})"),
            });
        }
        if !status.is_success() {
            let body_text = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Amazon,
                message: format!("Create listing failed ({status}): {body_text}"),
            });
        }

        let submission: AmazonListingSubmissionResponse =
            resp.json().await.map_err(|e| SyncError::ApiError {
                platform: Platform::Amazon,
                message: format!("Failed to parse create listing response: {e}"),
            })?;

        debug!(sku = sku, "Created Amazon listing");
        Ok(submission)
    }
}
