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
    // Built in core (Db::export_data) so the export the UI writes is the
    // exact one the round-trip test covers, stamped with the current
    // format version (nisaba_core::types::EXPORT_FORMAT_VERSION).
    let export = db.export_data().await.map_err(|e| e.to_string())?;

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
    std::fs::write(&file_path, &json).map_err(|e| format!("Failed to write file: {e}"))?;

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
    let contents =
        std::fs::read_to_string(&file_path).map_err(|e| format!("Failed to read file: {e}"))?;

    let data: ExportData =
        serde_json::from_str(&contents).map_err(|e| format!("Invalid export file: {e}"))?;

    // Compat shim lives in core: every earlier 1.x format imports, and
    // Db::import_data re-checks before writing anything.
    data.validate_version()?;

    let db = state.active_db().await?;
    db.import_data(&data).await.map_err(|e| e.to_string())?;

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
