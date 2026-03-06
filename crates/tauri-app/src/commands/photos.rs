use nisaba_core::types::{ListingPhoto, Platform};
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub async fn get_product_photos(
    state: State<'_, AppState>,
    product_id: String,
    platform: Option<String>,
) -> Result<Vec<(Platform, ListingPhoto)>, String> {
    let db = state.active_db().await?;
    db.get_listing_photos(&product_id, platform.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn sync_photo(
    state: State<'_, AppState>,
    target_platform: String,
    target_item_id: String,
    photo_url: String,
    position: i32,
) -> Result<String, String> {
    let plat = Platform::from_str_loose(&target_platform)
        .ok_or_else(|| format!("Unknown platform: {target_platform}"))?;

    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let adapter = adapters
        .get(&plat)
        .ok_or_else(|| format!("Platform {target_platform} is not enabled"))?;

    if !adapter.is_authenticated().await {
        adapter
            .refresh_auth()
            .await
            .map_err(|e| format!("{plat} auth failed: {e}"))?;
    }

    adapter
        .upload_photo(&target_item_id, &photo_url, position)
        .await
        .map_err(|e| e.to_string())
}
