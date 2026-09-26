use std::collections::HashMap;
use std::io::Read as _;
use std::path::Path;
use std::sync::Arc;

use base64::Engine as _;
use nisaba_core::db::Db;
use nisaba_core::types::{VendorListingCache, VendorPluginRow};
use nisaba_vendor_runtime::{BatchCallback, VendorRuntime};
use serde::Serialize;
use tauri::{Emitter, Manager, State};

use crate::state::AppState;

/// Plugin info returned to the frontend (combines registry + install state).
#[derive(Debug, Clone, Serialize)]
pub struct VendorPluginInfo {
    pub id: String,
    pub plugin_file: String,
    pub display_name: String,
    pub description: String,
    pub version: String,
    pub config_fields: Vec<serde_json::Value>,
    pub status: String,
    pub submitted_by: Option<String>,
    pub approved_by: Option<String>,
    pub has_config: bool,
    pub installed: bool,
    pub enabled: bool,
    pub category: String,
    pub icon: Option<String>,
    pub include_vendor_stock: bool,
    /// `"restricted"` (only `allowed_hosts`), `"unrestricted"` (the plugin declares no
    /// allowlist), or `"unknown"` (not yet computed from this plugin's files — it will be
    /// at install).
    pub network_access: String,
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VendorListingResult {
    pub vendor_item_id: String,
    pub title: String,
    pub price: Option<f64>,
    pub currency: Option<String>,
    pub quantity: Option<i64>,
    pub sku: Option<String>,
    pub image_url: Option<String>,
    pub url: Option<String>,
    pub extras: HashMap<String, String>,
    pub group_key: Option<String>,
    pub variant_attributes: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct VendorSyncEvent {
    pub plugin_id: String,
    pub status: String,
    pub listing_count: usize,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct VendorSyncStatus {
    pub syncing: bool,
    pub last_synced_at: Option<String>,
    pub cached_count: i64,
}

fn listing_to_cache(
    plugin_id: &str,
    l: &nisaba_vendor_runtime::VendorListing,
    fetched_at: &str,
) -> VendorListingCache {
    VendorListingCache {
        plugin_id: plugin_id.to_string(),
        vendor_item_id: l.vendor_item_id.clone(),
        title: l.title.clone(),
        price: l.price,
        currency: l.currency.clone(),
        quantity: l.quantity,
        sku: l.sku.clone(),
        image_url: l.image_url.clone(),
        url: l.url.clone(),
        extras: l.extras.clone(),
        group_key: l.group_key.clone(),
        variant_attributes: l.variant_attributes.clone(),
        fetched_at: fetched_at.to_string(),
    }
}

fn cache_to_result(c: VendorListingCache) -> VendorListingResult {
    VendorListingResult {
        vendor_item_id: c.vendor_item_id,
        title: c.title,
        price: c.price,
        currency: c.currency,
        quantity: c.quantity,
        sku: c.sku,
        image_url: c.image_url,
        url: c.url,
        extras: c.extras,
        group_key: c.group_key,
        variant_attributes: c.variant_attributes,
    }
}

// ── File type helpers ─────────────────────────────────────────

const TEXT_EXTENSIONS: &[&str] = &["ts", "js", "json", "svg", "txt", "md"];
const BINARY_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp"];

fn is_text_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    TEXT_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

fn is_binary_file(name: &str) -> bool {
    let lower = name.to_lowercase();
    BINARY_EXTENSIONS.iter().any(|ext| lower.ends_with(ext))
}

fn mime_for_ext(name: &str) -> &'static str {
    let lower = name.to_lowercase();
    if lower.ends_with(".png") {
        "image/png"
    } else if lower.ends_with(".jpg") || lower.ends_with(".jpeg") {
        "image/jpeg"
    } else if lower.ends_with(".gif") {
        "image/gif"
    } else if lower.ends_with(".webp") {
        "image/webp"
    } else if lower.ends_with(".svg") {
        "image/svg+xml"
    } else {
        "application/octet-stream"
    }
}

/// Extract an icon data URL from a files map (checks icon.svg, icon.png).
fn extract_icon(files: &HashMap<String, String>) -> Option<String> {
    for name in &["icon.svg", "icon.png", "icon.jpg", "icon.webp"] {
        if let Some(content) = files.get(*name) {
            // Already a data URL (binary was encoded during reading)
            if content.starts_with("data:") {
                return Some(content.clone());
            }
            // SVG text content — wrap as data URL
            if name.ends_with(".svg") {
                let encoded = base64::engine::general_purpose::STANDARD.encode(content.as_bytes());
                return Some(format!("data:image/svg+xml;base64,{encoded}"));
            }
        }
    }
    None
}

/// Recursively read a directory into a files map.
fn read_directory_to_files_map(dir_path: &Path) -> Result<HashMap<String, String>, String> {
    let mut files = HashMap::new();

    fn visit(
        base: &Path,
        current: &Path,
        files: &mut HashMap<String, String>,
    ) -> Result<(), String> {
        let entries = std::fs::read_dir(current)
            .map_err(|e| format!("Failed to read directory {}: {e}", current.display()))?;
        for entry in entries {
            let entry = entry.map_err(|e| format!("Directory entry error: {e}"))?;
            let path = entry.path();
            if path.is_dir() {
                visit(base, &path, files)?;
            } else {
                let name = path
                    .strip_prefix(base)
                    .map_err(|e| format!("Path prefix error: {e}"))?
                    .to_string_lossy()
                    .replace('\\', "/");

                if is_text_file(&name) {
                    let content = std::fs::read_to_string(&path)
                        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
                    files.insert(name, content);
                } else if is_binary_file(&name) {
                    let bytes = std::fs::read(&path)
                        .map_err(|e| format!("Failed to read {}: {e}", path.display()))?;
                    let mime = mime_for_ext(&name);
                    let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
                    files.insert(name, format!("data:{mime};base64,{b64}"));
                }
                // Skip unknown file types
            }
        }
        Ok(())
    }

    visit(dir_path, dir_path, &mut files)?;

    if !files.contains_key("index.ts") {
        return Err("Plugin directory must contain an index.ts file".to_string());
    }

    Ok(files)
}

/// Extract files from a zip archive, auto-stripping a root folder prefix if present.
fn extract_zip_to_files_map(bytes: &[u8]) -> Result<HashMap<String, String>, String> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("Failed to open zip: {e}"))?;

    // Detect common root prefix (e.g. "plugin-v1.0/")
    // Collect all entry names first to avoid borrow issues
    let entry_names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|e| e.name().to_string()))
        .collect();

    let mut prefix = String::new();
    if let Some(first) = entry_names.first() {
        if first.ends_with('/') {
            let candidate = first.clone();
            if entry_names[1..].iter().all(|n| n.starts_with(&candidate)) {
                prefix = candidate;
            }
        }
    }

    let mut files = HashMap::new();

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("Zip error: {e}"))?;
        if entry.is_dir() {
            continue;
        }

        let raw_name = entry.name().to_string();
        let name = if !prefix.is_empty() && raw_name.starts_with(&prefix) {
            raw_name[prefix.len()..].to_string()
        } else {
            raw_name
        };

        if name.is_empty() {
            continue;
        }

        if is_text_file(&name) {
            let mut content = String::new();
            entry
                .read_to_string(&mut content)
                .map_err(|e| format!("Failed to read zip entry {name}: {e}"))?;
            files.insert(name, content);
        } else if is_binary_file(&name) {
            let mut bytes = Vec::new();
            entry
                .read_to_end(&mut bytes)
                .map_err(|e| format!("Failed to read zip entry {name}: {e}"))?;
            let mime = mime_for_ext(&name);
            let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
            files.insert(name, format!("data:{mime};base64,{b64}"));
        }
    }

    if !files.contains_key("index.ts") {
        return Err("Zip archive must contain an index.ts file".to_string());
    }

    Ok(files)
}

/// Run VendorRuntime::read_metadata_from_files in a thread that owns a LocalSet.
/// deno_core JsRuntime uses Rc internally and is !Send.
async fn read_metadata_in_thread(
    files: HashMap<String, String>,
) -> Result<nisaba_vendor_runtime::PluginMetadata, String> {
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("Runtime error: {e}"))?;
        rt.block_on(VendorRuntime::read_metadata_from_files(&files))
            .map_err(|e| format!("Failed to read plugin metadata: {e}"))
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// Run VendorRuntime::execute_plugin_from_files in a thread that owns a LocalSet.
async fn execute_plugin_in_thread(
    files: HashMap<String, String>,
    config: HashMap<String, String>,
    batch_callback: Option<BatchCallback>,
) -> Result<Vec<nisaba_vendor_runtime::VendorListing>, String> {
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("Runtime error: {e}"))?;
        rt.block_on(VendorRuntime::execute_plugin_from_files(
            &files,
            config,
            batch_callback,
        ))
        .map_err(|e| format!("Plugin execution failed: {e}"))
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

fn build_plugin_info(p: VendorPluginRow, installed: bool, enabled: bool) -> VendorPluginInfo {
    let config_fields: Vec<serde_json::Value> = p
        .config_fields
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    let (network_access, allowed_hosts) = match p
        .allowed_hosts_json
        .as_deref()
        .map(serde_json::from_str::<Option<Vec<String>>>)
    {
        Some(Ok(Some(hosts))) => ("restricted", hosts),
        Some(Ok(None)) => ("unrestricted", Vec::new()),
        _ => ("unknown", Vec::new()),
    };

    VendorPluginInfo {
        id: p.id,
        plugin_file: p.plugin_file,
        display_name: p.display_name,
        description: p.description,
        version: p.version,
        config_fields,
        status: p.status,
        submitted_by: p.submitted_by,
        approved_by: p.approved_by,
        has_config: p.config_json.is_some(),
        installed,
        enabled,
        category: p.category,
        icon: p.icon,
        include_vendor_stock: p.include_vendor_stock,
        network_access: network_access.to_string(),
        allowed_hosts,
    }
}

/// The value cached in `VendorPluginRow::allowed_hosts_json` for this metadata.
fn allowed_hosts_json(metadata: &nisaba_vendor_runtime::PluginMetadata) -> Option<String> {
    serde_json::to_string(&metadata.allowed_hosts).ok()
}

/// Parse a files_json string into a HashMap.
fn parse_files_json(files_json: &str) -> Result<HashMap<String, String>, String> {
    if files_json.is_empty() {
        return Err("Plugin has no files".to_string());
    }
    serde_json::from_str(files_json).map_err(|e| format!("Failed to parse plugin files: {e}"))
}

// ── Registry commands (company-level, syncs via P2P) ──────────

#[tauri::command]
pub async fn list_registry_plugins(
    state: State<'_, AppState>,
) -> Result<Vec<VendorPluginInfo>, String> {
    let db = state.active_db().await?;
    let plugins = db
        .list_registry_plugins()
        .await
        .map_err(|e| e.to_string())?;
    let installs = db.list_installed_plugins().await.unwrap_or_default();

    let install_map: HashMap<String, (bool, bool)> = installs
        .into_iter()
        .map(|i| (i.plugin_id, (i.installed, i.enabled)))
        .collect();

    let infos = plugins
        .into_iter()
        .map(|p| {
            let (installed, enabled) = install_map.get(&p.id).copied().unwrap_or((false, false));
            build_plugin_info(p, installed, enabled)
        })
        .collect();

    Ok(infos)
}

#[tauri::command]
pub async fn submit_vendor_plugin(
    state: State<'_, AppState>,
    url: String,
) -> Result<VendorPluginInfo, String> {
    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| format!("Failed to download plugin: {e}"))?;

    // Detect zip vs single file from content-type or URL extension
    let content_type = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let is_zip = content_type.contains("zip") || url.ends_with(".zip");

    let files = if is_zip {
        let bytes = resp
            .bytes()
            .await
            .map_err(|e| format!("Failed to read plugin body: {e}"))?;
        extract_zip_to_files_map(&bytes)?
    } else {
        let code = resp
            .text()
            .await
            .map_err(|e| format!("Failed to read plugin body: {e}"))?;
        let mut map = HashMap::new();
        map.insert("index.ts".to_string(), code);
        map
    };

    let metadata = read_metadata_in_thread(files.clone()).await?;
    let allowed_hosts_json = allowed_hosts_json(&metadata);

    // Extract icon from files or use metadata icon
    let icon = extract_icon(&files).or_else(|| metadata.icon.clone());

    let db = state.active_db().await?;

    let submitted_by = db
        .get_company()
        .await
        .ok()
        .flatten()
        .and_then(|(_, _, _, onion, _)| onion);

    let file_name = url.rsplit('/').next().unwrap_or("plugin.ts").to_string();
    let config_fields_json = serde_json::to_string(&metadata.config_fields).ok();
    let files_json = serde_json::to_string(&files).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    // Dedup: check for existing plugin with same name
    let existing = db
        .find_registry_plugin_by_name(&metadata.name)
        .await
        .map_err(|e| e.to_string())?;

    let is_update = existing.is_some();
    let row = if let Some(existing) = existing {
        if existing.version == metadata.version {
            return Err(format!(
                "Plugin '{}' is already imported at version {}",
                metadata.name, metadata.version
            ));
        }
        // Update in-place: preserve id, config, approval status
        VendorPluginRow {
            id: existing.id,
            plugin_file: file_name,
            display_name: metadata.name,
            description: metadata.description,
            files_json,
            version: metadata.version,
            config_fields: config_fields_json,
            config_json: existing.config_json,
            status: existing.status,
            submitted_by,
            approved_by: existing.approved_by,
            category: metadata.category,
            icon,
            include_vendor_stock: existing.include_vendor_stock,
            created_at: existing.created_at,
            updated_at: now,
            allowed_hosts_json: allowed_hosts_json.clone(),
        }
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        VendorPluginRow {
            id,
            plugin_file: file_name,
            display_name: metadata.name,
            description: metadata.description,
            files_json,
            version: metadata.version,
            config_fields: config_fields_json,
            config_json: None,
            status: "pending".to_string(),
            submitted_by,
            approved_by: None,
            category: metadata.category,
            icon,
            include_vendor_stock: false,
            created_at: now.clone(),
            updated_at: now,
            allowed_hosts_json,
        }
    };

    db.upsert_registry_plugin(&row)
        .await
        .map_err(|e| e.to_string())?;

    // Clear stale cache and auto-sync when plugin version changes
    if is_update {
        let _ = db.clear_vendor_listings_cache(&row.id).await;
        let db_sync = db.clone();
        let plugin_id = row.id.clone();
        tokio::spawn(async move {
            run_vendor_sync_single(db_sync, plugin_id, None).await;
        });
    }

    Ok(build_plugin_info(row, false, false))
}

#[tauri::command]
pub async fn import_vendor_plugin(
    state: State<'_, AppState>,
    path: String,
) -> Result<VendorPluginInfo, String> {
    let p = Path::new(&path);

    let files = if p.is_dir() {
        // Directory import
        read_directory_to_files_map(p)?
    } else {
        // Single file import
        let code = tokio::fs::read_to_string(&path)
            .await
            .map_err(|e| format!("Failed to read plugin file: {e}"))?;
        let mut map = HashMap::new();
        map.insert("index.ts".to_string(), code);
        map
    };

    let metadata = read_metadata_in_thread(files.clone()).await?;
    let allowed_hosts_json = allowed_hosts_json(&metadata);

    // Extract icon from files or use metadata icon
    let icon = extract_icon(&files).or_else(|| metadata.icon.clone());

    let db = state.active_db().await?;

    let submitted_by = db
        .get_company()
        .await
        .ok()
        .flatten()
        .and_then(|(_, _, _, onion, _)| onion);

    let file_name = p
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("plugin")
        .to_string();
    let config_fields_json = serde_json::to_string(&metadata.config_fields).ok();
    let files_json = serde_json::to_string(&files).map_err(|e| e.to_string())?;
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    // Dedup: check for existing plugin with same name
    let existing = db
        .find_registry_plugin_by_name(&metadata.name)
        .await
        .map_err(|e| e.to_string())?;

    let is_update = existing.is_some();
    let row = if let Some(existing) = existing {
        if existing.version == metadata.version {
            return Err(format!(
                "Plugin '{}' is already imported at version {}",
                metadata.name, metadata.version
            ));
        }
        // Update in-place: preserve id, config, approval status
        VendorPluginRow {
            id: existing.id,
            plugin_file: file_name,
            display_name: metadata.name,
            description: metadata.description,
            files_json,
            version: metadata.version,
            config_fields: config_fields_json,
            config_json: existing.config_json,
            status: existing.status,
            submitted_by,
            approved_by: existing.approved_by,
            category: metadata.category,
            icon,
            include_vendor_stock: existing.include_vendor_stock,
            created_at: existing.created_at,
            updated_at: now,
            allowed_hosts_json: allowed_hosts_json.clone(),
        }
    } else {
        let id = uuid::Uuid::new_v4().to_string();
        VendorPluginRow {
            id,
            plugin_file: file_name,
            display_name: metadata.name,
            description: metadata.description,
            files_json,
            version: metadata.version,
            config_fields: config_fields_json,
            config_json: None,
            status: "pending".to_string(),
            submitted_by,
            approved_by: None,
            category: metadata.category,
            icon,
            include_vendor_stock: false,
            created_at: now.clone(),
            updated_at: now,
            allowed_hosts_json,
        }
    };

    db.upsert_registry_plugin(&row)
        .await
        .map_err(|e| e.to_string())?;

    // Clear stale cache and auto-sync when plugin version changes
    if is_update {
        let _ = db.clear_vendor_listings_cache(&row.id).await;
        let db_sync = db.clone();
        let plugin_id = row.id.clone();
        tokio::spawn(async move {
            run_vendor_sync_single(db_sync, plugin_id, None).await;
        });
    }

    Ok(build_plugin_info(row, false, false))
}

#[tauri::command]
pub async fn approve_vendor_plugin(
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<(), String> {
    let role = state.active_role().await?;
    if role != "admin" {
        return Err("Only admins can approve plugins".to_string());
    }

    let db = state.active_db().await?;
    let our_onion = db
        .get_company()
        .await
        .ok()
        .flatten()
        .and_then(|(_, _, _, onion, _)| onion);

    db.set_plugin_status(&plugin_id, "approved", our_onion.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn reject_vendor_plugin(
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<(), String> {
    let role = state.active_role().await?;
    if role != "admin" {
        return Err("Only admins can reject plugins".to_string());
    }

    let db = state.active_db().await?;
    let our_onion = db
        .get_company()
        .await
        .ok()
        .flatten()
        .and_then(|(_, _, _, onion, _)| onion);

    db.set_plugin_status(&plugin_id, "rejected", our_onion.as_deref())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_vendor_plugin_config(
    state: State<'_, AppState>,
    plugin_id: String,
    config: HashMap<String, String>,
) -> Result<(), String> {
    let role = state.active_role().await?;
    if role != "admin" {
        return Err("Only admins can set plugin credentials".to_string());
    }

    let db = state.active_db().await?;
    let config_json = serde_json::to_string(&config).map_err(|e| e.to_string())?;
    db.set_plugin_config(&plugin_id, &config_json)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn set_vendor_include_stock(
    state: State<'_, AppState>,
    plugin_id: String,
    include: bool,
) -> Result<(), String> {
    let role = state.active_role().await?;
    if role != "admin" {
        return Err("Only admins can change dropship settings".to_string());
    }

    let db = state.active_db().await?;
    db.set_plugin_include_vendor_stock(&plugin_id, include)
        .await
        .map_err(|e| e.to_string())?;

    let updated = db
        .recalc_products_for_plugin(&plugin_id)
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!(
        plugin = %plugin_id,
        include,
        products_recalculated = updated,
        "Dropship toggle changed"
    );
    Ok(())
}

#[tauri::command]
pub async fn remove_registry_plugin(
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<(), String> {
    let role = state.active_role().await?;
    if role != "admin" {
        return Err("Only admins can remove plugins".to_string());
    }

    let db = state.active_db().await?;
    let _ = db.clear_vendor_listings_cache(&plugin_id).await;
    db.delete_registry_plugin(&plugin_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_vendor_plugin_config(
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<HashMap<String, String>, String> {
    let db = state.active_db().await?;
    let plugin = db
        .get_registry_plugin(&plugin_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("Plugin not found")?;

    let config: HashMap<String, String> = plugin
        .config_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    Ok(config)
}

// ── User install commands (local only) ────────────────────────

#[tauri::command]
pub async fn install_vendor_plugin(
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<(), String> {
    let db = state.active_db().await?;

    let plugin = db
        .get_registry_plugin(&plugin_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("Plugin not found")?;

    if plugin.status != "approved" {
        return Err("Can only install approved plugins".to_string());
    }

    // Recompute the network allowlist from the files being installed rather than trusting
    // the cached value, which may have arrived from a peer or predate these files. A
    // plugin whose metadata no longer loads is not installed.
    let files = parse_files_json(&plugin.files_json)?;
    let metadata = read_metadata_in_thread(files).await?;
    db.set_registry_plugin_allowed_hosts(&plugin_id, allowed_hosts_json(&metadata).as_deref())
        .await
        .map_err(|e| e.to_string())?;

    db.install_plugin(&plugin_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn uninstall_vendor_plugin(
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<(), String> {
    let db = state.active_db().await?;
    let _ = db.clear_vendor_listings_cache(&plugin_id).await;
    db.uninstall_plugin(&plugin_id)
        .await
        .map_err(|e| e.to_string())
}

// ── Execution ─────────────────────────────────────────────────

#[tauri::command]
pub async fn fetch_vendor_listings(
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<Vec<VendorListingResult>, String> {
    let db = state.active_db().await?;

    // Verify installed and enabled
    let installs = db
        .list_installed_plugins()
        .await
        .map_err(|e| e.to_string())?;
    if !installs
        .iter()
        .any(|i| i.plugin_id == plugin_id && i.installed && i.enabled)
    {
        return Err("Plugin not installed or not enabled".to_string());
    }

    let plugin = db
        .get_registry_plugin(&plugin_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("Plugin not found in registry")?;

    let config: HashMap<String, String> = plugin
        .config_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    let files = parse_files_json(&plugin.files_json)?;
    let result = execute_plugin_in_thread(files, config, None).await?;

    // Save to cache as side-effect
    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    let cache_rows: Vec<VendorListingCache> = result
        .iter()
        .map(|l| listing_to_cache(&plugin_id, l, &now))
        .collect();
    if let Err(e) = db.save_vendor_listings_cache(&plugin_id, &cache_rows).await {
        tracing::error!(plugin = %plugin_id, "Failed to save vendor listings cache: {e}");
    } else {
        tracing::info!(plugin = %plugin_id, count = cache_rows.len(), "Saved vendor listings cache");
    }

    let listings = result
        .into_iter()
        .map(|l| VendorListingResult {
            vendor_item_id: l.vendor_item_id,
            title: l.title,
            price: l.price,
            currency: l.currency,
            quantity: l.quantity,
            sku: l.sku,
            image_url: l.image_url,
            url: l.url,
            extras: l.extras,
            group_key: l.group_key,
            variant_attributes: l.variant_attributes,
        })
        .collect();

    Ok(listings)
}

// ── Vendor Cache Commands ─────────────────────────────────────

#[tauri::command]
pub async fn get_cached_vendor_listings(
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<Vec<VendorListingResult>, String> {
    let db = state.active_db().await?;
    let cached = db
        .get_vendor_listings_cache(&plugin_id)
        .await
        .map_err(|e| e.to_string())?;
    Ok(cached.into_iter().map(cache_to_result).collect())
}

#[tauri::command]
pub async fn sync_vendor_listings(
    app: tauri::AppHandle,
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<(), String> {
    // Check if already syncing
    {
        let mut syncing = state.vendor_syncing.lock().await;
        if syncing.contains(&plugin_id) {
            return Err("Sync already in progress for this plugin".to_string());
        }
        syncing.insert(plugin_id.clone());
    }

    let db = state.active_db().await?;

    // Verify installed and enabled
    let installs = db
        .list_installed_plugins()
        .await
        .map_err(|e| e.to_string())?;
    if !installs
        .iter()
        .any(|i| i.plugin_id == plugin_id && i.installed && i.enabled)
    {
        state.vendor_syncing.lock().await.remove(&plugin_id);
        return Err("Plugin not installed or not enabled".to_string());
    }

    let plugin = db
        .get_registry_plugin(&plugin_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| {
            // Clean up syncing state on error
            "Plugin not found in registry".to_string()
        })?;

    let config: HashMap<String, String> = plugin
        .config_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    let files = parse_files_json(&plugin.files_json)?;

    // Emit started event
    let _ = app.emit(
        "vendor-sync",
        VendorSyncEvent {
            plugin_id: plugin_id.clone(),
            status: "started".to_string(),
            listing_count: 0,
            message: "Sync started".to_string(),
        },
    );

    // Create batch callback to stream progressive results to the frontend
    let app_for_batch = app.clone();
    let pid_for_batch = plugin_id.clone();
    let batch_callback = BatchCallback(Box::new(move |json: String| {
        let _ = app_for_batch.emit(
            "vendor-sync-batch",
            serde_json::json!({
                "plugin_id": pid_for_batch,
                "listings_json": json,
            }),
        );
    }));

    let pid = plugin_id.clone();
    let app_handle = app.clone();
    let db_spawn = db.clone();
    tokio::spawn(async move {
        let result = execute_plugin_in_thread(files, config, Some(batch_callback)).await;

        match result {
            Ok(vendor_listings) => {
                let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
                let cache_rows: Vec<VendorListingCache> = vendor_listings
                    .iter()
                    .map(|l| listing_to_cache(&pid, l, &now))
                    .collect();
                let count = cache_rows.len();

                if let Err(e) = db_spawn.save_vendor_listings_cache(&pid, &cache_rows).await {
                    tracing::error!(plugin = %pid, "Failed to save vendor listings cache: {e}");
                    let _ = app_handle.emit(
                        "vendor-sync",
                        VendorSyncEvent {
                            plugin_id: pid.clone(),
                            status: "completed".to_string(),
                            listing_count: count,
                            message: format!("Synced {count} listings (cache save failed: {e})"),
                        },
                    );
                } else {
                    // Update permanent product_vendor_data with fresh values
                    match db_spawn
                        .update_product_vendor_data_from_cache(&pid, &cache_rows)
                        .await
                    {
                        Ok(n) if n > 0 => {
                            tracing::info!(plugin = %pid, updated = n, "Updated product vendor data from sync");
                            // Recalc product quantities to reflect updated vendor stock
                            match db_spawn.recalc_products_for_plugin(&pid).await {
                                Ok(r) if r > 0 => {
                                    tracing::info!(plugin = %pid, recalced = r, "Recalculated product quantities after vendor sync")
                                }
                                Ok(_) => {}
                                Err(e) => {
                                    tracing::warn!(plugin = %pid, "Failed to recalc products after vendor sync: {e}")
                                }
                            }
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(plugin = %pid, "Failed to update product vendor data: {e}")
                        }
                    }

                    tracing::info!(plugin = %pid, count, "Saved vendor listings cache");
                    let _ = app_handle.emit(
                        "vendor-sync",
                        VendorSyncEvent {
                            plugin_id: pid.clone(),
                            status: "completed".to_string(),
                            listing_count: count,
                            message: format!("Synced {count} listings"),
                        },
                    );
                }
            }
            Err(e) => {
                let _ = app_handle.emit(
                    "vendor-sync",
                    VendorSyncEvent {
                        plugin_id: pid.clone(),
                        status: "failed".to_string(),
                        listing_count: 0,
                        message: e.clone(),
                    },
                );
                tracing::error!(plugin = %pid, "Vendor sync failed: {e}");
            }
        }

        app_handle
            .state::<AppState>()
            .vendor_syncing
            .lock()
            .await
            .remove(&pid);
    });

    Ok(())
}

#[tauri::command]
pub async fn get_vendor_sync_status(
    state: State<'_, AppState>,
    plugin_id: String,
) -> Result<VendorSyncStatus, String> {
    let syncing = state.vendor_syncing.lock().await.contains(&plugin_id);
    let db = state.active_db().await?;
    let meta = db
        .get_vendor_cache_meta(&plugin_id)
        .await
        .map_err(|e| e.to_string())?;

    Ok(VendorSyncStatus {
        syncing,
        last_synced_at: meta.as_ref().map(|(ts, _)| ts.clone()),
        cached_count: meta.map(|(_, c)| c).unwrap_or(0),
    })
}

// ── Vendor Listings for Product ───────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct VendorListingForProduct {
    pub plugin_name: String,
    pub variant_id: Option<String>,
    pub vendor_item_id: String,
    pub title: String,
    pub price: Option<f64>,
    pub currency: Option<String>,
    pub quantity: Option<i64>,
    pub sku: Option<String>,
    pub image_url: Option<String>,
    pub url: Option<String>,
    pub extras: HashMap<String, String>,
    pub group_key: Option<String>,
    pub variant_attributes: HashMap<String, String>,
    pub fetched_at: String,
}

#[tauri::command]
pub async fn get_vendor_listings_for_product(
    state: State<'_, AppState>,
    product_id: String,
) -> Result<Vec<VendorListingForProduct>, String> {
    let db = state.active_db().await?;
    let results = db
        .get_vendor_listings_for_product(&product_id)
        .await
        .map_err(|e| e.to_string())?;

    tracing::info!(
        product_id = %product_id,
        count = results.len(),
        "get_vendor_listings_for_product"
    );

    Ok(results
        .into_iter()
        .map(|(plugin_name, variant_id, c)| VendorListingForProduct {
            plugin_name,
            variant_id,
            vendor_item_id: c.vendor_item_id,
            title: c.title,
            price: c.price,
            currency: c.currency,
            quantity: c.quantity,
            sku: c.sku,
            image_url: c.image_url,
            url: c.url,
            extras: c.extras,
            group_key: c.group_key,
            variant_attributes: c.variant_attributes,
            fetched_at: c.fetched_at,
        })
        .collect())
}

// ── Vendor Import ─────────────────────────────────────────────

#[derive(Debug, Clone, serde::Deserialize)]
pub struct VendorImportItem {
    pub vendor_item_id: String,
    pub group_key: Option<String>,
    pub title: String,
    pub sku: String,
    pub quantity: i64,
    pub price: Option<f64>,
    pub currency: Option<String>,
    pub image_url: Option<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub extras: HashMap<String, String>,
    pub variant_attributes: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ImportResult {
    pub products_created: usize,
    pub variants_created: usize,
}

#[tauri::command]
pub async fn import_vendor_listings(
    state: State<'_, AppState>,
    plugin_id: String,
    items: Vec<VendorImportItem>,
) -> Result<ImportResult, String> {
    let db = state.active_db().await?;
    tracing::info!(plugin = %plugin_id, items = items.len(), "Importing vendor listings");

    // Group items by group_key
    let mut groups: HashMap<String, Vec<&VendorImportItem>> = HashMap::new();
    let mut standalone: Vec<&VendorImportItem> = Vec::new();

    for item in &items {
        match &item.group_key {
            Some(key) if !key.is_empty() => {
                groups.entry(key.clone()).or_default().push(item);
            }
            _ => standalone.push(item),
        }
    }

    let mut products_created = 0usize;
    let mut variants_created = 0usize;

    // Create grouped products with variants
    for (group_key, group_items) in &groups {
        let product_id = uuid::Uuid::new_v4().to_string();
        let first = group_items[0];
        let total_qty: i64 = group_items.iter().map(|i| i.quantity).sum();

        let product = nisaba_core::types::Product {
            id: product_id.clone(),
            canonical_sku: group_key.clone(),
            name: first.title.clone(),
            quantity: total_qty,
            low_stock_threshold: None,
            is_tracked: true,
            has_variants: true,
            created_at: String::new(),
            updated_at: String::new(),
        };
        db.insert_product(&product)
            .await
            .map_err(|e| e.to_string())?;
        products_created += 1;

        for (i, item) in group_items.iter().enumerate() {
            let variant_id = uuid::Uuid::new_v4().to_string();
            let variant = nisaba_core::types::ProductVariant {
                id: variant_id.clone(),
                product_id: product_id.clone(),
                sku: item.sku.clone(),
                name: item.vendor_item_id.clone(),
                attributes: item.variant_attributes.clone(),
                quantity: item.quantity,
                on_hand_quantity: 0,
                image_url: item.image_url.clone(),
                sort_order: i as i32,
                source_plugin_id: Some(plugin_id.clone()),
                source_vendor_item_id: Some(item.vendor_item_id.clone()),
                created_at: String::new(),
                updated_at: String::new(),
            };
            db.insert_variant(&variant)
                .await
                .map_err(|e| e.to_string())?;

            // Persist full vendor listing data (price, url, extras, etc.)
            let listing = VendorListingCache {
                plugin_id: plugin_id.clone(),
                vendor_item_id: item.vendor_item_id.clone(),
                title: item.title.clone(),
                price: item.price,
                currency: item.currency.clone(),
                quantity: Some(item.quantity),
                sku: Some(item.sku.clone()),
                image_url: item.image_url.clone(),
                url: item.url.clone(),
                extras: item.extras.clone(),
                group_key: item.group_key.clone(),
                variant_attributes: item.variant_attributes.clone(),
                fetched_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
            };
            db.save_product_vendor_data(&product_id, &variant_id, &plugin_id, &listing)
                .await
                .map_err(|e| e.to_string())?;

            variants_created += 1;
        }

        // Recalc product quantity (applies dropship logic if enabled)
        db.recalc_product_quantity(&product_id)
            .await
            .map_err(|e| e.to_string())?;
    }

    // Create standalone products (always with a default variant)
    for item in &standalone {
        let product_id = uuid::Uuid::new_v4().to_string();
        tracing::debug!(
            vendor_item_id = %item.vendor_item_id,
            price = ?item.price,
            "Creating standalone product"
        );
        let product = nisaba_core::types::Product {
            id: product_id.clone(),
            canonical_sku: item.sku.clone(),
            name: item.title.clone(),
            quantity: item.quantity,
            low_stock_threshold: None,
            is_tracked: true,
            has_variants: true,
            created_at: String::new(),
            updated_at: String::new(),
        };
        db.insert_product(&product)
            .await
            .map_err(|e| e.to_string())?;
        products_created += 1;

        let variant_id = uuid::Uuid::new_v4().to_string();
        let variant = nisaba_core::types::ProductVariant {
            id: variant_id.clone(),
            product_id: product_id.clone(),
            sku: item.sku.clone(),
            name: item.vendor_item_id.clone(),
            attributes: item.variant_attributes.clone(),
            quantity: item.quantity,
            on_hand_quantity: 0,
            image_url: item.image_url.clone(),
            sort_order: 0,
            source_plugin_id: Some(plugin_id.clone()),
            source_vendor_item_id: Some(item.vendor_item_id.clone()),
            created_at: String::new(),
            updated_at: String::new(),
        };
        db.insert_variant(&variant)
            .await
            .map_err(|e| e.to_string())?;

        // Persist full vendor listing data (price, url, extras, etc.)
        let listing = VendorListingCache {
            plugin_id: plugin_id.clone(),
            vendor_item_id: item.vendor_item_id.clone(),
            title: item.title.clone(),
            price: item.price,
            currency: item.currency.clone(),
            quantity: Some(item.quantity),
            sku: Some(item.sku.clone()),
            image_url: item.image_url.clone(),
            url: item.url.clone(),
            extras: item.extras.clone(),
            group_key: None,
            variant_attributes: item.variant_attributes.clone(),
            fetched_at: chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        };
        db.save_product_vendor_data(&product_id, &variant_id, &plugin_id, &listing)
            .await
            .map_err(|e| e.to_string())?;

        // Recalc product quantity (applies dropship logic if enabled)
        db.recalc_product_quantity(&product_id)
            .await
            .map_err(|e| e.to_string())?;

        variants_created += 1;
    }

    tracing::info!(
        products = products_created,
        variants = variants_created,
        "Vendor import complete"
    );

    Ok(ImportResult {
        products_created,
        variants_created,
    })
}

// ── Automatic vendor sync (called by sync engine + plugin updates) ───

use std::sync::atomic::{AtomicBool, Ordering};

/// Guard to prevent overlapping auto vendor syncs.
static AUTO_VENDOR_SYNC_RUNNING: AtomicBool = AtomicBool::new(false);

/// Run a sync for a single vendor plugin: execute → cache → update product data → recalc.
pub async fn run_vendor_sync_single(db: Arc<Db>, plugin_id: String, app: Option<tauri::AppHandle>) {
    let plugin = match db.get_registry_plugin(&plugin_id).await {
        Ok(Some(p)) => p,
        Ok(None) => {
            tracing::warn!(plugin = %plugin_id, "Auto vendor sync: plugin not found");
            return;
        }
        Err(e) => {
            tracing::warn!(plugin = %plugin_id, error = %e, "Auto vendor sync: failed to load plugin");
            return;
        }
    };

    let config: HashMap<String, String> = plugin
        .config_json
        .as_deref()
        .and_then(|s| serde_json::from_str(s).ok())
        .unwrap_or_default();

    let files = match parse_files_json(&plugin.files_json) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(plugin = %plugin_id, "Auto vendor sync: {e}");
            return;
        }
    };

    tracing::info!(plugin = %plugin.display_name, version = %plugin.version, "Auto vendor sync starting");

    match execute_plugin_in_thread(files, config, None).await {
        Ok(vendor_listings) => {
            let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
            let cache_rows: Vec<VendorListingCache> = vendor_listings
                .iter()
                .map(|l| listing_to_cache(&plugin_id, l, &now))
                .collect();
            let count = cache_rows.len();

            if let Err(e) = db.save_vendor_listings_cache(&plugin_id, &cache_rows).await {
                tracing::error!(plugin = %plugin_id, "Auto vendor sync: cache save failed: {e}");
                return;
            }

            match db
                .update_product_vendor_data_from_cache(&plugin_id, &cache_rows)
                .await
            {
                Ok(n) if n > 0 => {
                    tracing::info!(plugin = %plugin_id, updated = n, "Auto vendor sync: updated product vendor data");
                    match db.recalc_products_for_plugin(&plugin_id).await {
                        Ok(r) if r > 0 => {
                            tracing::info!(plugin = %plugin_id, recalced = r, "Auto vendor sync: recalculated quantities")
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(plugin = %plugin_id, "Auto vendor sync: recalc failed: {e}")
                        }
                    }
                }
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!(plugin = %plugin_id, "Auto vendor sync: update product data failed: {e}")
                }
            }

            tracing::info!(plugin = %plugin.display_name, count, "Auto vendor sync complete");

            // Notify frontend if app handle is available
            if let Some(ref app) = app {
                let _ = app.emit(
                    "vendor-sync",
                    VendorSyncEvent {
                        plugin_id: plugin_id.clone(),
                        status: "completed".to_string(),
                        listing_count: count,
                        message: format!("Auto-synced {count} listings"),
                    },
                );
            }
        }
        Err(e) => {
            tracing::error!(plugin = %plugin.display_name, "Auto vendor sync failed: {e}");
            if let Some(ref app) = app {
                let _ = app.emit(
                    "vendor-sync",
                    VendorSyncEvent {
                        plugin_id: plugin_id.clone(),
                        status: "failed".to_string(),
                        listing_count: 0,
                        message: format!("Auto-sync failed: {e}"),
                    },
                );
            }
        }
    }
}

/// Run vendor sync for ALL installed+enabled plugins.
/// Skips if another auto-sync is already running.
pub async fn run_vendor_sync_all(db: Arc<Db>, app: Option<tauri::AppHandle>) {
    // Prevent overlapping runs — if already syncing, skip silently
    if AUTO_VENDOR_SYNC_RUNNING
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        tracing::debug!("Auto vendor sync: skipping, already running");
        return;
    }

    // Ensure flag is cleared on exit (even on early return / panic)
    struct ResetOnDrop;
    impl Drop for ResetOnDrop {
        fn drop(&mut self) {
            AUTO_VENDOR_SYNC_RUNNING.store(false, Ordering::SeqCst);
        }
    }
    let _guard = ResetOnDrop;

    let installs = match db.list_installed_plugins().await {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!("Auto vendor sync: failed to list plugins: {e}");
            return;
        }
    };

    let active: Vec<_> = installs
        .into_iter()
        .filter(|i| i.installed && i.enabled)
        .collect();

    if active.is_empty() {
        return;
    }

    tracing::info!(
        count = active.len(),
        "Auto vendor sync: refreshing all plugins"
    );

    for install in active {
        run_vendor_sync_single(db.clone(), install.plugin_id, app.clone()).await;
    }
}
