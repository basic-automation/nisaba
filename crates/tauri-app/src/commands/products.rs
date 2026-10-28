use std::collections::HashMap;

use nisaba_core::types::{PlatformMapping, Product, ProductVariant};
use serde::Serialize;
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub async fn list_products(state: State<'_, AppState>) -> Result<Vec<Product>, String> {
    let db = state.active_db().await?;
    db.list_products().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_product(
    state: State<'_, AppState>,
    id: String,
) -> Result<Option<Product>, String> {
    let db = state.active_db().await?;
    db.get_product(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_product(
    state: State<'_, AppState>,
    name: String,
    sku: String,
    quantity: i64,
    low_stock_threshold: Option<i64>,
    with_default_variant: Option<bool>,
) -> Result<Product, String> {
    let db = state.active_db().await?;
    let id = uuid::Uuid::new_v4().to_string();
    let product = Product {
        id: id.clone(),
        canonical_sku: sku.clone(),
        name: name.clone(),
        quantity,
        low_stock_threshold,
        is_tracked: true,
        has_variants: true,
        created_at: String::new(),
        updated_at: String::new(),
    };
    db.insert_product(&product)
        .await
        .map_err(|e| e.to_string())?;

    // Auto-create a default variant, unless the caller is about to create the real
    // ones (a listing import): a second variant with the same SKU would violate the
    // unique (product_id, sku) index.
    if with_default_variant.unwrap_or(true) {
        let variant = ProductVariant {
            id: uuid::Uuid::new_v4().to_string(),
            product_id: id.clone(),
            sku,
            name: "Default".to_string(),
            attributes: HashMap::new(),
            quantity,
            on_hand_quantity: quantity,
            image_url: None,
            sort_order: 0,
            source_plugin_id: None,
            source_vendor_item_id: None,
            created_at: String::new(),
            updated_at: String::new(),
        };
        db.insert_variant(&variant)
            .await
            .map_err(|e| e.to_string())?;
    }

    db.get_product(&id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Product not found after insert".to_string())
}

#[tauri::command]
pub async fn update_product(
    state: State<'_, AppState>,
    id: String,
    name: String,
    sku: String,
    low_stock_threshold: Option<i64>,
) -> Result<(), String> {
    let db = state.active_db().await?;
    db.update_product(&id, &name, &sku, low_stock_threshold)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_product(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let db = state.active_db().await?;
    db.delete_product(&id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_mappings(
    state: State<'_, AppState>,
    product_id: String,
) -> Result<Vec<PlatformMapping>, String> {
    let db = state.active_db().await?;
    db.list_mappings_for_product(&product_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn create_mapping(
    state: State<'_, AppState>,
    product_id: String,
    platform: String,
    platform_item_id: String,
    platform_sku: Option<String>,
    variant_id: String,
) -> Result<(), String> {
    let db = state.active_db().await?;
    db.insert_mapping(
        &product_id,
        &platform,
        &platform_item_id,
        platform_sku.as_deref(),
        &variant_id,
    )
    .await
    .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_mapping(state: State<'_, AppState>, id: i64) -> Result<(), String> {
    let db = state.active_db().await?;
    db.delete_mapping(id).await.map_err(|e| e.to_string())
}

/// Batch version of `get_mappings`: one IPC round trip for many products
/// (N-11.7 — replaces the per-product loop on dashboard/listings/products).
/// Only live (non-soft-deleted) mappings are returned, matching `get_mappings`.
#[tauri::command]
pub async fn get_mappings_batch(
    state: State<'_, AppState>,
    product_ids: Vec<String>,
) -> Result<HashMap<String, Vec<PlatformMapping>>, String> {
    let db = state.active_db().await?;
    let all = db.list_all_mappings().await.map_err(|e| e.to_string())?;
    let mut out: HashMap<String, Vec<PlatformMapping>> = HashMap::new();
    for m in all {
        if m.deleted_at.is_none() && product_ids.contains(&m.product_id) {
            out.entry(m.product_id.clone()).or_default().push(m);
        }
    }
    Ok(out)
}

// ── Product Variants ─────────────────────────────────────

#[tauri::command]
pub async fn list_variants(
    state: State<'_, AppState>,
    product_id: String,
) -> Result<Vec<ProductVariant>, String> {
    let db = state.active_db().await?;
    db.list_variants(&product_id)
        .await
        .map_err(|e| e.to_string())
}

/// Batch version of `list_variants`: one IPC round trip for many products
/// (N-11.7 — replaces the per-product loop on the products page).
#[tauri::command]
pub async fn list_variants_batch(
    state: State<'_, AppState>,
    product_ids: Vec<String>,
) -> Result<HashMap<String, Vec<ProductVariant>>, String> {
    let db = state.active_db().await?;
    let all = db.list_all_variants().await.map_err(|e| e.to_string())?;
    let mut out: HashMap<String, Vec<ProductVariant>> = HashMap::new();
    for v in all {
        if product_ids.contains(&v.product_id) {
            out.entry(v.product_id.clone()).or_default().push(v);
        }
    }
    Ok(out)
}

#[tauri::command]
pub async fn create_variant(
    state: State<'_, AppState>,
    product_id: String,
    sku: String,
    name: String,
    attributes: Option<HashMap<String, String>>,
    quantity: i64,
    image_url: Option<String>,
) -> Result<ProductVariant, String> {
    let db = state.active_db().await?;
    let id = uuid::Uuid::new_v4().to_string();
    let variant = ProductVariant {
        id: id.clone(),
        product_id: product_id.clone(),
        sku,
        name,
        attributes: attributes.unwrap_or_default(),
        quantity,
        on_hand_quantity: quantity,
        image_url,
        sort_order: 0,
        source_plugin_id: None,
        source_vendor_item_id: None,
        created_at: String::new(),
        updated_at: String::new(),
    };
    db.insert_variant(&variant)
        .await
        .map_err(|e| e.to_string())?;
    db.recalc_product_quantity(&product_id)
        .await
        .map_err(|e| e.to_string())?;
    db.get_variant(&id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "Variant not found after insert".to_string())
}

#[tauri::command]
pub async fn update_variant(
    state: State<'_, AppState>,
    id: String,
    sku: String,
    name: String,
    attributes: Option<HashMap<String, String>>,
    quantity: i64,
    image_url: Option<String>,
) -> Result<(), String> {
    let db = state.active_db().await?;
    let existing = db
        .get_variant(&id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("Variant not found")?;

    let variant = ProductVariant {
        id: id.clone(),
        product_id: existing.product_id.clone(),
        sku,
        name,
        attributes: attributes.unwrap_or(existing.attributes),
        quantity,
        on_hand_quantity: quantity,
        image_url,
        sort_order: existing.sort_order,
        source_plugin_id: existing.source_plugin_id,
        source_vendor_item_id: existing.source_vendor_item_id,
        created_at: existing.created_at,
        updated_at: String::new(),
    };
    db.update_variant(&variant)
        .await
        .map_err(|e| e.to_string())?;
    db.recalc_product_quantity(&existing.product_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn delete_variant(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let db = state.active_db().await?;
    let variant = db
        .get_variant(&id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("Variant not found")?;
    db.delete_variant(&id).await.map_err(|e| e.to_string())?;
    db.recalc_product_quantity(&variant.product_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub async fn get_all_product_first_photos(
    state: State<'_, AppState>,
) -> Result<Vec<(String, String)>, String> {
    let db = state.active_db().await?;
    db.get_all_product_first_photos()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_all_cache_freshness(
    state: State<'_, AppState>,
) -> Result<Vec<(String, String)>, String> {
    let db = state.active_db().await?;
    db.get_all_cache_freshness()
        .await
        .map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct VariantSkuEntry {
    pub variant_id: String,
    pub product_id: String,
    pub sku: String,
}

#[tauri::command]
pub async fn get_all_variant_skus(
    state: State<'_, AppState>,
) -> Result<Vec<VariantSkuEntry>, String> {
    let db = state.active_db().await?;
    let rows = db.get_all_variant_skus().await.map_err(|e| e.to_string())?;
    Ok(rows
        .into_iter()
        .map(|(variant_id, product_id, sku)| VariantSkuEntry {
            variant_id,
            product_id,
            sku,
        })
        .collect())
}
