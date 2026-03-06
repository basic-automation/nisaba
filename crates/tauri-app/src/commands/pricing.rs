use nisaba_core::types::{ListingPrice, Platform, PricingSnapshot};
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub async fn get_product_prices(
    state: State<'_, AppState>,
    product_id: String,
) -> Result<Vec<PricingSnapshot>, String> {
    let db = state.active_db().await?;
    db.get_latest_prices(&product_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_all_product_prices(
    state: State<'_, AppState>,
) -> Result<Vec<PricingSnapshot>, String> {
    let db = state.active_db().await?;
    db.get_all_latest_prices()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn update_price(
    state: State<'_, AppState>,
    platform: String,
    platform_item_id: String,
    amount: f64,
    currency: String,
) -> Result<(), String> {
    let plat = Platform::from_str_loose(&platform)
        .ok_or_else(|| format!("Unknown platform: {platform}"))?;

    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let adapter = adapters
        .get(&plat)
        .ok_or_else(|| format!("Platform {platform} is not enabled"))?;

    if !adapter.is_authenticated().await {
        adapter
            .refresh_auth()
            .await
            .map_err(|e| format!("{plat} auth failed: {e}"))?;
    }

    let price = ListingPrice { amount, currency };
    adapter
        .set_price(&platform_item_id, price)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_pricing_history(
    state: State<'_, AppState>,
    product_id: String,
    interval: Option<String>,
) -> Result<Vec<PricingSnapshot>, String> {
    let db = state.active_db().await?;
    db.get_pricing_snapshots(&product_id, interval.as_deref())
        .await
        .map_err(|e| e.to_string())
}
