use nisaba_core::error::SyncError;
use nisaba_core::types::Platform;
use reqwest::Client;
use tracing::debug;

use crate::types::*;

const API_BASE: &str = "https://api.squarespace.com";

pub struct SquarespaceClient {
    http: Client,
}

impl Default for SquarespaceClient {
    fn default() -> Self {
        Self::new()
    }
}

impl SquarespaceClient {
    pub fn new() -> Self {
        // Squarespace API requires a User-Agent header; their WAF blocks
        // default/bot-like agents with 403.
        let http = Client::builder()
            .user_agent("Mozilla/5.0 (compatible; Nisaba/0.1.0)")
            .build()
            .expect("Failed to build HTTP client");
        Self { http }
    }

    /// Fetch all inventory items with pagination.
    pub async fn fetch_inventory(
        &self,
        api_key: &str,
    ) -> Result<Vec<SquarespaceInventoryItem>, SyncError> {
        let mut all_items = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut url = format!("{}/1.0/commerce/inventory", API_BASE);
            if let Some(ref c) = cursor {
                url.push_str(&format!("?cursor={}", c));
            }

            let resp = self
                .http
                .get(&url)
                .bearer_auth(api_key)
                .send()
                .await
                .map_err(|e| SyncError::NetworkError {
                    platform: Platform::Squarespace,
                    message: e.to_string(),
                })?;

            let status = resp.status();
            if status.as_u16() == 401 {
                return Err(SyncError::AuthError {
                    platform: Platform::Squarespace,
                    message: "Invalid API key".into(),
                });
            }
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(SyncError::ApiError {
                    platform: Platform::Squarespace,
                    message: format!("Inventory fetch failed ({status}): {body}"),
                });
            }

            let response: SquarespaceInventoryResponse =
                resp.json().await.map_err(|e| SyncError::ApiError {
                    platform: Platform::Squarespace,
                    message: format!("Failed to parse inventory response: {e}"),
                })?;

            all_items.extend(response.inventory);

            match response.pagination {
                Some(p) if p.has_next_page => {
                    cursor = p.next_page_cursor;
                }
                _ => break,
            }
        }

        debug!(count = all_items.len(), "Fetched Squarespace inventory");
        Ok(all_items)
    }

    /// Fetch all products (for full listing info including names and images).
    pub async fn fetch_products(
        &self,
        api_key: &str,
    ) -> Result<Vec<SquarespaceProduct>, SyncError> {
        let mut all_products = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let mut url = format!("{}/1.0/commerce/products", API_BASE);
            if let Some(ref c) = cursor {
                url.push_str(&format!("?cursor={}", c));
            }

            let resp = self
                .http
                .get(&url)
                .bearer_auth(api_key)
                .send()
                .await
                .map_err(|e| SyncError::NetworkError {
                    platform: Platform::Squarespace,
                    message: e.to_string(),
                })?;

            let status = resp.status();
            if !status.is_success() {
                let body = resp.text().await.unwrap_or_default();
                return Err(SyncError::ApiError {
                    platform: Platform::Squarespace,
                    message: format!("Products fetch failed ({status}): {body}"),
                });
            }

            let response: SquarespaceProductsResponse =
                resp.json().await.map_err(|e| SyncError::ApiError {
                    platform: Platform::Squarespace,
                    message: format!("Failed to parse products response: {e}"),
                })?;

            all_products.extend(response.products);

            match response.pagination {
                Some(p) if p.has_next_page => {
                    cursor = p.next_page_cursor;
                }
                _ => break,
            }
        }

        debug!(count = all_products.len(), "Fetched Squarespace products");
        Ok(all_products)
    }

    /// Fetch store pages (needed for creating products).
    pub async fn fetch_store_pages(
        &self,
        api_key: &str,
    ) -> Result<Vec<SquarespaceStorePage>, SyncError> {
        let url = format!("{}/1.0/commerce/store_pages", API_BASE);

        let resp = self
            .http
            .get(&url)
            .bearer_auth(api_key)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Squarespace,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Squarespace,
                message: format!("Store pages fetch failed ({status}): {body}"),
            });
        }

        let response: SquarespaceStorePageResponse =
            resp.json().await.map_err(|e| SyncError::ApiError {
                platform: Platform::Squarespace,
                message: format!("Failed to parse store pages response: {e}"),
            })?;

        debug!(
            count = response.store_pages.len(),
            "Fetched Squarespace store pages"
        );
        Ok(response.store_pages)
    }

    /// Create a new product on Squarespace.
    pub async fn create_product(
        &self,
        api_key: &str,
        request: &SquarespaceCreateProductRequest,
    ) -> Result<SquarespaceCreateProductResponse, SyncError> {
        let url = format!("{}/1.0/commerce/products", API_BASE);
        let idempotency_key = uuid::Uuid::new_v4().to_string();

        let resp = self
            .http
            .post(&url)
            .bearer_auth(api_key)
            .header("Idempotency-Key", &idempotency_key)
            .json(request)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Squarespace,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Squarespace,
                message: format!("Product creation failed ({status}): {body}"),
            });
        }

        let response: SquarespaceCreateProductResponse =
            resp.json().await.map_err(|e| SyncError::ApiError {
                platform: Platform::Squarespace,
                message: format!("Failed to parse create product response: {e}"),
            })?;

        debug!(product_id = %response.id, "Created Squarespace product");
        Ok(response)
    }

    /// Update an existing product on Squarespace.
    pub async fn update_product(
        &self,
        api_key: &str,
        product_id: &str,
        request: &SquarespaceUpdateProductRequest,
    ) -> Result<(), SyncError> {
        let url = format!("{}/1.0/commerce/products/{}", API_BASE, product_id);
        let idempotency_key = uuid::Uuid::new_v4().to_string();

        let resp = self
            .http
            .post(&url)
            .bearer_auth(api_key)
            .header("Idempotency-Key", &idempotency_key)
            .json(request)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Squarespace,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Squarespace,
                message: format!("Product update failed ({status}): {body}"),
            });
        }

        debug!(product_id, "Updated Squarespace product");
        Ok(())
    }

    /// Set inventory quantity for a specific variant using the adjustments endpoint.
    /// Requires an Idempotency-Key header per Squarespace API requirements.
    pub async fn set_quantity(
        &self,
        api_key: &str,
        variant_id: &str,
        quantity: i64,
    ) -> Result<(), SyncError> {
        let idempotency_key = uuid::Uuid::new_v4().to_string();

        let body = InventoryAdjustmentRequest {
            set_finite_operations: vec![SetFiniteOperation {
                variant_id: variant_id.to_string(),
                quantity,
            }],
        };

        let url = format!("{}/1.0/commerce/inventory/adjustments", API_BASE);

        let resp = self
            .http
            .post(&url)
            .bearer_auth(api_key)
            .header("Idempotency-Key", &idempotency_key)
            .json(&body)
            .send()
            .await
            .map_err(|e| SyncError::NetworkError {
                platform: Platform::Squarespace,
                message: e.to_string(),
            })?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Squarespace,
                message: format!("Inventory adjustment failed ({status}): {body}"),
            });
        }

        debug!(variant_id, quantity, "Squarespace quantity updated");
        Ok(())
    }
}
