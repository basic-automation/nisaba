use std::collections::HashMap;

use nisaba_core::types::{
    FullListing, ListingPhoto, ListingPrice, Platform, PlatformInventoryItem, PlatformListing,
};
use rust_decimal::Decimal;

use crate::types::{AmazonListingIssue, AmazonListingItem, AmazonListingOffer};

/// Total quantity across every fulfilment channel.
///
/// Amazon does not guarantee the order of `fulfillmentAvailability`, and a
/// listing can be stocked on several channels at once (e.g. merchant-fulfilled
/// `DEFAULT` plus FBA `AMAZON_NA`). Taking only the first entry made the
/// reported quantity depend on array order and understated stock, which the
/// sync engine then resolved as the cross-platform minimum. Summing every
/// channel is deterministic and order-independent.
pub fn total_quantity(item: &AmazonListingItem) -> i64 {
    item.fulfillment_availability
        .iter()
        .map(|fa| fa.quantity.unwrap_or(0))
        .sum()
}

/// The offer for the configured marketplace(s), if any.
///
/// Offers are considered in the order the marketplaces are configured, so the
/// result is deterministic and never depends on the order Amazon returned the
/// offers in. When `marketplace_ids` is empty (no configuration available to
/// the caller) the first offer is used as a fallback. When marketplaces are
/// configured but none of them has an offer, `None` is returned rather than
/// silently reporting another marketplace's price.
pub fn selected_offer<'a>(
    item: &'a AmazonListingItem,
    marketplace_ids: &[String],
) -> Option<&'a AmazonListingOffer> {
    if marketplace_ids.is_empty() {
        return item.offers.first();
    }
    marketplace_ids.iter().find_map(|id| {
        item.offers
            .iter()
            .find(|o| o.marketplace_id.as_deref() == Some(id.as_str()))
    })
}

/// The price from [`selected_offer`], keeping the offer's currency.
///
/// A missing currency code on an otherwise priced offer falls back to `USD`,
/// matching the adapter's historical behaviour; a present code (e.g. `CAD`
/// for the Canada marketplace) is preserved verbatim.
pub fn selected_price(
    item: &AmazonListingItem,
    marketplace_ids: &[String],
) -> Option<ListingPrice> {
    let offer = selected_offer(item, marketplace_ids)?;
    let money = offer.price.as_ref()?;
    let amount: Decimal = money.amount.as_ref()?.parse().ok()?;
    Some(ListingPrice {
        amount,
        currency: money
            .currency_code
            .clone()
            .unwrap_or_else(|| "USD".to_string()),
    })
}

/// ERROR-severity issues on the listing, in the order Amazon returned them.
pub fn error_issues(item: &AmazonListingItem) -> Vec<&AmazonListingIssue> {
    item.issues
        .iter()
        .filter(|i| {
            i.severity
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("ERROR"))
        })
        .collect()
}

/// Carry ERROR-severity issues into a `FullListing`'s `extras`.
///
/// A suppressed listing otherwise maps to a normal listing with zero stock
/// and no explanation. The joined `amazon_error_issues` entry keeps every
/// error's code and message visible in the mapping output; the count and the
/// indexed per-issue entries let a UI surface them individually.
pub fn error_issue_extras(item: &AmazonListingItem) -> HashMap<String, String> {
    let errors = error_issues(item);
    let mut extras = HashMap::new();
    if errors.is_empty() {
        return extras;
    }
    let joined = errors
        .iter()
        .map(|i| match (i.code.as_deref(), i.message.as_deref()) {
            (Some(code), Some(msg)) => format!("{code}: {msg}"),
            (Some(code), None) => code.to_string(),
            (None, Some(msg)) => msg.to_string(),
            (None, None) => "ERROR".to_string(),
        })
        .collect::<Vec<_>>()
        .join("; ");
    extras.insert("amazon_error_issues".to_string(), joined);
    extras.insert(
        "amazon_error_issue_count".to_string(),
        errors.len().to_string(),
    );
    for (idx, issue) in errors.iter().enumerate() {
        if let Some(code) = &issue.code {
            extras.insert(format!("amazon_error_issue_{idx}_code"), code.clone());
        }
        if let Some(message) = &issue.message {
            extras.insert(format!("amazon_error_issue_{idx}_message"), message.clone());
        }
    }
    extras
}

/// Convert an Amazon listing item to a PlatformInventoryItem (for sync engine).
pub fn to_inventory_item(item: &AmazonListingItem) -> PlatformInventoryItem {
    PlatformInventoryItem {
        platform_item_id: item.sku.clone(),
        quantity: total_quantity(item),
    }
}

/// Convert an Amazon listing item to a PlatformListing (for mapping UI).
///
/// Without configured marketplaces the first offer sets the price; prefer
/// [`to_listing_for_marketplaces`] when the configured `marketplace_ids` are
/// known. `PlatformListing` carries a bare amount, so the currency is only
/// preserved on the `FullListing` / `ListingPrice` paths.
pub fn to_listing(item: &AmazonListingItem) -> PlatformListing {
    to_listing_for_marketplaces(item, &[])
}

/// Marketplace-aware variant of [`to_listing`]: the price amount comes from
/// the configured marketplace's offer, not from whichever offer is first.
pub fn to_listing_for_marketplaces(
    item: &AmazonListingItem,
    marketplace_ids: &[String],
) -> PlatformListing {
    let title = item
        .summaries
        .first()
        .and_then(|s| s.item_name.clone())
        .unwrap_or_else(|| item.sku.clone());

    let image_url = item
        .summaries
        .first()
        .and_then(|s| s.main_image.as_ref())
        .and_then(|img| img.link.clone());

    let price = selected_price(item, marketplace_ids).map(|p| p.amount);

    PlatformListing {
        platform_item_id: item.sku.clone(),
        title,
        sku: Some(item.sku.clone()),
        quantity: total_quantity(item),
        price,
        image_url,
        group_key: None,
        variant_attributes: HashMap::new(),
    }
}

/// Convert an Amazon listing item to a FullListing (for detail view).
pub fn to_full_listing(item: &AmazonListingItem) -> FullListing {
    to_full_listing_for_marketplaces(item, &[])
}

/// Marketplace-aware variant of [`to_full_listing`]: the price (amount and
/// currency) comes from the configured marketplace's offer, and
/// ERROR-severity issues are carried through `extras`.
pub fn to_full_listing_for_marketplaces(
    item: &AmazonListingItem,
    marketplace_ids: &[String],
) -> FullListing {
    let title = item
        .summaries
        .first()
        .and_then(|s| s.item_name.clone())
        .unwrap_or_else(|| item.sku.clone());

    let photos: Vec<ListingPhoto> = item
        .summaries
        .first()
        .and_then(|s| s.main_image.as_ref())
        .and_then(|img| img.link.clone())
        .into_iter()
        .enumerate()
        .map(|(i, url)| ListingPhoto {
            url,
            position: i as i32,
            width: None,
            height: None,
            alt_text: None,
        })
        .collect();

    let price = selected_price(item, marketplace_ids);

    let asin = item.summaries.first().and_then(|s| s.asin.clone());

    let url = asin.map(|asin| format!("https://www.amazon.com/dp/{}", asin));

    FullListing {
        platform: Platform::Amazon,
        platform_item_id: item.sku.clone(),
        title,
        sku: Some(item.sku.clone()),
        quantity: total_quantity(item),
        price,
        description: None, // Amazon listings don't expose description via Listings Items API
        photos,
        url,
        fetched_at: chrono::Utc::now().to_rfc3339(),
        extras: error_issue_extras(item),
        variant_id: None,
    }
}
