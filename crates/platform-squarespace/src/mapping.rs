use std::collections::HashMap;

use chrono::Utc;
use nisaba_core::types::{
    FullListing, ListingDescription, ListingPhoto, ListingPrice, Platform, PlatformInventoryItem,
    PlatformListing,
};

use crate::types::{SquarespaceInventoryItem, SquarespaceProduct};

/// Convert a Squarespace inventory item to the common PlatformInventoryItem.
/// Returns None for unlimited inventory (not quantity-trackable).
pub fn to_inventory_item(item: &SquarespaceInventoryItem) -> Option<PlatformInventoryItem> {
    if item.is_unlimited {
        return None;
    }
    Some(PlatformInventoryItem {
        platform_item_id: item.variant_id.clone(),
        quantity: item.quantity,
    })
}

/// Convert Squarespace products + inventory data into full PlatformListings.
/// Merges product info (name, images, price) with inventory data (quantity).
/// Skips variants with unlimited inventory (not quantity-trackable).
pub fn to_listings(
    products: &[SquarespaceProduct],
    inventory: &[SquarespaceInventoryItem],
) -> Vec<PlatformListing> {
    // Build lookups from inventory data
    let inv_qty: HashMap<&str, i64> = inventory
        .iter()
        .filter(|i| !i.is_unlimited)
        .map(|i| (i.variant_id.as_str(), i.quantity))
        .collect();

    let inv_variant_name: HashMap<&str, Option<&str>> = inventory
        .iter()
        .map(|i| (i.variant_id.as_str(), i.variant_name.as_deref()))
        .collect();

    let unlimited_ids: std::collections::HashSet<&str> = inventory
        .iter()
        .filter(|i| i.is_unlimited)
        .map(|i| i.variant_id.as_str())
        .collect();

    let mut listings = Vec::new();

    for product in products {
        let image_url = product
            .images
            .as_ref()
            .and_then(|imgs| imgs.first())
            .and_then(|img| img.url.clone());

        if let Some(variants) = &product.variants {
            let multi_variant = variants.len() > 1;

            for variant in variants {
                // Skip unlimited inventory — can't track quantity
                if unlimited_ids.contains(variant.id.as_str()) {
                    continue;
                }

                let quantity = inv_qty.get(variant.id.as_str()).copied().unwrap_or(0);

                // Append variant name to title when product has multiple variants
                let title = if multi_variant {
                    let vname = inv_variant_name
                        .get(variant.id.as_str())
                        .and_then(|v| *v)
                        .or(variant.sku.as_deref())
                        .unwrap_or("Default");
                    format!("{} [{}]", product.name, vname)
                } else {
                    product.name.clone()
                };

                let price = variant
                    .pricing
                    .as_ref()
                    .and_then(|p| p.base_price.as_ref())
                    .and_then(|bp| bp.value.as_ref())
                    .and_then(|v| v.parse::<f64>().ok());

                listings.push(PlatformListing {
                    platform_item_id: variant.id.clone(),
                    title,
                    sku: variant.sku.clone(),
                    quantity,
                    price,
                    image_url: image_url.clone(),
                    group_key: Some(product.id.clone()),
                    variant_attributes: variant.attributes.clone(),
                });
            }
        }
    }

    listings
}

/// Build a FullListing from Squarespace product data, a specific variant ID, and inventory data.
/// Returns None if the variant_id is not found within the product's variants.
pub fn to_full_listing(
    product: &SquarespaceProduct,
    variant_id: &str,
    inventory: &[SquarespaceInventoryItem],
) -> Option<FullListing> {
    let variants = product.variants.as_ref()?;
    let variant = variants.iter().find(|v| v.id == variant_id)?;

    // Look up quantity from inventory data
    let quantity = inventory
        .iter()
        .find(|inv| inv.variant_id == variant_id)
        .map(|inv| inv.quantity)
        .unwrap_or(0);

    // Build the variant-aware title
    let title = if variants.len() > 1 {
        let vname = inventory
            .iter()
            .find(|inv| inv.variant_id == variant_id)
            .and_then(|inv| inv.variant_name.as_deref())
            .or(variant.sku.as_deref())
            .unwrap_or("Default");
        format!("{} [{}]", product.name, vname)
    } else {
        product.name.clone()
    };

    // Extract price from the variant's pricing.basePrice
    let price = variant
        .pricing
        .as_ref()
        .and_then(|p| p.base_price.as_ref())
        .and_then(|bp| {
            let amount = bp.value.as_ref()?.parse::<f64>().ok()?;
            let currency = bp.currency.clone().unwrap_or_else(|| "USD".to_string());
            Some(ListingPrice { amount, currency })
        });

    // Build description if present
    let now = Utc::now().to_rfc3339();
    let description = product.description.as_ref().map(|desc| ListingDescription {
        platform: Platform::Squarespace,
        platform_item_id: variant_id.to_string(),
        html: Some(desc.clone()),
        plain_text: None,
        fetched_at: now.clone(),
    });

    // Build photos from product images
    let photos = product
        .images
        .as_ref()
        .map(|imgs| {
            imgs.iter()
                .enumerate()
                .filter_map(|(i, img)| {
                    let url = img.url.as_ref()?;
                    Some(ListingPhoto {
                        url: url.clone(),
                        position: i as i32,
                        width: None,
                        height: None,
                        alt_text: None,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Some(FullListing {
        platform: Platform::Squarespace,
        platform_item_id: variant_id.to_string(),
        title,
        sku: variant.sku.clone(),
        quantity,
        price,
        description,
        photos,
        url: product.url.clone(),
        fetched_at: now,
        extras: std::collections::HashMap::new(),
        variant_id: None,
    })
}
