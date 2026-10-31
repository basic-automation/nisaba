use chrono::{DateTime, Utc};
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

impl EbayTokenResponse {
    /// N-11.2: absolute UTC timestamp (`YYYY-MM-DD HH:MM:SS`, the same shape
    /// the adapters persist in `auth_tokens`) at which the refresh token
    /// expires. `None` when the response does not expose a refresh-token
    /// expiry — callers must preserve any previously stored value rather
    /// than erase it. `now` is a parameter so the computation is
    /// unit-testable without clock mocking.
    pub fn refresh_token_expires_at(&self, now: DateTime<Utc>) -> Option<String> {
        self.refresh_token_expires_in.map(|secs| {
            (now + chrono::Duration::seconds(secs))
                .format("%Y-%m-%d %H:%M:%S")
                .to_string()
        })
    }
}

/// eBay inventory item from the Inventory API.
///
/// Lossless by construction (N-5.2): `createOrReplaceInventoryItem` is a
/// full replace, so every field eBay returns that this adapter does not
/// actively edit must survive a read-modify-write cycle. Fields the adapter
/// knows about but does not edit (brand, MPN, UPC, package weight/size,
/// condition descriptors, …) are modelled explicitly; anything else —
/// including fields eBay adds in future — is captured by `extra` via
/// `#[serde(flatten)]` and re-serialised verbatim on the way back out.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EbayInventoryItem {
    pub sku: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub product: Option<EbayProductInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub availability: Option<EbayAvailability>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(
        rename = "conditionDescription",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub condition_description: Option<String>,
    /// e.g. `[{ "name": "3000", "values": ["..."] }]` — carried opaquely.
    #[serde(
        rename = "conditionDescriptors",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub condition_descriptors: Option<serde_json::Value>,
    /// `{ dimensions, weight, ... }` — carried opaquely.
    #[serde(
        rename = "packageWeightAndSize",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub package_weight_and_size: Option<serde_json::Value>,
    /// Any other top-level field (e.g. `locale`) round-trips through here.
    #[serde(flatten, default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EbayProductInfo {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(rename = "imageUrls", default, skip_serializing_if = "Option::is_none")]
    pub image_urls: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aspects: Option<HashMap<String, Vec<String>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brand: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mpn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upc: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ean: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub isbn: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epid: Option<String>,
    /// Unmodelled product fields (e.g. `productIdentifiers`, fields eBay
    /// adds later) round-trip through here.
    #[serde(flatten, default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl Default for EbayProductInfo {
    fn default() -> Self {
        Self {
            title: None,
            description: None,
            image_urls: None,
            aspects: None,
            brand: None,
            mpn: None,
            upc: None,
            ean: None,
            isbn: None,
            epid: None,
            extra: serde_json::Map::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EbayAvailability {
    #[serde(
        rename = "shipToLocationAvailability",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub ship_to_location_availability: Option<EbayShipToLocation>,
    /// e.g. `pickupAtLocationAvailability` and any future availability
    /// blocks round-trip through here.
    #[serde(flatten, default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EbayShipToLocation {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quantity: Option<i64>,
    /// e.g. `availabilityDistributions` round-trips through here.
    #[serde(flatten, default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
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

/// An offer as returned by the Offer API. Lossless like
/// [`EbayInventoryItem`]: `updateOffer` is a full replace, so a complete
/// PUT body is rebuilt from this value (see
/// `client::build_offer_put_body`); unmodelled fields (`listingDuration`,
/// `merchantLocationKey`, `tax`, `charity`, …) round-trip via `extra`.
/// `offerId` / `listingId` / `status` are read-only and are stripped from
/// PUT bodies, never written back.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EbayOffer {
    #[serde(rename = "offerId", default, skip_serializing_if = "Option::is_none")]
    pub offer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sku: Option<String>,
    #[serde(
        rename = "marketplaceId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub marketplace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    #[serde(rename = "listingId", default, skip_serializing_if = "Option::is_none")]
    pub listing_id: Option<String>,
    #[serde(
        rename = "pricingSummary",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub pricing_summary: Option<EbayOfferPrice>,
    #[serde(
        rename = "listingDescription",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub listing_description: Option<String>,
    #[serde(
        rename = "availableQuantity",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub available_quantity: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(
        rename = "categoryId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub category_id: Option<String>,
    #[serde(
        rename = "listingPolicies",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub listing_policies: Option<EbayOfferListingPolicies>,
    /// Unmodelled offer fields round-trip through here.
    #[serde(flatten, default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EbayOfferListingPolicies {
    #[serde(
        rename = "fulfillmentPolicyId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub fulfillment_policy_id: Option<String>,
    #[serde(
        rename = "paymentPolicyId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub payment_policy_id: Option<String>,
    #[serde(
        rename = "returnPolicyId",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub return_policy_id: Option<String>,
    #[serde(flatten, default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EbayOfferPrice {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<EbayAmount>,
    #[serde(flatten, default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EbayAmount {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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
    /// eBay refuses to publish an offer that has no inventory location.
    #[serde(rename = "merchantLocationKey")]
    pub merchant_location_key: String,
}

/// One entry from `GET /sell/inventory/v1/location`.
#[derive(Debug, Clone, Deserialize)]
pub struct EbayInventoryLocation {
    #[serde(rename = "merchantLocationKey")]
    pub merchant_location_key: String,
    #[serde(rename = "merchantLocationStatus", default)]
    pub merchant_location_status: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EbayInventoryLocationsResponse {
    #[serde(default)]
    pub locations: Option<Vec<EbayInventoryLocation>>,
}

/// The location a new offer ships from: the one the caller named, else the first
/// enabled location, else the first location of any status. `None` when the seller
/// has none at all.
pub fn pick_merchant_location(
    requested: Option<&str>,
    locations: &[EbayInventoryLocation],
) -> Option<String> {
    if let Some(key) = requested.filter(|k| !k.is_empty()) {
        return Some(key.to_string());
    }
    locations
        .iter()
        .find(|l| l.merchant_location_status.as_deref() == Some("ENABLED"))
        .or_else(|| locations.first())
        .map(|l| l.merchant_location_key.clone())
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

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn token_response(refresh_token_expires_in: Option<i64>) -> EbayTokenResponse {
        EbayTokenResponse {
            access_token: "a".to_string(),
            expires_in: 7200,
            token_type: "Bearer".to_string(),
            refresh_token: Some("r".to_string()),
            refresh_token_expires_in,
        }
    }

    /// N-11.2: the absolute refresh-token expiry is `now + refresh_token_expires_in`.
    #[test]
    fn refresh_token_expires_at_adds_lifetime_to_now() {
        let token = token_response(Some(86_400)); // 1 day
        let now = Utc.with_ymd_and_hms(2027, 1, 1, 12, 0, 0).unwrap();
        assert_eq!(
            token.refresh_token_expires_at(now).as_deref(),
            Some("2027-01-02 12:00:00")
        );
    }

    /// N-11.2: no expiry in the response means no timestamp — the caller
    /// must preserve any previously stored value, not erase it.
    #[test]
    fn refresh_token_expires_at_is_none_when_not_exposed() {
        let token = token_response(None);
        let now = Utc.with_ymd_and_hms(2027, 1, 1, 12, 0, 0).unwrap();
        assert_eq!(token.refresh_token_expires_at(now), None);
    }
}
