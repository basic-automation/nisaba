use serde::{Deserialize, Serialize};

// ────── Create Product (POST /1.0/commerce/products) ──────

#[derive(Debug, Serialize)]
pub struct SquarespaceCreateProductRequest {
    #[serde(rename = "type")]
    pub product_type: String,
    #[serde(rename = "storePageId")]
    pub store_page_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(rename = "isVisible", skip_serializing_if = "Option::is_none")]
    pub is_visible: Option<bool>,
    pub variants: Vec<SquarespaceCreateVariant>,
}

#[derive(Debug, Serialize)]
pub struct SquarespaceCreateVariant {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sku: Option<String>,
    pub pricing: SquarespaceCreatePricing,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stock: Option<SquarespaceStock>,
}

#[derive(Debug, Serialize)]
pub struct SquarespaceCreatePricing {
    #[serde(rename = "basePrice")]
    pub base_price: SquarespaceCreatePrice,
}

#[derive(Debug, Serialize)]
pub struct SquarespaceCreatePrice {
    pub currency: String,
    pub value: String,
}

#[derive(Debug, Serialize)]
pub struct SquarespaceStock {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quantity: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unlimited: Option<bool>,
}

#[derive(Debug, Deserialize)]
pub struct SquarespaceCreateProductResponse {
    pub id: String,
    pub variants: Option<Vec<SquarespaceVariant>>,
}

// ────── Update Product (POST /1.0/commerce/products/{id}) ──────

#[derive(Debug, Serialize)]
pub struct SquarespaceUpdateProductRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    #[serde(rename = "isVisible", skip_serializing_if = "Option::is_none")]
    pub is_visible: Option<bool>,
}

// ────── Store Pages (GET /1.0/commerce/store_pages) ──────

#[derive(Debug, Deserialize)]
pub struct SquarespaceStorePageResponse {
    #[serde(rename = "storePages")]
    pub store_pages: Vec<SquarespaceStorePage>,
}

#[derive(Debug, Deserialize)]
pub struct SquarespaceStorePage {
    pub id: String,
    pub title: String,
    #[serde(rename = "isEnabled")]
    pub is_enabled: bool,
}

/// Squarespace inventory response from GET /1.0/commerce/inventory
#[derive(Debug, Deserialize)]
pub struct SquarespaceInventoryResponse {
    pub inventory: Vec<SquarespaceInventoryItem>,
    pub pagination: Option<SquarespacePagination>,
}

#[derive(Debug, Deserialize)]
pub struct SquarespaceInventoryItem {
    #[serde(rename = "variantId")]
    pub variant_id: String,
    pub sku: Option<String>,
    #[serde(rename = "isUnlimited")]
    pub is_unlimited: bool,
    pub quantity: i64,
    #[serde(rename = "productId")]
    pub product_id: Option<String>,
    #[serde(rename = "productName")]
    pub product_name: Option<String>,
    #[serde(rename = "variantName")]
    pub variant_name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SquarespacePagination {
    #[serde(rename = "nextPageCursor")]
    pub next_page_cursor: Option<String>,
    #[serde(rename = "hasNextPage")]
    pub has_next_page: bool,
}

/// Squarespace product from GET /1.0/commerce/products
#[derive(Debug, Deserialize)]
pub struct SquarespaceProductsResponse {
    pub products: Vec<SquarespaceProduct>,
    pub pagination: Option<SquarespacePagination>,
}

#[derive(Debug, Deserialize)]
pub struct SquarespaceProduct {
    pub id: String,
    pub name: String,
    pub description: Option<String>,
    pub url: Option<String>,
    pub variants: Option<Vec<SquarespaceVariant>>,
    pub images: Option<Vec<SquarespaceImage>>,
}

#[derive(Debug, Deserialize)]
pub struct SquarespaceVariant {
    pub id: String,
    pub sku: Option<String>,
    pub pricing: Option<SquarespaceVariantPricing>,
    #[serde(default)]
    pub attributes: std::collections::HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
pub struct SquarespaceVariantPricing {
    #[serde(rename = "basePrice")]
    pub base_price: Option<SquarespacePrice>,
}

#[derive(Debug, Deserialize)]
pub struct SquarespacePrice {
    pub value: Option<String>,
    pub currency: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct SquarespaceImage {
    pub url: Option<String>,
}

/// Request body for POST /1.0/commerce/inventory/adjustments
#[derive(Debug, Serialize)]
pub struct InventoryAdjustmentRequest {
    #[serde(rename = "setFiniteOperations")]
    pub set_finite_operations: Vec<SetFiniteOperation>,
}

#[derive(Debug, Serialize)]
pub struct SetFiniteOperation {
    #[serde(rename = "variantId")]
    pub variant_id: String,
    pub quantity: i64,
}
