use std::collections::HashSet;
use std::path::PathBuf;

use nisaba_core::types::ExportData;
use serde::Serialize;
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

/// What `apply_import_data` would change, computed without writing anything.
///
/// The classification mirrors the write statements in `Db::import_data`:
/// products and variants are `INSERT OR REPLACE` (a live row with the same id
/// — or, for products, the same canonical SKU — is replaced), snapshots are
/// `INSERT OR REPLACE`, while mappings and processed orders are
/// `INSERT OR IGNORE` (existing rows are skipped).
#[derive(Debug, Clone, Serialize)]
pub struct ImportPreview {
    pub file_name: String,
    /// Path picked by the file dialog; handed back to `apply_import_data`.
    pub file_path: String,
    pub products_new: usize,
    pub products_replaced: usize,
    pub variants_new: usize,
    pub variants_replaced: usize,
    pub mappings_new: usize,
    pub mappings_skipped: usize,
    pub snapshots_new: usize,
    pub snapshots_replaced: usize,
    pub orders_new: usize,
    pub orders_skipped: usize,
    /// First few replaced product SKUs (`"SKU — Name"`) for display.
    pub replaced_skus: Vec<String>,
    /// First few new product SKUs (`"SKU — Name"`) for display.
    pub new_skus: Vec<String>,
    /// Replaced/new SKUs beyond the capped display lists.
    pub replaced_skus_extra: usize,
    pub new_skus_extra: usize,
}

/// Rows already present in the database, used to diff an import file.
#[derive(Debug, Default)]
struct ExistingRows {
    product_ids: HashSet<String>,
    product_skus: HashSet<String>,
    variant_ids: HashSet<String>,
    mapping_ids: HashSet<i64>,
    snapshot_ids: HashSet<i64>,
    order_ids: HashSet<String>,
}

/// Cap for the SKU lists shown in the import preview dialog.
const PREVIEW_SKU_LIST_CAP: usize = 25;

fn diff_import(data: &ExportData, existing: &ExistingRows) -> ImportPreview {
    let mut preview = ImportPreview {
        file_name: String::new(),
        file_path: String::new(),
        products_new: 0,
        products_replaced: 0,
        variants_new: 0,
        variants_replaced: 0,
        mappings_new: 0,
        mappings_skipped: 0,
        snapshots_new: 0,
        snapshots_replaced: 0,
        orders_new: 0,
        orders_skipped: 0,
        replaced_skus: Vec::new(),
        new_skus: Vec::new(),
        replaced_skus_extra: 0,
        new_skus_extra: 0,
    };

    for product in &data.products {
        // INSERT OR REPLACE fires on the id PK and on the unique SKU index.
        let replaced = existing.product_ids.contains(&product.id)
            || existing.product_skus.contains(&product.canonical_sku);
        let label = format!("{} — {}", product.canonical_sku, product.name);
        if replaced {
            preview.products_replaced += 1;
            if preview.replaced_skus.len() < PREVIEW_SKU_LIST_CAP {
                preview.replaced_skus.push(label);
            } else {
                preview.replaced_skus_extra += 1;
            }
        } else {
            preview.products_new += 1;
            if preview.new_skus.len() < PREVIEW_SKU_LIST_CAP {
                preview.new_skus.push(label);
            } else {
                preview.new_skus_extra += 1;
            }
        }
    }

    for variant in &data.product_variants {
        if existing.variant_ids.contains(&variant.id) {
            preview.variants_replaced += 1;
        } else {
            preview.variants_new += 1;
        }
    }

    // INSERT OR IGNORE: existing rows are skipped, not overwritten.
    for mapping in &data.platform_mappings {
        if existing.mapping_ids.contains(&mapping.id) {
            preview.mappings_skipped += 1;
        } else {
            preview.mappings_new += 1;
        }
    }

    for snapshot in &data.platform_snapshots {
        if existing.snapshot_ids.contains(&snapshot.id) {
            preview.snapshots_replaced += 1;
        } else {
            preview.snapshots_new += 1;
        }
    }

    for order in &data.xmr_processed_orders {
        if existing.order_ids.contains(&order.order_id) {
            preview.orders_skipped += 1;
        } else {
            preview.orders_new += 1;
        }
    }

    preview
}

/// Pick an import file and read + validate it. Shared by preview and apply.
async fn read_import_file(
    app: &tauri::AppHandle,
) -> Result<Option<(String, PathBuf, ExportData)>, String> {
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
        return Ok(None);
    };

    let file_path = path.into_path().map_err(|e| e.to_string())?;
    let file_name = file_path
        .file_name()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let contents =
        std::fs::read_to_string(&file_path).map_err(|e| format!("Failed to read file: {e}"))?;

    let data: ExportData =
        serde_json::from_str(&contents).map_err(|e| format!("Invalid export file: {e}"))?;

    // Compat shim lives in core: every earlier 1.x format imports, and
    // Db::import_data re-checks before writing anything.
    data.validate_version()?;

    Ok(Some((file_name, file_path, data)))
}

/// Show what an import file would change before anything is written.
/// Returns `None` when the user cancels the file picker.
#[tauri::command]
pub async fn preview_import_data(
    state: State<'_, AppState>,
    app: tauri::AppHandle,
) -> Result<Option<ImportPreview>, String> {
    let Some((file_name, file_path, data)) = read_import_file(&app).await? else {
        return Ok(None);
    };

    let db = state.active_db().await?;

    let mut existing = ExistingRows::default();
    for product in db.list_products().await.map_err(|e| e.to_string())? {
        existing.product_ids.insert(product.id.clone());
        existing.product_skus.insert(product.canonical_sku.clone());
    }
    for variant in db.list_all_variants().await.map_err(|e| e.to_string())? {
        existing.variant_ids.insert(variant.id.clone());
    }
    for mapping in db.list_all_mappings().await.map_err(|e| e.to_string())? {
        existing.mapping_ids.insert(mapping.id);
    }
    for snapshot in db.list_all_snapshots().await.map_err(|e| e.to_string())? {
        existing.snapshot_ids.insert(snapshot.id);
    }
    for order in db
        .list_xmr_processed_orders()
        .await
        .map_err(|e| e.to_string())?
    {
        existing.order_ids.insert(order.order_id.clone());
    }

    let mut preview = diff_import(&data, &existing);
    preview.file_name = file_name;
    preview.file_path = file_path.to_string_lossy().into_owned();
    Ok(Some(preview))
}

/// Apply an import file previously shown by `preview_import_data`.
/// The file is re-read and re-validated; nothing is written before this call.
#[tauri::command]
pub async fn apply_import_data(
    state: State<'_, AppState>,
    file_path: String,
) -> Result<String, String> {
    let contents =
        std::fs::read_to_string(&file_path).map_err(|e| format!("Failed to read file: {e}"))?;
    let data: ExportData =
        serde_json::from_str(&contents).map_err(|e| format!("Invalid export file: {e}"))?;
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
    info!("{summary} from {file_path}");

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use nisaba_core::types::{
        Platform, PlatformMapping, PlatformSnapshot, Product, ProductVariant, XmrProcessedOrder,
        EXPORT_FORMAT_VERSION,
    };
    use std::collections::HashMap;

    fn test_product(id: &str, sku: &str) -> Product {
        Product {
            id: id.to_string(),
            canonical_sku: sku.to_string(),
            name: format!("Product {sku}"),
            quantity: 1,
            low_stock_threshold: None,
            is_tracked: true,
            has_variants: true,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn test_variant(id: &str) -> ProductVariant {
        ProductVariant {
            id: id.to_string(),
            product_id: "p1".to_string(),
            sku: "SKU".to_string(),
            name: "Default".to_string(),
            attributes: HashMap::new(),
            quantity: 1,
            on_hand_quantity: 1,
            image_url: None,
            sort_order: 0,
            source_plugin_id: None,
            source_vendor_item_id: None,
            created_at: String::new(),
            updated_at: String::new(),
        }
    }

    fn test_data() -> ExportData {
        ExportData {
            version: EXPORT_FORMAT_VERSION,
            exported_at: String::new(),
            products: vec![
                test_product("p-same-id", "SKU-SAME-ID"),
                test_product("p-new-id", "SKU-LIVE"),
                test_product("p-brand-new", "SKU-NEW"),
            ],
            product_variants: vec![test_variant("v-existing"), test_variant("v-new")],
            platform_mappings: vec![
                PlatformMapping {
                    id: 1,
                    product_id: "p1".to_string(),
                    platform: Platform::Ebay,
                    platform_item_id: "item".to_string(),
                    platform_sku: None,
                    is_active: true,
                    variant_id: None,
                    deleted_at: None,
                },
                PlatformMapping {
                    id: 2,
                    product_id: "p1".to_string(),
                    platform: Platform::Ebay,
                    platform_item_id: "item2".to_string(),
                    platform_sku: None,
                    is_active: true,
                    variant_id: None,
                    deleted_at: None,
                },
            ],
            platform_snapshots: vec![PlatformSnapshot {
                id: 7,
                product_id: "p1".to_string(),
                platform: Platform::Ebay,
                variant_id: None,
                last_known_quantity: 1,
                last_polled_at: String::new(),
                last_pushed_at: None,
                version_tag: None,
            }],
            xmr_processed_orders: vec![XmrProcessedOrder {
                order_id: "ord-existing".to_string(),
                product_id: None,
                listing_id: "l".to_string(),
                quantity: 1,
                processed_at: String::new(),
            }],
        }
    }

    // ── N-11.3: import preview diff ──

    #[test]
    fn preview_classifies_new_replaced_and_skipped_rows() {
        let data = test_data();
        let existing = ExistingRows {
            product_ids: ["p-same-id".to_string()].into_iter().collect(),
            product_skus: ["SKU-LIVE".to_string()].into_iter().collect(),
            variant_ids: ["v-existing".to_string()].into_iter().collect(),
            mapping_ids: [1].into_iter().collect(),
            snapshot_ids: HashSet::new(),
            order_ids: ["ord-existing".to_string()].into_iter().collect(),
        };

        let preview = diff_import(&data, &existing);

        // Same id -> replaced; same SKU under a different id -> replaced
        // (the unique SKU index makes INSERT OR REPLACE swap the row).
        assert_eq!(preview.products_replaced, 2);
        assert_eq!(preview.products_new, 1);
        assert_eq!(preview.variants_replaced, 1);
        assert_eq!(preview.variants_new, 1);
        // Mappings are INSERT OR IGNORE: existing ids are skipped, never replaced.
        assert_eq!(preview.mappings_new, 1);
        assert_eq!(preview.mappings_skipped, 1);
        assert_eq!(preview.snapshots_new, 1);
        assert_eq!(preview.snapshots_replaced, 0);
        assert_eq!(preview.orders_new, 0);
        assert_eq!(preview.orders_skipped, 1);
        assert_eq!(preview.replaced_skus.len(), 2);
        assert_eq!(preview.new_skus.len(), 1);
        assert_eq!(preview.replaced_skus_extra, 0);
    }

    #[test]
    fn preview_caps_sku_display_lists() {
        let mut data = test_data();
        data.products = (0..60)
            .map(|i| test_product(&format!("p{i}"), &format!("SKU-{i}")))
            .collect();
        let preview = diff_import(&data, &ExistingRows::default());
        assert_eq!(preview.products_new, 60);
        assert_eq!(preview.new_skus.len(), PREVIEW_SKU_LIST_CAP);
        assert_eq!(preview.new_skus_extra, 60 - PREVIEW_SKU_LIST_CAP);
    }
}
