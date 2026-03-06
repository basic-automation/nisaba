use std::collections::HashMap;

use nisaba_core::types::{
    FullListing, ListingDescription, ListingPhoto, ListingPrice, Platform, PlatformInventoryItem,
    PlatformListing,
};

use crate::types::{EbayInventoryItem, EbayOffer, TradingItem};

/// Convert an eBay inventory item to the common PlatformInventoryItem.
pub fn to_inventory_item(item: &EbayInventoryItem) -> PlatformInventoryItem {
    let quantity = item
        .availability
        .as_ref()
        .and_then(|a| a.ship_to_location_availability.as_ref())
        .and_then(|s| s.quantity)
        .unwrap_or(0);

    PlatformInventoryItem {
        platform_item_id: item.sku.clone(),
        quantity,
    }
}

/// Convert an eBay inventory item to a full PlatformListing for the mapping UI.
pub fn to_listing(item: &EbayInventoryItem) -> PlatformListing {
    let quantity = item
        .availability
        .as_ref()
        .and_then(|a| a.ship_to_location_availability.as_ref())
        .and_then(|s| s.quantity)
        .unwrap_or(0);

    let title = item
        .product
        .as_ref()
        .and_then(|p| p.title.clone())
        .unwrap_or_else(|| item.sku.clone());

    let image_url = item
        .product
        .as_ref()
        .and_then(|p| p.image_urls.as_ref())
        .and_then(|urls| urls.first().cloned());

    PlatformListing {
        platform_item_id: item.sku.clone(),
        title,
        sku: Some(item.sku.clone()),
        quantity,
        price: None, // eBay pricing is in offers, not inventory items
        image_url,
        group_key: None,
        variant_attributes: HashMap::new(),
    }
}

/// Convert an eBay inventory item and its offers into a FullListing.
pub fn to_full_listing(item: &EbayInventoryItem, offers: &[EbayOffer]) -> FullListing {
    let quantity = item
        .availability
        .as_ref()
        .and_then(|a| a.ship_to_location_availability.as_ref())
        .and_then(|s| s.quantity)
        .unwrap_or(0);

    let title = item
        .product
        .as_ref()
        .and_then(|p| p.title.clone())
        .unwrap_or_else(|| item.sku.clone());

    // Extract description from the inventory item's product info
    let description = item
        .product
        .as_ref()
        .and_then(|p| p.description.clone())
        .map(|desc| ListingDescription {
            platform: Platform::Ebay,
            platform_item_id: item.sku.clone(),
            html: Some(desc),
            plain_text: None,
            fetched_at: chrono::Utc::now().to_rfc3339(),
        });

    // Extract photos from imageUrls
    let photos: Vec<ListingPhoto> = item
        .product
        .as_ref()
        .and_then(|p| p.image_urls.as_ref())
        .map(|urls| {
            urls.iter()
                .enumerate()
                .map(|(i, url)| ListingPhoto {
                    url: url.clone(),
                    position: i as i32,
                    width: None,
                    height: None,
                    alt_text: None,
                })
                .collect()
        })
        .unwrap_or_default();

    // Extract price from the first offer that has pricing info
    let price = offers
        .iter()
        .filter_map(|offer| {
            let pricing = offer.pricing_summary.as_ref()?;
            let amount = pricing.price.as_ref()?;
            let value_str = amount.value.as_ref()?;
            let value: f64 = value_str.parse().ok()?;
            let currency = amount
                .currency
                .clone()
                .unwrap_or_else(|| "USD".to_string());
            Some(ListingPrice {
                amount: value,
                currency,
            })
        })
        .next();

    // Try to find a listing URL from any active offer
    let url = offers
        .iter()
        .filter_map(|offer| {
            let listing_id = offer.listing_id.as_ref()?;
            Some(format!("https://www.ebay.com/itm/{}", listing_id))
        })
        .next();

    // Populate extras with eBay-specific metadata for round-tripping
    let mut extras = HashMap::new();

    if let Some(condition) = &item.condition {
        extras.insert("ebay_condition".to_string(), condition.clone());
    }

    if let Some(product) = &item.product {
        if let Some(aspects) = &product.aspects {
            if let Ok(json) = serde_json::to_string(aspects) {
                extras.insert("ebay_aspects".to_string(), json);
            }
        }
    }

    // Extract category, format, and policies from the first offer
    if let Some(offer) = offers.first() {
        if let Some(cat_id) = &offer.category_id {
            extras.insert("ebay_category_id".to_string(), cat_id.clone());
        }
        if let Some(fmt) = &offer.format {
            extras.insert("ebay_format".to_string(), fmt.clone());
        }
        if let Some(policies) = &offer.listing_policies {
            if let Some(id) = &policies.fulfillment_policy_id {
                extras.insert("ebay_fulfillment_policy_id".to_string(), id.clone());
            }
            if let Some(id) = &policies.payment_policy_id {
                extras.insert("ebay_payment_policy_id".to_string(), id.clone());
            }
            if let Some(id) = &policies.return_policy_id {
                extras.insert("ebay_return_policy_id".to_string(), id.clone());
            }
        }
    }

    FullListing {
        platform: Platform::Ebay,
        platform_item_id: item.sku.clone(),
        title,
        sku: Some(item.sku.clone()),
        quantity,
        price,
        description,
        photos,
        url,
        fetched_at: chrono::Utc::now().to_rfc3339(),
        extras,
        variant_id: None,
    }
}

/// Convert a Trading API item to a FullListing (for product detail/comparison).
pub fn trading_to_full_listing(item: &TradingItem) -> FullListing {
    let total_qty = item.quantity.unwrap_or(0);
    let sold_qty = item.quantity_sold.unwrap_or(0);
    let available = total_qty - sold_qty;

    let title = item
        .title
        .clone()
        .unwrap_or_else(|| item.item_id.clone());

    let description = item.description.clone().map(|html| ListingDescription {
        platform: Platform::Ebay,
        platform_item_id: item.item_id.clone(),
        html: Some(html),
        plain_text: None,
        fetched_at: chrono::Utc::now().to_rfc3339(),
    });

    let photos: Vec<ListingPhoto> = item
        .picture_details
        .as_ref()
        .map(|pd| {
            pd.picture_urls
                .iter()
                .enumerate()
                .map(|(i, url)| ListingPhoto {
                    url: url.clone(),
                    position: i as i32,
                    width: None,
                    height: None,
                    alt_text: None,
                })
                .collect()
        })
        .unwrap_or_default();

    let price = item
        .selling_status
        .as_ref()
        .and_then(|ss| ss.current_price.as_ref())
        .and_then(|cp| {
            let val: f64 = cp.value.as_ref()?.parse().ok()?;
            let currency = cp
                .currency_id
                .clone()
                .unwrap_or_else(|| "USD".to_string());
            Some(ListingPrice {
                amount: val,
                currency,
            })
        });

    FullListing {
        platform: Platform::Ebay,
        platform_item_id: item.item_id.clone(),
        title,
        sku: item.sku.clone(),
        quantity: available,
        price,
        description,
        photos,
        url: item.view_item_url.clone(),
        fetched_at: chrono::Utc::now().to_rfc3339(),
        extras: HashMap::new(),
        variant_id: None,
    }
}

/// Convert a Trading API item to a PlatformListing.
pub fn trading_to_listing(item: &TradingItem) -> PlatformListing {
    let total_qty = item.quantity.unwrap_or(0);
    let sold_qty = item.quantity_sold.unwrap_or(0);
    let available = total_qty - sold_qty;

    let title = item
        .title
        .clone()
        .unwrap_or_else(|| item.item_id.clone());

    let image_url = item
        .picture_details
        .as_ref()
        .and_then(|pd| {
            pd.gallery_url
                .clone()
                .or_else(|| pd.picture_urls.first().cloned())
        });

    let price = item
        .selling_status
        .as_ref()
        .and_then(|ss| ss.current_price.as_ref())
        .and_then(|cp| {
            let val: f64 = cp.value.as_ref()?.parse().ok()?;
            Some(val)
        });

    PlatformListing {
        platform_item_id: item.item_id.clone(),
        title,
        sku: item.sku.clone(),
        quantity: available,
        price,
        image_url,
        group_key: None,
        variant_attributes: HashMap::new(),
    }
}
