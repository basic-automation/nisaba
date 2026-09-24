use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// eBay OAuth token response.
#[derive(Debug, Deserialize)]
pub struct EbayTokenResponse {
    pub access_token: String,
    pub expires_in: i64,
    pub token_type: String,
    pub refresh_token: Option<String>,
    pub refresh_token_expires_in: Option<i64>,
}

/// eBay inventory item from the Inventory API.
#[derive(Debug, Clone, Deserialize)]
pub struct EbayInventoryItem {
    pub sku: String,
    pub product: Option<EbayProductInfo>,
    pub availability: Option<EbayAvailability>,
    pub condition: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EbayProductInfo {
    pub title: Option<String>,
    pub description: Option<String>,
    #[serde(rename = "imageUrls")]
    pub image_urls: Option<Vec<String>>,
    #[serde(default)]
    pub aspects: Option<HashMap<String, Vec<String>>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EbayAvailability {
    #[serde(rename = "shipToLocationAvailability")]
    pub ship_to_location_availability: Option<EbayShipToLocation>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EbayShipToLocation {
    pub quantity: Option<i64>,
}

/// Response from GET /sell/inventory/v1/inventory_item
#[derive(Debug, Deserialize)]
pub struct EbayInventoryItemsResponse {
    #[serde(rename = "inventoryItems")]
    pub inventory_items: Option<Vec<EbayInventoryItem>>,
    pub total: Option<i64>,
    pub next: Option<String>,
}

/// Request body for POST /sell/inventory/v1/bulk_update_price_quantity
#[derive(Debug, Serialize)]
pub struct BulkUpdateRequest {
    pub requests: Vec<BulkUpdateEntry>,
}

#[derive(Debug, Serialize)]
pub struct BulkUpdateEntry {
    pub sku: String,
    #[serde(rename = "shipToLocationAvailability")]
    pub ship_to_location_availability: ShipToLocationUpdate,
}

#[derive(Debug, Serialize)]
pub struct ShipToLocationUpdate {
    pub quantity: i64,
}

/// Response from the bulk update endpoint.
#[derive(Debug, Deserialize)]
pub struct BulkUpdateResponse {
    pub responses: Option<Vec<BulkUpdateResponseEntry>>,
}

#[derive(Debug, Deserialize)]
pub struct BulkUpdateResponseEntry {
    pub sku: Option<String>,
    #[serde(rename = "statusCode")]
    pub status_code: Option<i32>,
    pub errors: Option<Vec<EbayApiError>>,
}

#[derive(Debug, Deserialize)]
pub struct EbayApiError {
    #[serde(rename = "errorId")]
    pub error_id: Option<i64>,
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Offers API types
// ---------------------------------------------------------------------------

/// Response from GET /sell/inventory/v1/offer?sku=...
#[derive(Debug, Deserialize)]
pub struct EbayOffersResponse {
    pub offers: Option<Vec<EbayOffer>>,
    pub total: Option<i64>,
    pub next: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EbayOffer {
    #[serde(rename = "offerId")]
    pub offer_id: Option<String>,
    pub sku: Option<String>,
    #[serde(rename = "marketplaceId")]
    pub marketplace_id: Option<String>,
    pub format: Option<String>,
    #[serde(rename = "listingId")]
    pub listing_id: Option<String>,
    #[serde(rename = "pricingSummary")]
    pub pricing_summary: Option<EbayOfferPrice>,
    #[serde(rename = "listingDescription")]
    pub listing_description: Option<String>,
    #[serde(rename = "availableQuantity")]
    pub available_quantity: Option<i64>,
    pub status: Option<String>,
    #[serde(rename = "categoryId")]
    pub category_id: Option<String>,
    #[serde(rename = "listingPolicies")]
    pub listing_policies: Option<EbayOfferListingPolicies>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EbayOfferListingPolicies {
    #[serde(rename = "fulfillmentPolicyId")]
    pub fulfillment_policy_id: Option<String>,
    #[serde(rename = "paymentPolicyId")]
    pub payment_policy_id: Option<String>,
    #[serde(rename = "returnPolicyId")]
    pub return_policy_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EbayOfferPrice {
    pub price: Option<EbayAmount>,
}

#[derive(Debug, Deserialize)]
pub struct EbayAmount {
    pub value: Option<String>,
    pub currency: Option<String>,
}

// ---------------------------------------------------------------------------
// Create / update inventory item types
// ---------------------------------------------------------------------------

/// Request body for PUT /sell/inventory/v1/inventory_item/{sku}
#[derive(Debug, Serialize)]
pub struct EbayCreateItemRequest {
    pub product: EbayCreateItemProduct,
    pub availability: Option<EbayCreateItemAvailability>,
    pub condition: Option<String>,
    #[serde(
        rename = "conditionDescription",
        skip_serializing_if = "Option::is_none"
    )]
    pub condition_description: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EbayCreateItemProduct {
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "imageUrls", skip_serializing_if = "Option::is_none")]
    pub image_urls: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub aspects: Option<HashMap<String, Vec<String>>>,
}

#[derive(Debug, Serialize)]
pub struct EbayCreateItemAvailability {
    #[serde(rename = "shipToLocationAvailability")]
    pub ship_to_location_availability: ShipToLocationUpdate,
}

// ---------------------------------------------------------------------------
// Create offer types
// ---------------------------------------------------------------------------

/// Request body for POST /sell/inventory/v1/offer
#[derive(Debug, Serialize)]
pub struct EbayCreateOfferRequest {
    pub sku: String,
    #[serde(rename = "marketplaceId")]
    pub marketplace_id: String,
    pub format: String,
    #[serde(rename = "availableQuantity")]
    pub available_quantity: i64,
    #[serde(rename = "pricingSummary")]
    pub pricing_summary: EbayCreateOfferPricing,
    #[serde(rename = "listingPolicies")]
    pub listing_policies: EbayListingPolicies,
    #[serde(rename = "categoryId", skip_serializing_if = "Option::is_none")]
    pub category_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct EbayCreateOfferPricing {
    pub price: EbayCreateAmount,
}

#[derive(Debug, Serialize)]
pub struct EbayCreateAmount {
    pub value: String,
    pub currency: String,
}

/// Minimal listing policies — the seller must have default policies configured.
/// These IDs are required by the eBay API.
#[derive(Debug, Serialize)]
pub struct EbayListingPolicies {
    #[serde(rename = "fulfillmentPolicyId")]
    pub fulfillment_policy_id: String,
    #[serde(rename = "paymentPolicyId")]
    pub payment_policy_id: String,
    #[serde(rename = "returnPolicyId")]
    pub return_policy_id: String,
}

/// Response from POST /sell/inventory/v1/offer
#[derive(Debug, Deserialize)]
pub struct EbayCreateOfferResponse {
    #[serde(rename = "offerId")]
    pub offer_id: Option<String>,
}

/// Response from POST /sell/inventory/v1/offer/{offerId}/publish
#[derive(Debug, Deserialize)]
pub struct EbayPublishOfferResponse {
    #[serde(rename = "listingId")]
    pub listing_id: Option<String>,
}

// ---------------------------------------------------------------------------
// Trading API types (XML — GetMyeBaySelling)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct TradingGetMyeBaySellingResponse {
    #[serde(rename = "Ack")]
    pub ack: Option<String>,
    #[serde(rename = "ActiveList")]
    pub active_list: Option<TradingActiveList>,
    #[serde(rename = "Errors")]
    pub errors: Option<Vec<TradingError>>,
}

#[derive(Debug, Deserialize)]
pub struct TradingError {
    #[serde(rename = "ShortMessage")]
    pub short_message: Option<String>,
    #[serde(rename = "LongMessage")]
    pub long_message: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TradingActiveList {
    #[serde(rename = "ItemArray")]
    pub item_array: Option<TradingItemArray>,
    #[serde(rename = "PaginationResult")]
    pub pagination_result: Option<TradingPaginationResult>,
}

#[derive(Debug, Deserialize)]
pub struct TradingItemArray {
    #[serde(rename = "Item", default)]
    pub items: Vec<TradingItem>,
}

#[derive(Debug, Deserialize)]
pub struct TradingItem {
    #[serde(rename = "ItemID")]
    pub item_id: String,
    #[serde(rename = "Title")]
    pub title: Option<String>,
    #[serde(rename = "Description")]
    pub description: Option<String>,
    #[serde(rename = "Quantity")]
    pub quantity: Option<i64>,
    #[serde(rename = "QuantitySold")]
    pub quantity_sold: Option<i64>,
    #[serde(rename = "SKU")]
    pub sku: Option<String>,
    #[serde(rename = "SellingStatus")]
    pub selling_status: Option<TradingSellingStatus>,
    #[serde(rename = "PictureDetails")]
    pub picture_details: Option<TradingPictureDetails>,
    #[serde(rename = "ListingType")]
    pub listing_type: Option<String>,
    #[serde(rename = "ViewItemURL")]
    pub view_item_url: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TradingSellingStatus {
    #[serde(rename = "CurrentPrice")]
    pub current_price: Option<TradingAmount>,
}

#[derive(Debug, Deserialize)]
pub struct TradingAmount {
    #[serde(rename = "@currencyID")]
    pub currency_id: Option<String>,
    #[serde(rename = "$text")]
    pub value: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TradingPictureDetails {
    #[serde(rename = "GalleryURL")]
    pub gallery_url: Option<String>,
    #[serde(rename = "PictureURL", default)]
    pub picture_urls: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct TradingPaginationResult {
    #[serde(rename = "TotalNumberOfEntries")]
    pub total_entries: Option<i64>,
    #[serde(rename = "TotalNumberOfPages")]
    pub total_pages: Option<i64>,
}

/// Response from Trading API GetItem call.
#[derive(Debug, Deserialize)]
pub struct TradingGetItemResponse {
    #[serde(rename = "Ack")]
    pub ack: Option<String>,
    #[serde(rename = "Item")]
    pub item: Option<TradingItem>,
    #[serde(rename = "Errors")]
    pub errors: Option<Vec<TradingError>>,
}

// ---------------------------------------------------------------------------
// Commerce Taxonomy API types
// ---------------------------------------------------------------------------

/// Response from GET /commerce/taxonomy/v1/get_default_category_tree_id
#[derive(Debug, Deserialize)]
pub struct EbayCategoryTreeIdResponse {
    #[serde(rename = "categoryTreeId")]
    pub category_tree_id: Option<String>,
}

/// Response from GET /commerce/taxonomy/v1/category_tree/{id}/get_category_suggestions
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbayCategorySuggestionsResponse {
    #[serde(rename = "categorySuggestions")]
    pub category_suggestions: Option<Vec<EbayCategorySuggestion>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbayCategorySuggestion {
    pub category: EbayCategoryInfo,
    #[serde(alias = "categoryTreeNodeAncestors")]
    pub ancestors: Option<Vec<EbayCategoryAncestor>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbayCategoryInfo {
    #[serde(alias = "categoryId")]
    pub category_id: String,
    #[serde(alias = "categoryName")]
    pub category_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbayCategoryAncestor {
    #[serde(alias = "categoryId")]
    pub category_id: String,
    #[serde(alias = "categoryName")]
    pub category_name: String,
}

/// Response from GET /commerce/taxonomy/v1/category_tree/{id}/get_item_aspects_for_category
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbayAspectsResponse {
    pub aspects: Option<Vec<EbayAspectMetadata>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbayAspectMetadata {
    #[serde(alias = "localizedAspectName")]
    pub name: String,
    #[serde(alias = "aspectConstraint")]
    pub constraint: Option<EbayAspectConstraint>,
    #[serde(alias = "aspectValues")]
    pub values: Option<Vec<EbayAspectValue>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbayAspectConstraint {
    #[serde(alias = "aspectRequired")]
    pub required: Option<bool>,
    #[serde(alias = "aspectMode")]
    pub mode: Option<String>,
    #[serde(alias = "aspectDataType")]
    pub data_type: Option<String>,
    #[serde(alias = "itemToAspectCardinality")]
    pub cardinality: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbayAspectValue {
    #[serde(alias = "localizedValue")]
    pub value: String,
}

// ---------------------------------------------------------------------------
// Account API types — seller business policies
// ---------------------------------------------------------------------------

/// A single business policy (fulfillment, payment, or return).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EbayBusinessPolicy {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Deserialize)]
pub struct EbayFulfillmentPoliciesResponse {
    #[serde(rename = "fulfillmentPolicies", default)]
    pub policies: Vec<EbayFulfillmentPolicyEntry>,
}

#[derive(Debug, Deserialize)]
pub struct EbayFulfillmentPolicyEntry {
    #[serde(rename = "fulfillmentPolicyId")]
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EbayPaymentPoliciesResponse {
    #[serde(rename = "paymentPolicies", default)]
    pub policies: Vec<EbayPaymentPolicyEntry>,
}

#[derive(Debug, Deserialize)]
pub struct EbayPaymentPolicyEntry {
    #[serde(rename = "paymentPolicyId")]
    pub id: Option<String>,
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EbayReturnPoliciesResponse {
    #[serde(rename = "returnPolicies", default)]
    pub policies: Vec<EbayReturnPolicyEntry>,
}

#[derive(Debug, Deserialize)]
pub struct EbayReturnPolicyEntry {
    #[serde(rename = "returnPolicyId")]
    pub id: Option<String>,
    pub name: Option<String>,
}

// ---------------------------------------------------------------------------
// eBay condition constants
// ---------------------------------------------------------------------------

pub const EBAY_CONDITIONS: &[(&str, &str)] = &[
    ("NEW", "New"),
    ("LIKE_NEW", "Like New"),
    ("NEW_OTHER", "New (Other)"),
    ("MANUFACTURER_REFURBISHED", "Manufacturer Refurbished"),
    ("SELLER_REFURBISHED", "Seller Refurbished"),
    ("USED_EXCELLENT", "Used - Excellent"),
    ("USED_VERY_GOOD", "Used - Very Good"),
    ("USED_GOOD", "Used - Good"),
    ("USED_ACCEPTABLE", "Used - Acceptable"),
    ("FOR_PARTS_OR_NOT_WORKING", "For Parts or Not Working"),
];
