use nisaba_core::types::ExportData;
use tauri::State;
use tauri_plugin_dialog::DialogExt;
use tracing::info;

use crate::state::AppState;

#[tauri::command]
pub async fn export_data(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let db = state.active_db().await?;
    let products = db.list_products().await.map_err(|e| e.to_string())?;
    let platform_mappings = db.list_all_mappings().await.map_err(|e| e.to_string())?;
    let platform_snapshots = db.list_all_snapshots().await.map_err(|e| e.to_string())?;
    let xmr_processed_orders = db
        .list_xmr_processed_orders()
        .await
        .map_err(|e| e.to_string())?;

    let product_variants = db
        .list_all_variants()
        .await
        .map_err(|e| e.to_string())?;

    let export = ExportData {
        version: 2,
        exported_at: chrono::Utc::now().to_rfc3339(),
        products,
        product_variants,
        platform_mappings,
        platform_snapshots,
        xmr_processed_orders,
    };

    let json = serde_json::to_string_pretty(&export).map_err(|e| e.to_string())?;

    let dialog = app.dialog().clone();
    let path = tokio::task::spawn_blocking(move || {
        dialog
            .file()
            .set_file_name("nisaba-export.json")
            .add_filter("JSON", &["json"])
            .blocking_save_file()
    })
    .await
    .map_err(|e| e.to_string())?;

    let Some(path) = path else {
        return Ok("Cancelled".to_string());
    };

    let file_path = path.into_path().map_err(|e| e.to_string())?;
    std::fs::write(&file_path, &json)
        .map_err(|e| format!("Failed to write file: {e}"))?;

    let summary = format!(
        "Exported {} products, {} mappings, {} snapshots, {} XMR orders",
        export.products.len(),
        export.platform_mappings.len(),
        export.platform_snapshots.len(),
        export.xmr_processed_orders.len(),
    );
    info!("{summary} to {}", file_path.display());

    Ok(summary)
}

#[tauri::command]
pub async fn import_data(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<String, String> {
    let dialog = app.dialog().clone();
    let path = tokio::task::spawn_blocking(move || {
        dialog
            .file()
            .add_filter("JSON", &["json"])
            .blocking_pick_file()
    })
    .await
    .map_err(|e| e.to_string())?;

    let Some(path) = path else {
        return Ok("Cancelled".to_string());
    };

    let file_path = path.into_path().map_err(|e| e.to_string())?;
    let contents = std::fs::read_to_string(&file_path)
        .map_err(|e| format!("Failed to read file: {e}"))?;

    let data: ExportData =
        serde_json::from_str(&contents).map_err(|e| format!("Invalid export file: {e}"))?;

    if data.version != 1 && data.version != 2 {
        return Err(format!(
            "Unsupported export version {}. Expected 1 or 2.",
            data.version
        ));
    }

    let db = state.active_db().await?;
    db.import_data(&data)
        .await
        .map_err(|e| e.to_string())?;

    let summary = format!(
        "Imported {} products, {} mappings, {} snapshots, {} XMR orders",
        data.products.len(),
        data.platform_mappings.len(),
        data.platform_snapshots.len(),
        data.xmr_processed_orders.len(),
    );
    info!("{summary} from {}", file_path.display());

    Ok(summary)
}
