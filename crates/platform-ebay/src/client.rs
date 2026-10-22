use nisaba_core::error::SyncError;
use nisaba_core::types::Platform;
use reqwest::Client;
use rust_decimal::Decimal;
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

/// The URL of the next page of a paged Inventory/Offer response, from the
/// response's `next` href.
///
/// eBay paginates with a full `next` URL, not just an offset: callers must
/// follow it rather than synthesise their own offsets (the href also encodes
/// the page size and any continuation state). Returns `None` — pagination
/// is over, or the offset fallback applies — when the href is absent,
/// empty, or points back at the URL just fetched (which would loop).
pub fn next_page_url(next: Option<&str>, current_url: &str) -> Option<String> {
    let next = next?;
    if next.is_empty() || next == current_url {
        return None;
    }
    Some(next.to_string())
}

/// Recursively remove object entries whose value is JSON `null`, so a
/// re-serialised GET response does not send explicit nulls back in a PUT
/// body (eBay treats a present-but-null field as clearing it).
fn strip_nulls(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            map.retain(|_, v| !v.is_null());
            for v in map.values_mut() {
                strip_nulls(v);
            }
        }
        serde_json::Value::Array(items) => {
            for v in items.iter_mut() {
                strip_nulls(v);
            }
        }
        _ => {}
    }
}

/// Fields of an inventory item that a read-modify-write is allowed to
/// change. Every other field of the complete body comes from the fetched
/// item.
#[derive(Debug, Default, Clone, Copy)]
pub struct InventoryItemOverrides<'a> {
    pub description: Option<&'a str>,
    pub title: Option<&'a str>,
    pub aspects: Option<&'a std::collections::HashMap<String, Vec<String>>>,
    pub condition: Option<&'a str>,
    pub condition_description: Option<&'a str>,
}

/// Build the `createOrReplaceInventoryItem` PUT body for a description /
/// metadata edit, losslessly (N-5.2).
///
/// The body is the just-fetched item re-serialised — modelled fields,
/// explicitly carried fields (brand, MPN, UPC, `packageWeightAndSize`,
/// `conditionDescriptors`, `conditionDescription`) and every unknown field
/// captured in the `extra` flatten maps — with only the caller's overrides
/// applied. Unknown fields therefore round-trip instead of being erased
/// by the full replace.
///
/// Stale-quantity hazard: `availability.shipToLocationAvailability.
/// quantity` (and any per-location distribution quantity) is *omitted*,
/// never written back. A quantity read moments earlier can already be
/// stale if a sale landed in between; writing it back would resurrect
/// sold stock. Quantity changes go through the bulk price/quantity
/// endpoint instead, which is the path that owns quantity.
pub fn build_inventory_item_put_body(
    existing: &EbayInventoryItem,
    overrides: &InventoryItemOverrides<'_>,
) -> serde_json::Value {
    let InventoryItemOverrides {
        description,
        title,
        aspects,
        condition,
        condition_description,
    } = *overrides;
    let mut body = serde_json::to_value(existing).unwrap_or_else(|_| json!({}));
    if let Some(obj) = body.as_object_mut() {
        // `sku` identifies the item in the PUT path; it is not a body field.
        obj.remove("sku");
    }
    strip_nulls(&mut body);

    let has_product_override = description.is_some() || title.is_some() || aspects.is_some();
    if has_product_override {
        if !body.get("product").is_some_and(|p| p.is_object()) {
            body["product"] = json!({});
        }
        if let Some(desc) = description {
            body["product"]["description"] = json!(desc);
        }
        if let Some(t) = title {
            body["product"]["title"] = json!(t);
        }
        if let Some(a) = aspects {
            body["product"]["aspects"] = json!(a);
        }
    }
    if let Some(c) = condition {
        body["condition"] = json!(c);
    }
    if let Some(cd) = condition_description {
        body["conditionDescription"] = json!(cd);
    }

    // Omit quantity (see above): strip it wherever any availability
    // sub-block carries it (ship-to-location, pickup-at-location, …),
    // including per-location distributions, and drop blocks that held
    // nothing else.
    if let Some(avail) = body.get_mut("availability").and_then(|a| a.as_object_mut()) {
        for sub in avail.values_mut() {
            if let Some(sub) = sub.as_object_mut() {
                sub.remove("quantity");
                if let Some(dists) = sub
                    .get_mut("availabilityDistributions")
                    .and_then(|d| d.as_array_mut())
                {
                    for dist in dists.iter_mut() {
                        if let Some(d) = dist.as_object_mut() {
                            d.remove("quantity");
                        }
                    }
                }
            }
        }
        avail.retain(|_, v| !(v.is_object() && v.as_object().is_some_and(|o| o.is_empty())));
    }
    if body
        .get("availability")
        .is_some_and(|a| a.is_object() && a.as_object().is_some_and(|o| o.is_empty()))
    {
        if let Some(obj) = body.as_object_mut() {
            obj.remove("availability");
        }
    }

    body
}

/// Fields of an offer that `update_listing` is allowed to change. Every
/// other field of the complete body comes from the fetched offer.
#[derive(Debug, Default, Clone, Copy)]
pub struct OfferOverrides<'a> {
    pub category_id: Option<&'a str>,
    pub fulfillment_policy_id: Option<&'a str>,
    pub payment_policy_id: Option<&'a str>,
    pub return_policy_id: Option<&'a str>,
    pub listing_description: Option<&'a str>,
}

/// The dotted paths `updateOffer` requires in a full-replace PUT body.
fn missing_offer_fields(body: &serde_json::Value) -> Vec<&'static str> {
    let mut missing = Vec::new();
    for key in [
        "sku",
        "marketplaceId",
        "format",
        "categoryId",
        "availableQuantity",
    ] {
        if body.get(key).is_none() {
            missing.push(match key {
                "sku" => "sku",
                "marketplaceId" => "marketplaceId",
                "format" => "format",
                "categoryId" => "categoryId",
                _ => "availableQuantity",
            });
        }
    }
    if body
        .pointer("/pricingSummary/price/value")
        .and_then(|v| v.as_str())
        .is_none_or(|s| s.is_empty())
    {
        missing.push("pricingSummary.price.value");
    }
    if body
        .pointer("/pricingSummary/price/currency")
        .and_then(|v| v.as_str())
        .is_none_or(|s| s.is_empty())
    {
        missing.push("pricingSummary.price.currency");
    }
    for (key, label) in [
        ("fulfillmentPolicyId", "listingPolicies.fulfillmentPolicyId"),
        ("paymentPolicyId", "listingPolicies.paymentPolicyId"),
        ("returnPolicyId", "listingPolicies.returnPolicyId"),
    ] {
        if body
            .pointer(&format!("/listingPolicies/{key}"))
            .and_then(|v| v.as_str())
            .is_none_or(|s| s.is_empty())
        {
            missing.push(label);
        }
    }
    missing
}

/// Reject a body that is not a complete `updateOffer` full replace.
pub fn validate_complete_offer_body(body: &serde_json::Value) -> Result<(), SyncError> {
    let missing = missing_offer_fields(body);
    if missing.is_empty() {
        Ok(())
    } else {
        Err(SyncError::ApiError {
            platform: Platform::Ebay,
            message: format!(
                "refusing to send a partial body to updateOffer (a full replace): \
                 missing required field(s): {}",
                missing.join(", ")
            ),
        })
    }
}

/// Build a complete `updateOffer` PUT body from a fetched offer (N-5.2):
/// the offer re-serialised losslessly (unmodelled fields via `extra`),
/// minus the read-only `offerId` / `listingId` / `status`, with only the
/// caller's overrides applied. Returns an explicit error when the fetched
/// offer lacks a field a full replace requires — a partial body is never
/// produced.
pub fn build_offer_put_body(
    existing: &EbayOffer,
    overrides: &OfferOverrides<'_>,
) -> Result<serde_json::Value, SyncError> {
    let mut body = serde_json::to_value(existing).unwrap_or_else(|_| json!({}));
    if let Some(obj) = body.as_object_mut() {
        obj.remove("offerId");
        obj.remove("listingId");
        obj.remove("status");
    }
    strip_nulls(&mut body);

    if let Some(cat) = overrides.category_id {
        body["categoryId"] = json!(cat);
    }
    if let Some(desc) = overrides.listing_description {
        body["listingDescription"] = json!(desc);
    }
    let has_policy_override = overrides.fulfillment_policy_id.is_some()
        || overrides.payment_policy_id.is_some()
        || overrides.return_policy_id.is_some();
    if has_policy_override {
        if !body.get("listingPolicies").is_some_and(|p| p.is_object()) {
            body["listingPolicies"] = json!({});
        }
        if let Some(id) = overrides.fulfillment_policy_id {
            body["listingPolicies"]["fulfillmentPolicyId"] = json!(id);
        }
        if let Some(id) = overrides.payment_policy_id {
            body["listingPolicies"]["paymentPolicyId"] = json!(id);
        }
        if let Some(id) = overrides.return_policy_id {
            body["listingPolicies"]["returnPolicyId"] = json!(id);
        }
    }

    validate_complete_offer_body(&body).map_err(|e| match e {
        SyncError::ApiError { message, .. } => SyncError::ApiError {
            platform: Platform::Ebay,
            message: format!(
                "cannot update eBay offer {}: {message}",
                existing.offer_id.as_deref().unwrap_or("?")
            ),
        },
        other => other,
    })?;
    Ok(body)
}

pub struct EbayClient {
    http: Client,
    api_base: String,
}

/// Inspect a bulk-update response and surface any per-SKU failure as `Err`.
///
/// eBay returns HTTP 200 when every entry succeeds and 207 when some (or
/// all) fail; in both cases the per-entry `statusCode` / `errors` in the
/// body are authoritative. Treating a 207 with per-SKU errors as success
/// would report a write that never happened.
pub(crate) fn check_bulk_update_response(
    response: &BulkUpdateResponse,
    context: &str,
) -> Result<(), SyncError> {
    let Some(responses) = &response.responses else {
        return Ok(());
    };
    let mut failures: Vec<String> = Vec::new();
    for entry in responses {
        let status_failed = entry
            .status_code
            .map(|c| !(200..300).contains(&c))
            .unwrap_or(false);
        let has_errors = entry
            .errors
            .as_ref()
            .map(|e| !e.is_empty())
            .unwrap_or(false);
        if !status_failed && !has_errors {
            continue;
        }
        if let Some(errors) = &entry.errors {
            for err in errors {
                warn!(
                    sku = entry.sku.as_deref().unwrap_or("?"),
                    error_id = err.error_id,
                    message = err.message.as_deref().unwrap_or(""),
                    "eBay bulk update error for item"
                );
            }
        }
        let detail = entry
            .errors
            .as_ref()
            .map(|errs| {
                errs.iter()
                    .filter_map(|e| e.message.clone())
                    .collect::<Vec<_>>()
                    .join("; ")
            })
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| format!("statusCode {}", entry.status_code.unwrap_or_default()));
        failures.push(format!("{}: {detail}", entry.sku.as_deref().unwrap_or("?")));
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(SyncError::ApiError {
            platform: Platform::Ebay,
            message: format!(
                "{context} failed for {} SKU(s): {}",
                failures.len(),
                failures.join(" | ")
            ),
        })
    }
}

/// Build the `bulk_update_price_quantity` body for a price-only update.
///
/// The offer entry carries the real `offerId` (without it eBay cannot
/// target an offer and the price is never changed) and there is
/// deliberately no `shipToLocationAvailability` — a price edit must
/// never write a quantity, so it can never zero stock.
pub(crate) fn build_bulk_price_body(
    sku: &str,
    offer_id: &str,
    price: Decimal,
    currency: &str,
) -> serde_json::Value {
    json!({
        "requests": [{
            "sku": sku,
            "offers": [{
                "offerId": offer_id,
                "price": {
                    "value": format!("{:.2}", price),
                    "currency": currency,
                }
            }]
        }]
    })
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

    /// Construct a client pointed at an explicit API base URL. Used by
    /// fake-server tests (and any future non-standard endpoint).
    pub fn with_api_base(api_base: String) -> Self {
        let http = Client::builder()
            .user_agent("Nisaba/0.1.0")
            .build()
            .expect("Failed to build HTTP client");
        Self { http, api_base }
    }

    pub fn http(&self) -> &Client {
        &self.http
    }

    /// Fetch one page of inventory items from the eBay Inventory API.
    ///
    /// Returns the parsed page (items plus the server-reported total) so
    /// callers can decide whether to keep paginating. N-11.1 uses a single
    /// page as the setup wizard's "Test connection" probe.
    pub async fn fetch_inventory_page(
        &self,
        access_token: &str,
        limit: usize,
        offset: usize,
    ) -> Result<EbayInventoryItemsResponse, SyncError> {
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

        resp.json().await.map_err(|e| SyncError::ApiError {
            platform: Platform::Ebay,
            message: format!("Failed to parse inventory response: {e}"),
        })
    }

    /// Fetch all inventory items from the eBay Inventory API.
    pub async fn fetch_inventory_items(
        &self,
        access_token: &str,
    ) -> Result<Vec<EbayInventoryItem>, SyncError> {
        let mut all_items = Vec::new();
        let mut offset = 0;
        let limit = 100;
        let mut next_url: Option<String> = None;

        loop {
            // Follow the response's `next` href when it offers one; the
            // offset arithmetic is only the fallback for responses
            // without one.
            let response: EbayInventoryItemsResponse;
            let url: String;
            if let Some(next) = next_url.take() {
                url = next;
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

                response = resp.json().await.map_err(|e| SyncError::ApiError {
                    platform: Platform::Ebay,
                    message: format!("Failed to parse inventory response: {e}"),
                })?;
            } else {
                url = format!(
                    "{}/sell/inventory/v1/inventory_item?limit={}&offset={}",
                    self.api_base, limit, offset
                );
                response = self
                    .fetch_inventory_page(access_token, limit, offset)
                    .await?;
            }

            let next = response.next.clone();
            let items = response.inventory_items.unwrap_or_default();
            let count = items.len();
            all_items.extend(items);

            let total = response.total.unwrap_or(0) as usize;
            offset += count;

            if let Some(next) = next_page_url(next.as_deref(), &url) {
                next_url = Some(next);
                continue;
            }
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

            // Per-SKU errors (including non-2xx statusCodes on a 200/207
            // response) must surface as Err, not a warned-about Ok.
            check_bulk_update_response(&response, "Bulk update")?;
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

        let item: EbayInventoryItem = resp.json().await.map_err(|e| SyncError::ApiError {
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
        let mut next_url: Option<String> = None;

        loop {
            let url = next_url.take().unwrap_or_else(|| {
                format!(
                    "{}/sell/inventory/v1/offer?sku={}&limit={}&offset={}",
                    self.api_base,
                    url_encode(sku),
                    limit,
                    offset
                )
            });

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

            let next = response.next.clone();
            let offers = response.offers.unwrap_or_default();
            let count = offers.len();
            all_offers.extend(offers);

            let total = response.total.unwrap_or(0) as usize;
            offset += count;

            if let Some(next) = next_page_url(next.as_deref(), &url) {
                next_url = Some(next);
                continue;
            }
            if offset >= total || count == 0 {
                break;
            }
        }

        debug!(sku = sku, count = all_offers.len(), "Fetched eBay offers");
        Ok(all_offers)
    }

    /// Fetch all offers across all SKUs (for bulk price lookup).
    pub async fn fetch_all_offers(&self, access_token: &str) -> Result<Vec<EbayOffer>, SyncError> {
        let mut all_offers = Vec::new();
        let mut offset = 0;
        let limit = 100;
        let mut next_url: Option<String> = None;

        loop {
            let url = next_url.take().unwrap_or_else(|| {
                format!(
                    "{}/sell/inventory/v1/offer?limit={}&offset={}",
                    self.api_base, limit, offset
                )
            });

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

            let next = response.next.clone();
            let offers = response.offers.unwrap_or_default();
            let count = offers.len();
            all_offers.extend(offers);

            let total = response.total.unwrap_or(0) as usize;
            offset += count;

            if let Some(next) = next_page_url(next.as_deref(), &url) {
                next_url = Some(next);
                continue;
            }
            if offset >= total || count == 0 {
                break;
            }
        }

        debug!(count = all_offers.len(), "Fetched all eBay offers");
        Ok(all_offers)
    }

    /// Update an inventory item's description via PUT (full replace).
    /// The item is re-read immediately before the PUT and the body is built
    /// losslessly from that read (see [`build_inventory_item_put_body`]):
    /// unmodelled fields round-trip and quantity is omitted rather than
    /// written back stale.
    pub async fn update_inventory_item(
        &self,
        access_token: &str,
        sku: &str,
        description: &str,
    ) -> Result<(), SyncError> {
        // Fetch existing item to preserve all other fields
        let existing = self.fetch_inventory_item(access_token, sku).await?;
        let body = build_inventory_item_put_body(
            &existing,
            &InventoryItemOverrides {
                description: Some(description),
                ..Default::default()
            },
        );

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

    /// Update price for a single SKU using the bulk update endpoint.
    /// Re-uses the existing bulk endpoint since eBay doesn't have a single-item
    /// price update in the Inventory API -- price is set via offer or bulk.
    ///
    /// The request targets a real offer via `offer_id` and carries no
    /// quantity at all, so a price edit can never alter (or zero) stock.
    /// The 200/207 response body is parsed and any per-SKU failure is
    /// surfaced as `Err`.
    pub async fn bulk_update_price_quantity(
        &self,
        access_token: &str,
        sku: &str,
        offer_id: &str,
        price: Decimal,
        currency: &str,
    ) -> Result<(), SyncError> {
        let url = format!(
            "{}/sell/inventory/v1/bulk_update_price_quantity",
            self.api_base
        );

        let body = build_bulk_price_body(sku, offer_id, price, currency);

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
        let body_text = resp.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Bulk update price/quantity failed ({status}): {body_text}"),
            });
        }
        if !body_text.trim().is_empty() {
            let response: BulkUpdateResponse =
                serde_json::from_str(&body_text).map_err(|e| SyncError::ApiError {
                    platform: Platform::Ebay,
                    message: format!("Failed to parse bulk update price response: {e}"),
                })?;
            check_bulk_update_response(&response, "Bulk update price")?;
        }

        debug!(
            sku = sku,
            price = %price,
            "Updated eBay price via bulk endpoint"
        );
        Ok(())
    }

    /// Fetch a single offer by offer ID (GET /sell/inventory/v1/offer/{id}).
    pub async fn fetch_offer(
        &self,
        access_token: &str,
        offer_id: &str,
    ) -> Result<EbayOffer, SyncError> {
        let url = format!(
            "{}/sell/inventory/v1/offer/{}",
            self.api_base,
            url_encode(offer_id)
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
                message: format!("Fetch offer failed ({status}): {body}"),
            });
        }

        let offer: EbayOffer = resp.json().await.map_err(|e| SyncError::ApiError {
            platform: Platform::Ebay,
            message: format!("Failed to parse offer response: {e}"),
        })?;

        debug!(offer_id = offer_id, "Fetched eBay offer");
        Ok(offer)
    }

    /// Update an offer's category / policies / listing description via a
    /// lossless read-modify-write: fetch the complete offer, rebuild a
    /// complete PUT body from it (see [`build_offer_put_body`]) and PUT
    /// that. If the fetched offer cannot yield a complete body, this
    /// returns an explicit error and sends nothing.
    pub async fn update_offer_fields(
        &self,
        access_token: &str,
        offer_id: &str,
        overrides: &OfferOverrides<'_>,
    ) -> Result<(), SyncError> {
        let existing = self.fetch_offer(access_token, offer_id).await?;
        let body = build_offer_put_body(&existing, overrides)?;
        self.update_offer(access_token, offer_id, &body).await
    }

    /// Update an existing offer by offer ID (full PUT). The body must be
    /// complete (see [`validate_complete_offer_body`]); a partial body is
    /// rejected before anything is sent, because `updateOffer` replaces
    /// the whole offer.
    pub async fn update_offer(
        &self,
        access_token: &str,
        offer_id: &str,
        body: &serde_json::Value,
    ) -> Result<(), SyncError> {
        validate_complete_offer_body(body)?;
        let url = format!("{}/sell/inventory/v1/offer/{}", self.api_base, offer_id);

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

    /// Update an inventory item via a lossless fetch-merge-PUT, supporting
    /// optional description, title, aspects, and condition overrides.
    /// Unmodelled fields round-trip and quantity is omitted rather than
    /// written back stale (see [`build_inventory_item_put_body`]). The
    /// item is re-read here, immediately before the PUT.
    pub async fn update_inventory_item_full(
        &self,
        access_token: &str,
        sku: &str,
        overrides: &InventoryItemOverrides<'_>,
    ) -> Result<(), SyncError> {
        let existing = self.fetch_inventory_item(access_token, sku).await?;
        let body = build_inventory_item_put_body(&existing, overrides);

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

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Get fulfillment policies failed: {body}"),
            });
        }

        let data: EbayFulfillmentPoliciesResponse =
            resp.json().await.map_err(|e| SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Parse fulfillment policies failed: {e}"),
            })?;

        Ok(data
            .policies
            .into_iter()
            .filter_map(|p| {
                Some(EbayBusinessPolicy {
                    id: p.id?,
                    name: p.name.unwrap_or_default(),
                })
            })
            .collect())
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

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Get payment policies failed: {body}"),
            });
        }

        let data: EbayPaymentPoliciesResponse =
            resp.json().await.map_err(|e| SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Parse payment policies failed: {e}"),
            })?;

        Ok(data
            .policies
            .into_iter()
            .filter_map(|p| {
                Some(EbayBusinessPolicy {
                    id: p.id?,
                    name: p.name.unwrap_or_default(),
                })
            })
            .collect())
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

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Get return policies failed: {body}"),
            });
        }

        let data: EbayReturnPoliciesResponse =
            resp.json().await.map_err(|e| SyncError::ApiError {
                platform: Platform::Ebay,
                message: format!("Parse return policies failed: {e}"),
            })?;

        Ok(data
            .policies
            .into_iter()
            .filter_map(|p| {
                Some(EbayBusinessPolicy {
                    id: p.id?,
                    name: p.name.unwrap_or_default(),
                })
            })
            .collect())
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

        response
            .category_tree_id
            .ok_or_else(|| SyncError::ApiError {
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
            self.api_base,
            category_tree_id,
            url_encode(query)
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

        let response: EbayAspectsResponse = resp.json().await.map_err(|e| SyncError::ApiError {
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

        info!(
            count = all_items.len(),
            "Fetched eBay active listings via Trading API"
        );
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// Read one HTTP/1.1 request (headers + content-length body).
    async fn read_request(socket: &mut tokio::net::TcpStream) -> String {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 4096];
        loop {
            // Check if we have the full request already.
            if let Some(header_end) = find_subslice(&buf, b"\r\n\r\n") {
                let header_text = String::from_utf8_lossy(&buf[..header_end]).to_lowercase();
                let content_length: usize = header_text
                    .lines()
                    .find_map(|l| l.strip_prefix("content-length:"))
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0);
                if buf.len() >= header_end + 4 + content_length {
                    break;
                }
            }
            match socket.read(&mut chunk).await {
                Ok(0) => break,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&buf).to_string()
    }

    fn find_subslice(haystack: &[u8], needle: &[u8]) -> Option<usize> {
        haystack.windows(needle.len()).position(|w| w == needle)
    }

    /// Spawn a fake eBay server that answers every request with the given
    /// status/body and captures raw requests for assertions.
    async fn spawn_fake_server(status: u16, body: String) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = captured.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let captured_inner = captured_clone.clone();
                let body_inner = body.clone();
                tokio::spawn(async move {
                    let req = read_request(&mut socket).await;
                    captured_inner.lock().unwrap().push(req);
                    let reason = match status {
                        200 => "OK",
                        207 => "Multi-Status",
                        _ => "Status",
                    };
                    let resp = format!(
                        "HTTP/1.1 {status} {reason}\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body_inner.len(),
                        body_inner
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });
        (format!("http://127.0.0.1:{port}"), captured)
    }

    fn request_body(raw: &str) -> &str {
        raw.split("\r\n\r\n").nth(1).unwrap_or("")
    }

    const REJECTED_BULK_BODY: &str = r#"{"responses":[{"sku":"SKU-1","statusCode":400,"errors":[{"errorId":25021,"message":"Invalid quantity for SKU-1"}]}]}"#;
    const OK_BULK_BODY: &str = r#"{"responses":[{"sku":"SKU-1","statusCode":200}]}"#;

    #[test]
    fn check_bulk_response_surfaces_status_code_without_errors() {
        let resp: BulkUpdateResponse =
            serde_json::from_str(r#"{"responses":[{"sku":"S1","statusCode":400}]}"#).unwrap();
        assert!(check_bulk_update_response(&resp, "ctx").is_err());
        let ok: BulkUpdateResponse =
            serde_json::from_str(r#"{"responses":[{"sku":"S1","statusCode":200}]}"#).unwrap();
        assert!(check_bulk_update_response(&ok, "ctx").is_ok());
    }

    #[test]
    fn price_body_targets_offer_and_never_writes_quantity() {
        let body = build_bulk_price_body("SKU-1", "offer-123", Decimal::new(125, 1), "USD");
        let text = body.to_string();
        assert!(text.contains("\"offerId\":\"offer-123\""), "{text}");
        assert!(!text.contains("quantity"), "{text}");
        assert!(!text.contains("shipToLocationAvailability"), "{text}");
    }

    #[test]
    fn price_body_serializes_a_computed_price_exactly() {
        // N-6.1: 0.1 + 0.2 in f64 is 0.30000000000000004; the eBay bulk
        // body must carry "0.30".
        use std::str::FromStr;
        let amount = Decimal::from_str("0.1").unwrap() + Decimal::from_str("0.2").unwrap();
        let body = build_bulk_price_body("SKU-1", "offer-123", amount, "USD");
        let text = body.to_string();
        assert!(
            text.contains(r#""value":"0.30""#),
            "price must serialize exactly, body: {text}"
        );
        assert!(
            !text.contains("0.30000000000000004"),
            "f64 artifact leaked into the eBay price body: {text}"
        );
    }

    /// set_quantity path: a per-SKU rejection (HTTP 207) must be Err.
    #[tokio::test]
    async fn bulk_update_quantity_rejected_write_surfaces_as_err() {
        let (base, _captured) = spawn_fake_server(207, REJECTED_BULK_BODY.to_string()).await;
        let client = EbayClient::with_api_base(base);
        let err = client
            .bulk_update_quantity("token", vec![("SKU-1".to_string(), 5)])
            .await
            .expect_err("a rejected per-SKU write must surface as Err");
        assert!(
            err.to_string().contains("SKU-1"),
            "error should name the SKU, got: {err}"
        );
    }

    #[tokio::test]
    async fn bulk_update_quantity_success_returns_ok() {
        let (base, _captured) = spawn_fake_server(200, OK_BULK_BODY.to_string()).await;
        let client = EbayClient::with_api_base(base);
        client
            .bulk_update_quantity("token", vec![("SKU-1".to_string(), 5)])
            .await
            .expect("a 200 per-SKU result must be Ok");
    }

    /// set_price path: a per-SKU rejection (HTTP 207) must be Err.
    #[tokio::test]
    async fn bulk_update_price_rejected_write_surfaces_as_err() {
        let (base, _captured) = spawn_fake_server(207, REJECTED_BULK_BODY.to_string()).await;
        let client = EbayClient::with_api_base(base);
        let err = client
            .bulk_update_price_quantity("token", "SKU-1", "offer-123", Decimal::new(1999, 2), "USD")
            .await
            .expect_err("a rejected price write must surface as Err");
        assert!(
            err.to_string().contains("SKU-1"),
            "error should name the SKU, got: {err}"
        );
    }

    #[test]
    fn next_page_url_follows_a_fresh_href_only() {
        let current = "https://api.ebay.com/sell/inventory/v1/offer?limit=100&offset=0";
        assert_eq!(
            next_page_url(
                Some("https://api.ebay.com/sell/inventory/v1/offer?limit=100&offset=100"),
                current
            )
            .as_deref(),
            Some("https://api.ebay.com/sell/inventory/v1/offer?limit=100&offset=100")
        );
        // Absent, empty, or self-referential hrefs end pagination instead
        // of looping or synthesising another page.
        assert_eq!(next_page_url(None, current), None);
        assert_eq!(next_page_url(Some(""), current), None);
        assert_eq!(next_page_url(Some(current), current), None);
    }

    /// Pagination follows the response's `next` href: the second request
    /// must go to the href the first response handed back (offset=100),
    /// not to an offset the client computed itself (which would be 1 after
    /// a one-item page).
    #[tokio::test]
    async fn inventory_pagination_follows_the_next_href() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let base = format!("http://127.0.0.1:{port}");
        let next_href = format!("{base}/sell/inventory/v1/inventory_item?limit=100&offset=100");
        let body =
            format!(r#"{{"inventoryItems":[{{"sku":"SKU-1"}}],"total":2,"next":"{next_href}"}}"#);
        let captured: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let captured_clone = captured.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let captured_inner = captured_clone.clone();
                let body_inner = body.clone();
                tokio::spawn(async move {
                    let req = read_request(&mut socket).await;
                    captured_inner.lock().unwrap().push(req);
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                        body_inner.len(),
                        body_inner
                    );
                    let _ = socket.write_all(resp.as_bytes()).await;
                    let _ = socket.flush().await;
                });
            }
        });

        let client = EbayClient::with_api_base(base);
        let items = client
            .fetch_inventory_items("token")
            .await
            .expect("both pages fetch");
        assert_eq!(items.len(), 2, "one item per page, two pages");

        let requests = captured.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert!(
            requests[0].contains("offset=0"),
            "first request: {}",
            requests[0]
        );
        assert!(
            requests[1].contains("offset=100"),
            "second request must follow the next href, got: {}",
            requests[1]
        );
    }

    #[tokio::test]
    async fn bulk_update_price_request_includes_offer_id_and_no_quantity() {
        let (base, captured) = spawn_fake_server(200, OK_BULK_BODY.to_string()).await;
        let client = EbayClient::with_api_base(base);
        client
            .bulk_update_price_quantity("token", "SKU-1", "offer-123", Decimal::new(1999, 2), "USD")
            .await
            .expect("a 200 per-SKU price result must be Ok");
        let requests = captured.lock().unwrap();
        assert_eq!(requests.len(), 1);
        let body = request_body(&requests[0]);
        assert!(body.contains("\"offerId\":\"offer-123\""), "body: {body}");
        assert!(
            !body.contains("quantity"),
            "body must not write quantity: {body}"
        );
        assert!(!body.contains("shipToLocationAvailability"), "body: {body}");
    }
}
