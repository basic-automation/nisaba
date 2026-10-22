use nisaba_core::error::SyncError;
use nisaba_core::types::Platform;
use reqwest::Client;
use tracing::debug;

use crate::types::*;

const API_BASE: &str = "https://api.squarespace.com";

/// Decide the cursor for the next page, or `None` to stop paginating.
///
/// Squarespace can report `hasNextPage: true` with a null (or empty, or
/// repeated) `nextPageCursor`; following that literally re-requests the
/// first page forever. A next page exists only when the API hands back a
/// fresh, non-empty cursor.
pub fn next_cursor(
    pagination: Option<&SquarespacePagination>,
    current: &Option<String>,
) -> Option<String> {
    let pagination = pagination?;
    if !pagination.has_next_page {
        return None;
    }
    let cursor = pagination.next_page_cursor.as_ref()?;
    if cursor.is_empty() || current.as_deref() == Some(cursor.as_str()) {
        return None;
    }
    Some(cursor.clone())
}

pub struct SquarespaceClient {
    http: Client,
    api_base: String,
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
        Self {
            http,
            api_base: API_BASE.to_string(),
        }
    }

    /// Construct a client pointed at an explicit API base URL. Used by
    /// fake-server tests.
    pub fn with_api_base(api_base: String) -> Self {
        let http = Client::builder()
            .user_agent("Mozilla/5.0 (compatible; Nisaba/0.1.0)")
            .build()
            .expect("Failed to build HTTP client");
        Self { http, api_base }
    }

    /// Fetch one page of inventory items.
    ///
    /// N-11.1 uses a single page as the setup wizard's "Test connection"
    /// probe — it proves the API key is accepted without pulling the whole
    /// catalog.
    pub async fn fetch_inventory_page(
        &self,
        api_key: &str,
        cursor: Option<&str>,
    ) -> Result<SquarespaceInventoryResponse, SyncError> {
        let mut url = format!("{}/1.0/commerce/inventory", self.api_base);
        if let Some(c) = cursor {
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

        resp.json().await.map_err(|e| SyncError::ApiError {
            platform: Platform::Squarespace,
            message: format!("Failed to parse inventory response: {e}"),
        })
    }

    /// Fetch all inventory items with pagination.
    pub async fn fetch_inventory(
        &self,
        api_key: &str,
    ) -> Result<Vec<SquarespaceInventoryItem>, SyncError> {
        let mut all_items = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let response = self
                .fetch_inventory_page(api_key, cursor.as_deref())
                .await?;

            all_items.extend(response.inventory);

            match next_cursor(response.pagination.as_ref(), &cursor) {
                Some(c) => cursor = Some(c),
                None => break,
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
                    message: format!("Products fetch failed ({status}): {body}"),
                });
            }

            let response: SquarespaceProductsResponse =
                resp.json().await.map_err(|e| SyncError::ApiError {
                    platform: Platform::Squarespace,
                    message: format!("Failed to parse products response: {e}"),
                })?;

            all_products.extend(response.products);

            match next_cursor(response.pagination.as_ref(), &cursor) {
                Some(c) => cursor = Some(c),
                None => break,
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

#[cfg(test)]
mod tests {
    use super::next_cursor;
    use crate::types::SquarespacePagination;

    fn pagination(has_next_page: bool, cursor: Option<&str>) -> SquarespacePagination {
        SquarespacePagination {
            has_next_page,
            next_page_cursor: cursor.map(str::to_string),
        }
    }

    #[test]
    fn a_fresh_cursor_is_followed() {
        let page = pagination(true, Some("cursor-2"));
        assert_eq!(next_cursor(Some(&page), &None).as_deref(), Some("cursor-2"));
        assert_eq!(
            next_cursor(Some(&page), &Some("cursor-1".to_string())).as_deref(),
            Some("cursor-2")
        );
    }

    #[test]
    fn a_null_cursor_terminates_pagination_even_when_a_next_page_is_claimed() {
        // Regression: `hasNextPage: true` with a null cursor used to loop
        // forever re-requesting the first page.
        let page = pagination(true, None);
        assert_eq!(next_cursor(Some(&page), &None), None);
        assert_eq!(
            next_cursor(Some(&page), &Some("cursor-1".to_string())),
            None
        );
    }

    #[test]
    fn an_empty_or_repeated_cursor_terminates_pagination() {
        let empty = pagination(true, Some(""));
        assert_eq!(next_cursor(Some(&empty), &None), None);

        let repeated = pagination(true, Some("cursor-1"));
        assert_eq!(
            next_cursor(Some(&repeated), &Some("cursor-1".to_string())),
            None,
            "re-requesting the same cursor would loop forever"
        );
    }

    #[test]
    fn no_next_page_or_no_pagination_block_terminates() {
        let last = pagination(false, Some("cursor-2"));
        assert_eq!(next_cursor(Some(&last), &None), None);
        assert_eq!(next_cursor(None, &None), None);
    }
}
