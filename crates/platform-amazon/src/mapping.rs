use std::collections::HashMap;

use nisaba_core::types::{
    FullListing, ListingPhoto, ListingPrice, Platform, PlatformInventoryItem,
    PlatformListing,
};

use crate::types::AmazonListingItem;

/// Convert an Amazon listing item to a PlatformInventoryItem (for sync engine).
pub fn to_inventory_item(item: &AmazonListingItem) -> PlatformInventoryItem {
    let quantity = item
        .fulfillment_availability
        .first()
        .and_then(|fa| fa.quantity)
        .unwrap_or(0);

    PlatformInventoryItem {
        platform_item_id: item.sku.clone(),
        quantity,
    }
}

/// Convert an Amazon listing item to a PlatformListing (for mapping UI).
pub fn to_listing(item: &AmazonListingItem) -> PlatformListing {
    let quantity = item
        .fulfillment_availability
        .first()
        .and_then(|fa| fa.quantity)
        .unwrap_or(0);

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

    let price = item
        .offers
        .first()
        .and_then(|o| o.price.as_ref())
        .and_then(|p| {
            let val: f64 = p.amount.as_ref()?.parse().ok()?;
            Some(val)
        });

    PlatformListing {
        platform_item_id: item.sku.clone(),
        title,
        sku: Some(item.sku.clone()),
        quantity,
        price,
        image_url,
        group_key: None,
        variant_attributes: HashMap::new(),
    }
}

/// Convert an Amazon listing item to a FullListing (for detail view).
pub fn to_full_listing(item: &AmazonListingItem) -> FullListing {
    let quantity = item
        .fulfillment_availability
        .first()
        .and_then(|fa| fa.quantity)
        .unwrap_or(0);

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

    let price = item
        .offers
        .first()
        .and_then(|o| o.price.as_ref())
        .and_then(|p| {
            let val: f64 = p.amount.as_ref()?.parse().ok()?;
            let currency = p
                .currency_code
                .clone()
                .unwrap_or_else(|| "USD".to_string());
            Some(ListingPrice {
                amount: val,
                currency,
            })
        });

    let asin = item
        .summaries
        .first()
        .and_then(|s| s.asin.clone());

    let url = asin.map(|asin| format!("https://www.amazon.com/dp/{}", asin));

    FullListing {
        platform: Platform::Amazon,
        platform_item_id: item.sku.clone(),
        title,
        sku: Some(item.sku.clone()),
        quantity,
        price,
        description: None, // Amazon listings don't expose description via Listings Items API
        photos,
        url,
        fetched_at: chrono::Utc::now().to_rfc3339(),
        extras: std::collections::HashMap::new(),
        variant_id: None,
    }
}
