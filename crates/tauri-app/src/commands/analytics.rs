use nisaba_core::analytics::{compute_all_analytics, compute_product_analytics};
use nisaba_core::types::{ProductAnalytics, TimeWindow};
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub async fn get_product_analytics(
    state: State<'_, AppState>,
    product_id: String,
    window: Option<TimeWindow>,
) -> Result<ProductAnalytics, String> {
    let db = state.active_db().await?;
    compute_product_analytics(&db, &product_id, window.unwrap_or(TimeWindow::Week))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_all_analytics(
    state: State<'_, AppState>,
    window: Option<TimeWindow>,
) -> Result<Vec<ProductAnalytics>, String> {
    let db = state.active_db().await?;
    compute_all_analytics(&db, window.unwrap_or(TimeWindow::Week))
        .await
        .map_err(|e| e.to_string())
}
