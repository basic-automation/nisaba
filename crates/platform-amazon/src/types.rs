use serde::{Deserialize, Serialize};

/// Amazon LWA OAuth token response.
#[derive(Debug, Deserialize)]
pub struct AmazonTokenResponse {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub token_type: String,
    pub expires_in: i64,
}

/// Response from GET /listings/2021-08-01/items/{sellerId}
#[derive(Debug, Deserialize)]
pub struct AmazonListingsResponse {
    #[serde(default, rename = "items")]
    pub items: Vec<AmazonListingItem>,
    #[serde(rename = "nextToken")]
    pub next_token: Option<String>,
    #[serde(rename = "numberOfResults")]
    pub number_of_results: Option<i64>,
}

/// A single listing item from the SP-API Listings Items API.
#[derive(Debug, Clone, Deserialize)]
pub struct AmazonListingItem {
    pub sku: String,
    #[serde(default)]
    pub summaries: Vec<AmazonListingSummary>,
    #[serde(default, rename = "fulfillmentAvailability")]
    pub fulfillment_availability: Vec<AmazonFulfillmentAvailability>,
    #[serde(default)]
    pub issues: Vec<AmazonListingIssue>,
    #[serde(default)]
    pub offers: Vec<AmazonListingOffer>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AmazonListingSummary {
    #[serde(rename = "marketplaceId")]
    pub marketplace_id: Option<String>,
    #[serde(rename = "itemName")]
    pub item_name: Option<String>,
    #[serde(rename = "asin")]
    pub asin: Option<String>,
    #[serde(rename = "productType")]
    pub product_type: Option<String>,
    #[serde(rename = "mainImage")]
    pub main_image: Option<AmazonImage>,
    #[serde(rename = "status")]
    pub status: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AmazonImage {
    pub link: Option<String>,
    pub height: Option<u32>,
    pub width: Option<u32>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AmazonFulfillmentAvailability {
    #[serde(rename = "fulfillmentChannelCode")]
    pub fulfillment_channel_code: Option<String>,
    pub quantity: Option<i64>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AmazonListingIssue {
    pub code: Option<String>,
    pub message: Option<String>,
    pub severity: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AmazonListingOffer {
    #[serde(rename = "marketplaceId")]
    pub marketplace_id: Option<String>,
    #[serde(rename = "offerType")]
    pub offer_type: Option<String>,
    pub price: Option<AmazonMoneyType>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct AmazonMoneyType {
    #[serde(rename = "currencyCode")]
    pub currency_code: Option<String>,
    pub amount: Option<String>,
}

/// JSON Patch request body for PATCH /listings/2021-08-01/items/{sellerId}/{sku}
#[derive(Debug, Serialize)]
pub struct AmazonPatchRequest {
    #[serde(rename = "productType")]
    pub product_type: String,
    pub patches: Vec<AmazonPatch>,
}

#[derive(Debug, Serialize)]
pub struct AmazonPatch {
    pub op: String,
    pub path: String,
    pub value: Vec<serde_json::Value>,
}

/// PUT request body for creating a new listing.
#[derive(Debug, Serialize)]
pub struct AmazonPutListingRequest {
    #[serde(rename = "productType")]
    pub product_type: String,
    pub requirements: String,
    pub attributes: serde_json::Value,
}

/// Response from listing operations (PATCH/PUT).
#[derive(Debug, Deserialize)]
pub struct AmazonListingSubmissionResponse {
    pub sku: Option<String>,
    pub status: Option<String>,
    #[serde(rename = "submissionId")]
    pub submission_id: Option<String>,
    pub issues: Option<Vec<AmazonListingIssue>>,
}
