use std::sync::Arc;

use nisaba_core::traits::PlatformAdapter;
use nisaba_core::types::{CreateListingRequest, FullListing, ListingPhoto, ListingPrice, Platform, PlatformListing, UpdateListingRequest};
use nisaba_ebay::EbayAdapter;
use nisaba_ebay::types::{EbayCategorySuggestion, EbayAspectMetadata, EbayBusinessPolicy};
use tauri::State;

use crate::state::AppState;

#[derive(serde::Deserialize)]
pub struct CreateListingParams {
    pub target_platform: String,
    pub product_id: String,
    pub title: String,
    pub description_html: Option<String>,
    pub price_amount: Option<f64>,
    pub price_currency: Option<String>,
    pub photo_urls: Vec<String>,
    pub extras: Option<std::collections::HashMap<String, String>>,
    pub variant_id: String,
    pub sku: Option<String>,
    pub quantity: Option<i64>,
}

#[derive(serde::Deserialize)]
pub struct UpdateListingParams {
    pub platform: String,
    pub platform_item_id: String,
    pub title: Option<String>,
    pub description_html: Option<String>,
    pub price_amount: Option<f64>,
    pub price_currency: Option<String>,
    pub tags: Option<Vec<String>>,
    pub extras: Option<std::collections::HashMap<String, String>>,
}

type EbayBusinessPolicies = (Vec<EbayBusinessPolicy>, Vec<EbayBusinessPolicy>, Vec<EbayBusinessPolicy>);

async fn ensure_auth(adapter: &dyn PlatformAdapter) -> Result<(), String> {
    if !adapter.is_authenticated().await {
        adapter
            .refresh_auth()
            .await
            .map_err(|e| format!("{} auth failed: {e}", adapter.platform()))?;
    }
    Ok(())
}

#[tauri::command]
pub async fn fetch_all_listings(
    state: State<'_, AppState>,
    platform: String,
) -> Result<Vec<PlatformListing>, String> {
    let plat = Platform::from_str_loose(&platform)
        .ok_or_else(|| format!("Unknown platform: {platform}"))?;

    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let adapter = adapters
        .get(&plat)
        .ok_or_else(|| format!("Platform {platform} is not enabled"))?;

    ensure_auth(adapter.as_ref()).await?;

    let listings = adapter
        .fetch_all_listings()
        .await
        .map_err(|e| e.to_string())?;

    // Record pricing snapshots and cache the full listing set for this platform.
    if let Ok(db) = state.active_db().await {
        // Pricing snapshots for mapped listings
        if let Ok(mappings) = db.list_all_active_mappings().await {
            let mapped: std::collections::HashMap<(&str, &str), &str> = mappings
                .iter()
                .map(|m| ((m.platform.as_str(), m.platform_item_id.as_str()), m.product_id.as_str()))
                .collect();

            for listing in &listings {
                if let Some(product_id) = mapped.get(&(plat.as_str(), listing.platform_item_id.as_str())) {
                    if let Some(price) = listing.price {
                        let _ = db
                            .record_pricing_snapshot(product_id, plat.as_str(), price, "USD")
                            .await;
                    }
                    if let Some(ref url) = listing.image_url {
                        let _ = db
                            .save_listing_photo(product_id, plat.as_str(), &listing.platform_item_id, url, 0)
                            .await;
                    }
                }
            }
        }

        // Cache the full listing set so the Listings page loads instantly next time
        if let Ok(json) = serde_json::to_string(&listings) {
            let _ = db.save_cached_platform_listings(plat.as_str(), &json).await;
        }
    }

    Ok(listings)
}

#[tauri::command]
pub async fn get_cached_platform_listings(
    state: State<'_, AppState>,
    platform: String,
) -> Result<Vec<PlatformListing>, String> {
    let plat = Platform::from_str_loose(&platform)
        .ok_or_else(|| format!("Unknown platform: {platform}"))?;
    let db = state.active_db().await?;
    match db.get_cached_platform_listings(plat.as_str()).await {
        Ok(Some(json)) => {
            serde_json::from_str::<Vec<PlatformListing>>(&json)
                .map_err(|e| format!("Failed to parse cached listings: {e}"))
        }
        Ok(None) => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

#[tauri::command]
pub async fn fetch_unmapped_listings(
    state: State<'_, AppState>,
    platform: String,
) -> Result<Vec<PlatformListing>, String> {
    let plat = Platform::from_str_loose(&platform)
        .ok_or_else(|| format!("Unknown platform: {platform}"))?;

    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let adapter = adapters
        .get(&plat)
        .ok_or_else(|| format!("Platform {platform} is not enabled"))?;

    ensure_auth(adapter.as_ref()).await?;

    let all_listings = adapter
        .fetch_all_listings()
        .await
        .map_err(|e| e.to_string())?;

    let db = state.active_db().await?;
    let mappings = db
        .list_all_active_mappings()
        .await
        .map_err(|e| e.to_string())?;

    let mapped_ids: std::collections::HashSet<(&str, &str)> = mappings
        .iter()
        .map(|m| (m.platform.as_str(), m.platform_item_id.as_str()))
        .collect();

    let unmapped: Vec<PlatformListing> = all_listings
        .into_iter()
        .filter(|l| !mapped_ids.contains(&(plat.as_str(), l.platform_item_id.as_str())))
        .collect();

    Ok(unmapped)
}

#[tauri::command]
pub async fn diff_listings(
    state: State<'_, AppState>,
    product_id: String,
) -> Result<Vec<FullListing>, String> {
    let db = state.active_db().await?;
    let mappings = db
        .list_mappings_for_product(&product_id)
        .await
        .map_err(|e| e.to_string())?;

    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;

    // Auth once per platform, collect work items with cloned adapter Arcs
    let mut authed_platforms = std::collections::HashSet::new();
    let mut work: Vec<(Arc<dyn PlatformAdapter>, String, Option<String>)> = Vec::new();

    for mapping in &mappings {
        if let Some(adapter) = adapters.get(&mapping.platform) {
            if !authed_platforms.contains(&mapping.platform) {
                if let Err(e) = ensure_auth(adapter.as_ref()).await {
                    tracing::warn!(
                        platform = %mapping.platform,
                        error = %e,
                        "Auth failed for diff"
                    );
                    continue;
                }
                authed_platforms.insert(mapping.platform);
            }
            work.push((
                Arc::clone(adapter),
                mapping.platform_item_id.clone(),
                mapping.variant_id.clone(),
            ));
        }
    }

    // Drop the read lock before spawning tasks
    drop(adapters);

    // Fetch all listings concurrently (max 8 at a time)
    let semaphore = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
    let mut tasks = tokio::task::JoinSet::new();

    for (adapter, item_id, variant_id) in work {
        let sem = semaphore.clone();
        tasks.spawn(async move {
            let _permit = sem.acquire().await;
            match adapter.fetch_full_listing(&item_id).await {
                Ok(mut listing) => {
                    listing.variant_id = variant_id;
                    Some(listing)
                }
                Err(e) => {
                    tracing::warn!(
                        item_id,
                        error = %e,
                        "Failed to fetch full listing for diff"
                    );
                    None
                }
            }
        });
    }

    let mut listings = Vec::new();
    while let Some(result) = tasks.join_next().await {
        if let Ok(Some(listing)) = result {
            listings.push(listing);
        }
    }

    Ok(listings)
}

#[tauri::command]
pub async fn migrate_listing(
    state: State<'_, AppState>,
    source_platform: String,
    source_item_id: String,
    target_platform: String,
    product_id: String,
    variant_id: String,
) -> Result<String, String> {
    let src_plat = Platform::from_str_loose(&source_platform)
        .ok_or_else(|| format!("Unknown source platform: {source_platform}"))?;
    let tgt_plat = Platform::from_str_loose(&target_platform)
        .ok_or_else(|| format!("Unknown target platform: {target_platform}"))?;

    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;

    let src_adapter = adapters
        .get(&src_plat)
        .ok_or_else(|| format!("Source platform {source_platform} is not enabled"))?;
    let tgt_adapter = adapters
        .get(&tgt_plat)
        .ok_or_else(|| format!("Target platform {target_platform} is not enabled"))?;

    ensure_auth(src_adapter.as_ref()).await?;
    ensure_auth(tgt_adapter.as_ref()).await?;

    let listing = src_adapter
        .fetch_full_listing(&source_item_id)
        .await
        .map_err(|e| e.to_string())?;

    let request = CreateListingRequest {
        title: listing.title,
        description_html: listing
            .description
            .as_ref()
            .and_then(|d| d.html.clone()),
        sku: listing.sku,
        quantity: listing.quantity,
        price: listing.price,
        photo_urls: listing.photos.iter().map(|p| p.url.clone()).collect(),
        extras: std::collections::HashMap::new(),
    };

    let new_item_id = tgt_adapter
        .create_listing(request)
        .await
        .map_err(|e| e.to_string())?;

    let db = state.active_db().await?;
    db.insert_mapping(&product_id, tgt_plat.as_str(), &new_item_id, None, &variant_id)
        .await
        .map_err(|e| e.to_string())?;

    Ok(new_item_id)
}

#[tauri::command]
pub async fn create_listing_on_platform(
    state: State<'_, AppState>,
    params: CreateListingParams,
) -> Result<String, String> {
    let tgt_plat = Platform::from_str_loose(&params.target_platform)
        .ok_or_else(|| format!("Unknown platform: {}", params.target_platform))?;

    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let tgt_adapter = adapters
        .get(&tgt_plat)
        .ok_or_else(|| format!("Platform {} is not enabled", params.target_platform))?;

    ensure_auth(tgt_adapter.as_ref()).await?;

    let price = match (params.price_amount, params.price_currency) {
        (Some(amount), Some(currency)) => Some(ListingPrice { amount, currency }),
        _ => None,
    };

    let request = CreateListingRequest {
        title: params.title,
        description_html: params.description_html,
        sku: params.sku.clone(),
        quantity: params.quantity.unwrap_or(1),
        price,
        photo_urls: params.photo_urls,
        extras: params.extras.unwrap_or_default(),
    };

    let new_item_id = tgt_adapter
        .create_listing(request)
        .await
        .map_err(|e| e.to_string())?;

    let db = state.active_db().await?;
    db.insert_mapping(
        &params.product_id,
        tgt_plat.as_str(),
        &new_item_id,
        params.sku.as_deref(),
        &params.variant_id,
    )
        .await
        .map_err(|e| e.to_string())?;

    Ok(new_item_id)
}

#[tauri::command]
pub async fn update_listing_on_platform(
    state: State<'_, AppState>,
    params: UpdateListingParams,
) -> Result<(), String> {
    let plat = Platform::from_str_loose(&params.platform)
        .ok_or_else(|| format!("Unknown platform: {}", params.platform))?;

    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let adapter = adapters
        .get(&plat)
        .ok_or_else(|| format!("Platform {} is not enabled", params.platform))?;

    ensure_auth(adapter.as_ref()).await?;

    let price = match (params.price_amount, params.price_currency) {
        (Some(amount), Some(currency)) => Some(ListingPrice { amount, currency }),
        _ => None,
    };

    let request = UpdateListingRequest {
        title: params.title,
        description_html: params.description_html,
        price,
        tags: params.tags,
        extras: params.extras,
    };

    adapter
        .update_listing(&params.platform_item_id, request)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ebay_get_category_suggestions(
    state: State<'_, AppState>,
    query: String,
    marketplace_id: Option<String>,
) -> Result<Vec<EbayCategorySuggestion>, String> {
    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let adapter = adapters
        .get(&Platform::Ebay)
        .ok_or_else(|| "eBay is not enabled".to_string())?;

    ensure_auth(adapter.as_ref()).await?;

    let ebay = adapter
        .as_any()
        .downcast_ref::<EbayAdapter>()
        .ok_or_else(|| "Failed to downcast to EbayAdapter".to_string())?;

    let marketplace = marketplace_id.as_deref().unwrap_or("EBAY_US");
    ebay.get_category_suggestions(&query, marketplace)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ebay_get_category_aspects(
    state: State<'_, AppState>,
    category_id: String,
    marketplace_id: Option<String>,
) -> Result<Vec<EbayAspectMetadata>, String> {
    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let adapter = adapters
        .get(&Platform::Ebay)
        .ok_or_else(|| "eBay is not enabled".to_string())?;

    ensure_auth(adapter.as_ref()).await?;

    let ebay = adapter
        .as_any()
        .downcast_ref::<EbayAdapter>()
        .ok_or_else(|| "Failed to downcast to EbayAdapter".to_string())?;

    let marketplace = marketplace_id.as_deref().unwrap_or("EBAY_US");
    ebay.get_category_aspects(&category_id, marketplace)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ebay_get_business_policies(
    state: State<'_, AppState>,
    marketplace_id: Option<String>,
) -> Result<EbayBusinessPolicies, String> {
    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let adapter = adapters
        .get(&Platform::Ebay)
        .ok_or_else(|| "eBay is not enabled".to_string())?;

    ensure_auth(adapter.as_ref()).await?;

    let ebay = adapter
        .as_any()
        .downcast_ref::<EbayAdapter>()
        .ok_or_else(|| "Failed to downcast to EbayAdapter".to_string())?;

    let marketplace = marketplace_id.as_deref().unwrap_or("EBAY_US");
    ebay.get_business_policies(marketplace)
        .await
        .map_err(|e| e.to_string())
}

/// Load cached full listings for a product (instant, no API calls).
#[tauri::command]
pub async fn get_cached_listings(
    state: State<'_, AppState>,
    product_id: String,
) -> Result<Vec<FullListing>, String> {
    let db = state.active_db().await?;
    let jsons = db
        .get_cached_listings(&product_id)
        .await
        .map_err(|e| e.to_string())?;

    let mut listings = Vec::new();
    for json in jsons {
        if let Ok(listing) = serde_json::from_str::<FullListing>(&json) {
            listings.push(listing);
        }
    }
    Ok(listings)
}

/// Fetch full listing detail from a platform and cache photos, description, and price in DB.
#[tauri::command]
pub async fn cache_listing_detail(
    state: State<'_, AppState>,
    product_id: String,
    platform: String,
    platform_item_id: String,
) -> Result<(), String> {
    let plat = Platform::from_str_loose(&platform)
        .ok_or_else(|| format!("Unknown platform: {platform}"))?;

    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let adapter = adapters
        .get(&plat)
        .ok_or_else(|| format!("Platform {platform} is not enabled"))?;

    ensure_auth(adapter.as_ref()).await?;

    let listing = adapter
        .fetch_full_listing(&platform_item_id)
        .await
        .map_err(|e| e.to_string())?;

    let db = state.active_db().await?;

    // Cache photos
    if !listing.photos.is_empty() {
        let photos: Vec<ListingPhoto> = listing.photos.iter().map(|p| ListingPhoto {
            url: p.url.clone(),
            position: p.position,
            width: p.width,
            height: p.height,
            alt_text: p.alt_text.clone(),
        }).collect();
        let _ = db
            .replace_listing_photos(&product_id, plat.as_str(), &platform_item_id, &photos)
            .await;
    }

    // Cache description
    if let Some(ref desc) = listing.description {
        let _ = db
            .upsert_listing_description(
                &product_id,
                plat.as_str(),
                &platform_item_id,
                desc.html.as_deref(),
                desc.plain_text.as_deref(),
            )
            .await;
    }

    // Cache price
    if let Some(ref price) = listing.price {
        let _ = db
            .record_pricing_snapshot(&product_id, plat.as_str(), price.amount, &price.currency)
            .await;
    }

    // Cache full listing JSON for instant loading
    if let Ok(json) = serde_json::to_string(&listing) {
        let _ = db
            .save_cached_listing(&product_id, plat.as_str(), &platform_item_id, &json)
            .await;
    }

    Ok(())
}
