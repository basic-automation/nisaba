use std::collections::HashMap;
use std::future::Future;
use std::io::Read as _;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;

use base64::Engine as _;
use nisaba_core::crypto::OsKeyringPluginSecrets;
use nisaba_core::db::Db;
use nisaba_core::types::{decimal_money, VendorListingCache, VendorPluginRow};
use nisaba_vendor_runtime::{
    BatchCallback, FetchListingsArgs, FetchListingsResult, PluginLimits, PluginStore, StoreError,
    VendorRuntime,
};
use rust_decimal::Decimal;
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
    /// This machine's time limit for one run, in minutes, and whether it is the default.
    pub timeout_minutes: u32,
    pub timeout_is_default: bool,
    /// SHA-256 fingerprint of the plugin's code and fetch allowlist, bound at the last
    /// review (see `nisaba_core::plugins`). A re-import or P2P merge that changed the
    /// code resets the plugin to pending and stores the new hash here, so the review
    /// card shows exactly what the admin is being asked to approve.
    pub code_hash: Option<String>,
    /// This machine's per-plugin auto-sync interval in minutes (N-7.5); `None`
    /// means the plugin runs on every global sync cycle.
    pub sync_interval_minutes: Option<i64>,
}

/// One recorded plugin run, as shown in the run history on the plugin's card
/// (N-7.5).
#[derive(Debug, Clone, Serialize)]
pub struct PluginRunInfo {
    pub id: i64,
    pub plugin_id: String,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub status: String,
    pub listings_emitted: i64,
    pub error: Option<String>,
    pub log_lines: Vec<String>,
    pub trigger_kind: String,
}

impl From<nisaba_core::types::PluginRunRecord> for PluginRunInfo {
    fn from(r: nisaba_core::types::PluginRunRecord) -> Self {
        Self {
            id: r.id,
            plugin_id: r.plugin_id,
            started_at: r.started_at,
            finished_at: r.finished_at,
            status: r.status,
            listings_emitted: r.listings_emitted,
            error: r.error,
            log_lines: r.log_lines,
            trigger_kind: r.trigger_kind,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct VendorListingResult {
    pub vendor_item_id: String,
    pub title: String,
    /// Exact decimal; serializes as a JSON number so the frontend contract
    /// (`price: number | null`) is unchanged (N-6.1).
    #[serde(with = "decimal_money::option")]
    pub price: Option<Decimal>,
    pub currency: Option<String>,
    /// `null` = unknown vendor stock, `0` = out of stock (N-7.2).
    pub quantity: Option<i64>,
    pub sku: Option<String>,
    /// All image URLs, most representative first (N-7.2).
    pub images: Vec<String>,
    /// First image — the pre-freeze single-URL shape, kept so the frontend's
    /// `image_url` keeps working unchanged.
    pub image_url: Option<String>,
    pub url: Option<String>,
    /// UPC / GTIN / EAN (string so leading zeros survive).
    pub upc: Option<String>,
    pub brand: Option<String>,
    /// Manufacturer part number.
    pub mpn: Option<String>,
    pub weight: Option<nisaba_core::types::Weight>,
    pub dimensions: Option<nisaba_core::types::Dimensions>,
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

/// Convert a plugin listing into a DB cache row.
///
/// N-7.2 enforcement: `currency` is required whenever a price is present — a
/// listing with a price but no currency is refused (the caller skips it with
/// a warning, so one bad listing cannot fail a whole catalog sync). The
/// `quantity` null-vs-0 distinction passes through untouched: `None` stays
/// unknown, it is never coerced to 0 here.
fn listing_to_cache(
    plugin_id: &str,
    l: &nisaba_vendor_runtime::VendorListing,
    fetched_at: &str,
) -> Result<VendorListingCache, String> {
    if l.price.is_some() && l.currency.as_deref().is_none_or(str::is_empty) {
        return Err(format!(
            "listing '{}' has a price but no currency",
            l.vendor_item_id
        ));
    }
    Ok(VendorListingCache {
        plugin_id: plugin_id.to_string(),
        vendor_item_id: l.vendor_item_id.clone(),
        title: l.title.clone(),
        // N-7.2: the plugin price is already an exact decimal; no f64 remains
        // on this path.
        price: l.price,
        currency: l.currency.clone(),
        quantity: l.quantity,
        sku: l.sku.clone(),
        images: l.images.clone(),
        // Legacy column, kept NULL on new rows (N-7.2).
        image_url: None,
        url: l.url.clone(),
        upc: l.upc.clone(),
        brand: l.brand.clone(),
        mpn: l.mpn.clone(),
        weight: l.weight.clone(),
        dimensions: l.dimensions.clone(),
        extras: l.extras.clone(),
        group_key: l.group_key.clone(),
        variant_attributes: l.variant_attributes.clone(),
        fetched_at: fetched_at.to_string(),
    })
}

/// Convert plugin listings to cache rows, skipping the ones
/// [`listing_to_cache`] refuses (with a warning each) instead of failing the
/// whole sync on one malformed listing.
fn cache_rows_or_skip(
    plugin_id: &str,
    listings: &[nisaba_vendor_runtime::VendorListing],
    fetched_at: &str,
) -> Vec<VendorListingCache> {
    let mut out = Vec::with_capacity(listings.len());
    for l in listings {
        match listing_to_cache(plugin_id, l, fetched_at) {
            Ok(c) => out.push(c),
            Err(e) => tracing::warn!(
                plugin = %plugin_id,
                vendor_item_id = %l.vendor_item_id,
                "{e}; skipping listing"
            ),
        }
    }
    out
}

/// A cached listing as a frontend result.
fn cache_to_result(c: VendorListingCache) -> VendorListingResult {
    let image_url = c.primary_image().map(str::to_string);
    let images = c.images_resolved();
    VendorListingResult {
        vendor_item_id: c.vendor_item_id,
        title: c.title,
        price: c.price,
        currency: c.currency,
        quantity: c.quantity,
        sku: c.sku,
        image_url,
        images,
        url: c.url,
        upc: c.upc,
        brand: c.brand,
        mpn: c.mpn,
        weight: c.weight,
        dimensions: c.dimensions,
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

/// Caps on what a plugin zip may unpack to. A plugin zip is untrusted and is unpacked
/// into memory before anything else looks at it, so a few kilobytes of zip bomb would
/// otherwise take the app down at import time.
struct ZipLimits {
    max_entries: usize,
    max_unpacked_bytes: u64,
}

const PLUGIN_ZIP_LIMITS: ZipLimits = ZipLimits {
    max_entries: 1_000,
    max_unpacked_bytes: 64 * 1024 * 1024,
};

/// The largest plugin `submit_vendor_plugin` will download.
const MAX_PLUGIN_DOWNLOAD_BYTES: usize = 64 * 1024 * 1024;

/// Extract files from a zip archive, auto-stripping a root folder prefix if present.
fn extract_zip_to_files_map(bytes: &[u8]) -> Result<HashMap<String, String>, String> {
    extract_zip_with_limits(bytes, &PLUGIN_ZIP_LIMITS)
}

fn extract_zip_with_limits(
    bytes: &[u8],
    limits: &ZipLimits,
) -> Result<HashMap<String, String>, String> {
    let cursor = std::io::Cursor::new(bytes);
    let mut archive =
        zip::ZipArchive::new(cursor).map_err(|e| format!("Failed to open zip: {e}"))?;
    if archive.len() > limits.max_entries {
        return Err(format!(
            "Plugin zip has {} entries; the limit is {}",
            archive.len(),
            limits.max_entries
        ));
    }

    // Detect common root prefix (e.g. "plugin-v1.0/")
    // Collect all entry names first to avoid borrow issues
    let entry_names: Vec<String> = (0..archive.len())
        .filter_map(|i| archive.by_index(i).ok().map(|e| e.name().to_string()))
        .collect();

    // Strip a single root folder shared by every entry. Derived from the names rather
    // than from a directory entry, which many zip tools do not write.
    let mut prefix = String::new();
    if let Some((root, _)) = entry_names.first().and_then(|n| n.split_once('/')) {
        let candidate = format!("{root}/");
        if !root.is_empty() && entry_names.iter().all(|n| n.starts_with(&candidate)) {
            prefix = candidate;
        }
    }

    let mut files = HashMap::new();
    let mut unpacked: u64 = 0;

    for i in 0..archive.len() {
        let mut entry = archive.by_index(i).map_err(|e| format!("Zip error: {e}"))?;
        if entry.is_dir() {
            continue;
        }
        // Absolute paths, `..` components ("zip slip") and backslashes, which are
        // separators on a Windows peer the plugin may be synced to. The plugin runtime
        // refuses these too; refusing here gives the user the reason at import.
        // (`ZipFile::enclosed_name` is not enough: it quietly turns `/abs` into `abs`.)
        let safe = !entry.name().contains('\\')
            && Path::new(entry.name())
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)));
        if !safe {
            return Err(format!(
                "Plugin zip entry {:?} has an unsafe path",
                entry.name()
            ));
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

        if !is_text_file(&name) && !is_binary_file(&name) {
            continue;
        }

        // Count what is actually decompressed — the sizes in the zip's headers are
        // attacker-supplied — and stop one byte past the budget.
        let budget = limits.max_unpacked_bytes - unpacked;
        let mut bytes = Vec::new();
        (&mut entry)
            .take(budget + 1)
            .read_to_end(&mut bytes)
            .map_err(|e| format!("Failed to read zip entry {name}: {e}"))?;
        if bytes.len() as u64 > budget {
            return Err(format!(
                "Plugin zip unpacks to more than {} MiB",
                limits.max_unpacked_bytes / (1024 * 1024)
            ));
        }
        unpacked += bytes.len() as u64;

        if is_text_file(&name) {
            let content = String::from_utf8(bytes)
                .map_err(|e| format!("Failed to read zip entry {name}: {e}"))?;
            files.insert(name, content);
        } else {
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

/// The longest time limit a user may give a plugin, in minutes.
const MAX_PLUGIN_TIMEOUT_MINUTES: u32 = 12 * 60;

/// The runtime limits for a plugin on this machine: the defaults, with the user's
/// per-install time limit applied if they set one.
async fn plugin_limits(db: &Db, plugin_id: &str) -> PluginLimits {
    let mut limits = PluginLimits::default();
    let custom = db
        .list_installed_plugins()
        .await
        .unwrap_or_default()
        .into_iter()
        .find(|i| i.plugin_id == plugin_id)
        .and_then(|i| i.timeout_minutes);
    if let Some(minutes) = custom {
        let minutes = minutes.clamp(1, MAX_PLUGIN_TIMEOUT_MINUTES as i64) as u64;
        limits.timeout = std::time::Duration::from_secs(minutes * 60);
    }
    limits
}

fn default_timeout_minutes() -> u32 {
    (PluginLimits::default().timeout.as_secs() / 60) as u32
}

/// [`PluginStore`] backed by the company database: the `plugin_kv_store` table
/// lives in the company DB, so the store behind `Nisaba.store` is scoped to
/// plugin + company by construction.
#[derive(Clone)]
struct DbPluginStore {
    db: Arc<Db>,
}

impl PluginStore for DbPluginStore {
    fn get(
        &self,
        plugin_id: &str,
        key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, StoreError>> + '_>> {
        let db = self.db.clone();
        let plugin_id = plugin_id.to_string();
        let key = key.to_string();
        Box::pin(async move {
            db.plugin_store_get(&plugin_id, &key)
                .await
                .map_err(|e| StoreError(e.to_string()))
        })
    }

    fn set(
        &self,
        plugin_id: &str,
        key: &str,
        value: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), StoreError>> + '_>> {
        let db = self.db.clone();
        let plugin_id = plugin_id.to_string();
        let key = key.to_string();
        let value = value.to_string();
        Box::pin(async move {
            db.plugin_store_set(&plugin_id, &key, &value)
                .await
                .map_err(|e| StoreError(e.to_string()))
        })
    }

    fn delete(
        &self,
        plugin_id: &str,
        key: &str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, StoreError>> + '_>> {
        let db = self.db.clone();
        let plugin_id = plugin_id.to_string();
        let key = key.to_string();
        Box::pin(async move {
            db.plugin_store_delete(&plugin_id, &key)
                .await
                .map_err(|e| StoreError(e.to_string()))
        })
    }

    fn total_bytes(
        &self,
        plugin_id: &str,
    ) -> Pin<Box<dyn Future<Output = Result<u64, StoreError>> + '_>> {
        let db = self.db.clone();
        let plugin_id = plugin_id.to_string();
        Box::pin(async move {
            db.plugin_store_total_bytes(&plugin_id)
                .await
                .map_err(|e| StoreError(e.to_string()))
        })
    }
}

/// Run VendorRuntime::execute_plugin_from_files_with_store_incremental in a thread
/// that owns a LocalSet.
///
/// `plugin_id` scopes the `Nisaba.store` key–value store; `store` is its
/// host-side backend (the company database in production). `args` carries the
/// host's incremental state (`since`, `cursor`) into the plugin's
/// `fetchListings(config, { since, cursor })`.
/// The outcome of one plugin execution: its result plus the plugin's own
/// log lines for the run (N-7.5), which survive a failed run.
struct PluginExecution {
    result: Result<FetchListingsResult, String>,
    logs: Vec<String>,
}

async fn execute_plugin_in_thread_incremental(
    files: HashMap<String, String>,
    config: HashMap<String, String>,
    batch_callback: Option<BatchCallback>,
    limits: PluginLimits,
    plugin_id: &str,
    store: Arc<dyn PluginStore>,
    args: &FetchListingsArgs,
) -> Result<PluginExecution, String> {
    let plugin_id = plugin_id.to_string();
    let args = args.clone();
    tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|e| format!("Runtime error: {e}"))?;
        let (result, logs) = rt.block_on(
            VendorRuntime::execute_plugin_from_files_with_store_incremental_logged(
                &files,
                &plugin_id,
                store,
                config,
                batch_callback,
                limits,
                &args,
            ),
        );
        Ok(PluginExecution {
            result: result.map_err(|e| format!("Plugin execution failed: {e}")),
            logs,
        })
    })
    .await
    .map_err(|e| format!("Task join error: {e}"))?
}

/// One vendor sync cycle: execute the plugin with the host's incremental args,
/// then fold the result into the cached catalog.
///
/// - A full catalog (plugins that ignore `{ since, cursor }`) replaces the
///   cache, as before — full-pull semantics are unchanged.
/// - A `{ changes, deleted, cursor }` delta is merged: changes are upserted by
///   `vendor_item_id`, deleted ids are removed, untouched rows keep their
///   stored `fetched_at`.
///
/// On success the host's sync state is recorded (`since` = this run's start,
/// `cursor` = the plugin's returned cursor, or none for a full pull), so the
/// next cycle can pull only what changed. Returns the merged catalog rows.
#[allow(clippy::too_many_arguments)]
async fn run_incremental_sync_cycle(
    db: &Db,
    plugin_id: &str,
    files: HashMap<String, String>,
    config: HashMap<String, String>,
    batch_callback: Option<BatchCallback>,
    limits: PluginLimits,
    store: Arc<dyn PluginStore>,
    trigger: &str,
) -> Result<Vec<VendorListingCache>, String> {
    // Run history (N-7.5): every run through this cycle is recorded. A
    // history-write failure never fails the sync itself.
    let run_id = match db.start_plugin_run(plugin_id, trigger).await {
        Ok(id) => Some(id),
        Err(e) => {
            tracing::warn!(plugin = %plugin_id, "Failed to record plugin run start: {e}");
            None
        }
    };
    let outcome = run_incremental_sync_cycle_inner(
        db,
        plugin_id,
        files,
        config,
        batch_callback,
        limits,
        store,
    )
    .await;
    match outcome {
        Ok((rows, emitted, logs)) => {
            record_run_finish(db, run_id, plugin_id, "completed", emitted, None, &logs).await;
            Ok(rows)
        }
        Err((e, logs)) => {
            record_run_finish(db, run_id, plugin_id, "failed", 0, Some(e.as_str()), &logs).await;
            Err(e)
        }
    }
}

/// Close a run-history row opened by [`run_incremental_sync_cycle`]; a
/// history-write failure is logged, never propagated.
async fn record_run_finish(
    db: &Db,
    run_id: Option<i64>,
    plugin_id: &str,
    status: &str,
    emitted: i64,
    error: Option<&str>,
    logs: &[String],
) {
    if let Some(run_id) = run_id {
        if let Err(e) = db
            .finish_plugin_run(run_id, plugin_id, status, emitted, error, logs)
            .await
        {
            tracing::warn!(plugin = %plugin_id, "Failed to record plugin run end: {e}");
        }
    }
}

/// The sync cycle itself, returning `(merged rows, listings emitted, log
/// lines)` on success and `(error, log lines)` on failure so the caller can
/// record the run either way.
#[allow(clippy::type_complexity)]
async fn run_incremental_sync_cycle_inner(
    db: &Db,
    plugin_id: &str,
    files: HashMap<String, String>,
    config: HashMap<String, String>,
    batch_callback: Option<BatchCallback>,
    limits: PluginLimits,
    store: Arc<dyn PluginStore>,
) -> Result<(Vec<VendorListingCache>, i64, Vec<String>), (String, Vec<String>)> {
    let (since, cursor) = db
        .get_vendor_sync_state(plugin_id)
        .await
        .map_err(|e| (e.to_string(), Vec::new()))?
        .map(|(s, c)| (Some(s), c))
        .unwrap_or((None, None));
    let args = FetchListingsArgs { since, cursor };

    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();

    let execution = execute_plugin_in_thread_incremental(
        files,
        config,
        batch_callback,
        limits,
        plugin_id,
        store,
        &args,
    )
    .await
    .map_err(|e| (e, Vec::new()))?;
    let logs = execution.logs;
    let result = execution.result.map_err(|e| (e, logs.clone()))?;

    let (rows, new_cursor, emitted) = match result {
        FetchListingsResult::Full(listings) => {
            let emitted = listings.len() as i64;
            let rows = cache_rows_or_skip(plugin_id, &listings, &now);
            db.save_vendor_listings_cache(plugin_id, &rows)
                .await
                .map_err(|e| (e.to_string(), logs.clone()))?;
            (rows, None, emitted)
        }
        FetchListingsResult::Delta(delta) => {
            let emitted = delta.changes.len() as i64;
            let changed = cache_rows_or_skip(plugin_id, &delta.changes, &now);
            db.merge_vendor_listings_cache(plugin_id, &changed, &delta.deleted)
                .await
                .map_err(|e| (e.to_string(), logs.clone()))?;
            let rows = db
                .get_vendor_listings_cache(plugin_id)
                .await
                .map_err(|e| (e.to_string(), logs.clone()))?;
            (rows, delta.cursor, emitted)
        }
    };

    // A state-save failure must not fail the sync: the catalog is already
    // merged, and the next run simply retries the same window.
    if let Err(e) = db
        .set_vendor_sync_state(plugin_id, &now, new_cursor.as_deref())
        .await
    {
        tracing::warn!(plugin = %plugin_id, "Failed to save incremental sync state: {e}");
    }

    Ok((rows, emitted, logs))
}

/// Load a plugin's effective config for a run: non-secret values from the
/// database plus `secret: true` values injected from the OS keyring.
///
/// Plaintext secrets left in `config_json` by older versions are migrated to
/// the keyring on this first load. Never returns an error: secret-store
/// failures degrade to the stored config so plugins keep running.
async fn plugin_config_for_run(db: &Db, plugin_id: &str) -> HashMap<String, String> {
    db.get_plugin_config_merged(plugin_id, &OsKeyringPluginSecrets)
        .await
        .unwrap_or_default()
}

fn build_plugin_info(p: VendorPluginRow, installed: bool, enabled: bool) -> VendorPluginInfo {
    build_plugin_info_with_timeout(p, installed, enabled, None, None)
}

fn build_plugin_info_with_timeout(
    p: VendorPluginRow,
    installed: bool,
    enabled: bool,
    timeout_minutes: Option<i64>,
    sync_interval_minutes: Option<i64>,
) -> VendorPluginInfo {
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
        timeout_minutes: timeout_minutes
            .map(|m| m.clamp(1, MAX_PLUGIN_TIMEOUT_MINUTES as i64) as u32)
            .unwrap_or_else(default_timeout_minutes),
        timeout_is_default: timeout_minutes.is_none(),
        code_hash: p.code_hash,
        sync_interval_minutes,
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

/// Reconcile a same-name re-import (or re-submission) against the existing registry
/// row: keep the stored review state when the code and fetch allowlist are unchanged;
/// otherwise reset to `pending` with the new code hash so the change is visible to
/// the reviewer. Returns `(status, approved_by, code_hash)`.
fn reconcile_reimport(
    existing: &VendorPluginRow,
    files_json: &str,
    allowed_hosts_json: &Option<String>,
) -> (String, Option<String>, Option<String>) {
    let (status, approved_by, code_hash) =
        nisaba_core::plugins::reconcile_plugin_approval(existing, files_json, allowed_hosts_json);
    if status == "pending" && existing.status != "pending" {
        tracing::warn!(
            plugin = %existing.display_name,
            "Plugin code or allowlist changed on re-import; approval revoked, pending re-approval"
        );
    }
    (status, approved_by, code_hash)
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

    let install_map: HashMap<String, (bool, bool, Option<i64>, Option<i64>)> = installs
        .into_iter()
        .map(|i| {
            (
                i.plugin_id,
                (
                    i.installed,
                    i.enabled,
                    i.timeout_minutes,
                    i.sync_interval_minutes,
                ),
            )
        })
        .collect();

    let infos = plugins
        .into_iter()
        .map(|p| {
            let (installed, enabled, timeout, interval) = install_map
                .get(&p.id)
                .copied()
                .unwrap_or((false, false, None, None));
            build_plugin_info_with_timeout(p, installed, enabled, timeout, interval)
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

    let mut resp = resp;
    let mut body = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| format!("Failed to read plugin body: {e}"))?
    {
        if body.len() + chunk.len() > MAX_PLUGIN_DOWNLOAD_BYTES {
            return Err(format!(
                "Plugin download exceeds {} MiB",
                MAX_PLUGIN_DOWNLOAD_BYTES / (1024 * 1024)
            ));
        }
        body.extend_from_slice(&chunk);
    }

    let files = if is_zip {
        extract_zip_to_files_map(&body)?
    } else {
        let code = String::from_utf8_lossy(&body).into_owned();
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
        // Update in-place: preserve id and config. Approval carries over only when
        // the code and allowlist are unchanged; any change resets to pending with
        // the new code hash surfaced for re-approval.
        let (status, approved_by, code_hash) =
            reconcile_reimport(&existing, &files_json, &allowed_hosts_json);
        VendorPluginRow {
            id: existing.id,
            plugin_file: file_name,
            display_name: metadata.name,
            description: metadata.description,
            files_json,
            version: metadata.version,
            config_fields: config_fields_json,
            config_json: existing.config_json,
            status,
            submitted_by,
            approved_by,
            category: metadata.category,
            icon,
            include_vendor_stock: existing.include_vendor_stock,
            created_at: existing.created_at,
            updated_at: now,
            allowed_hosts_json: allowed_hosts_json.clone(),
            code_hash,
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
            // Fresh import: never reviewed yet, so no bound code hash.
            code_hash: None,
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
        // Update in-place: preserve id and config. Approval carries over only when
        // the code and allowlist are unchanged; any change resets to pending with
        // the new code hash surfaced for re-approval.
        let (status, approved_by, code_hash) =
            reconcile_reimport(&existing, &files_json, &allowed_hosts_json);
        VendorPluginRow {
            id: existing.id,
            plugin_file: file_name,
            display_name: metadata.name,
            description: metadata.description,
            files_json,
            version: metadata.version,
            config_fields: config_fields_json,
            config_json: existing.config_json,
            status,
            submitted_by,
            approved_by,
            category: metadata.category,
            icon,
            include_vendor_stock: existing.include_vendor_stock,
            created_at: existing.created_at,
            updated_at: now,
            allowed_hosts_json: allowed_hosts_json.clone(),
            code_hash,
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
            // Fresh import: never reviewed yet, so no bound code hash.
            code_hash: None,
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

    db.review_registry_plugin(&plugin_id, "approved", our_onion.as_deref())
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

    db.review_registry_plugin(&plugin_id, "rejected", our_onion.as_deref())
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
    // `secret: true` values go to the OS keyring, never to config_json.
    db.set_plugin_config_split(&plugin_id, &config, &OsKeyringPluginSecrets)
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
    // Returns the effective config: `secret: true` values are injected from
    // the OS keyring so the config form can pre-fill them.
    db.get_plugin_config_merged(&plugin_id, &OsKeyringPluginSecrets)
        .await
        .map_err(|e| e.to_string())
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
    // A plugin built for a plugin API this host does not implement cannot run here:
    // refuse it at install with the reason, rather than failing at first sync.
    metadata.check_api_version()?;
    db.set_registry_plugin_allowed_hosts(&plugin_id, allowed_hosts_json(&metadata).as_deref())
        .await
        .map_err(|e| e.to_string())?;

    db.install_plugin(&plugin_id)
        .await
        .map_err(|e| e.to_string())
}

/// Set this machine's time limit for an installed plugin; `None` restores the default.
#[tauri::command]
pub async fn set_vendor_plugin_timeout(
    state: State<'_, AppState>,
    plugin_id: String,
    minutes: Option<u32>,
) -> Result<(), String> {
    if let Some(m) = minutes {
        if !(1..=MAX_PLUGIN_TIMEOUT_MINUTES).contains(&m) {
            return Err(format!(
                "A plugin time limit must be between 1 and {MAX_PLUGIN_TIMEOUT_MINUTES} minutes"
            ));
        }
    }
    let db = state.active_db().await?;
    db.set_plugin_timeout(&plugin_id, minutes.map(i64::from))
        .await
        .map_err(|e| e.to_string())
}

/// The longest per-plugin sync interval a user may set, in minutes (28 days).
const MAX_PLUGIN_SYNC_INTERVAL_MINUTES: u32 = 28 * 24 * 60;

/// Set this machine's per-plugin auto-sync interval for an installed plugin
/// (N-7.5); `None` restores "run on every global sync cycle". The interval
/// gates only the auto-sync path — manual runs (Sync / Fetch) always run.
#[tauri::command]
pub async fn set_vendor_plugin_sync_interval(
    state: State<'_, AppState>,
    plugin_id: String,
    minutes: Option<u32>,
) -> Result<(), String> {
    if let Some(m) = minutes {
        if !(1..=MAX_PLUGIN_SYNC_INTERVAL_MINUTES).contains(&m) {
            return Err(format!(
                "A plugin sync interval must be between 1 and {MAX_PLUGIN_SYNC_INTERVAL_MINUTES} minutes"
            ));
        }
    }
    let db = state.active_db().await?;
    db.set_plugin_sync_interval(&plugin_id, minutes.map(i64::from))
        .await
        .map_err(|e| e.to_string())
}

/// A plugin's run history (N-7.5), newest first: start/end, listings
/// emitted, the error on failure, and the plugin's own log lines.
#[tauri::command]
pub async fn get_plugin_run_history(
    state: State<'_, AppState>,
    plugin_id: String,
    limit: Option<u32>,
) -> Result<Vec<PluginRunInfo>, String> {
    let db = state.active_db().await?;
    let runs = db
        .list_plugin_runs(&plugin_id, i64::from(limit.unwrap_or(20).clamp(1, 100)))
        .await
        .map_err(|e| e.to_string())?;
    Ok(runs.into_iter().map(PluginRunInfo::from).collect())
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

    if plugin.status != "approved" {
        return Err("Plugin is not approved".to_string());
    }

    // Secrets are injected from the OS keyring here; config_json never holds them.
    let config = plugin_config_for_run(&db, &plugin_id).await;

    let files = parse_files_json(&plugin.files_json)?;
    let limits = plugin_limits(&db, &plugin_id).await;
    let store: Arc<dyn PluginStore> = Arc::new(DbPluginStore { db: db.clone() });

    // Runs the plugin with the host's incremental args and folds the result
    // into the cached catalog as a side effect (N-7.4).
    let cache_rows =
        run_incremental_sync_cycle(&db, &plugin_id, files, config, None, limits, store, "fetch")
            .await?;
    tracing::info!(plugin = %plugin_id, count = cache_rows.len(), "Fetched vendor listings");

    Ok(cache_rows.into_iter().map(cache_to_result).collect())
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

    if plugin.status != "approved" {
        state.vendor_syncing.lock().await.remove(&plugin_id);
        return Err("Plugin is not approved".to_string());
    }

    // Secrets are injected from the OS keyring here; config_json never holds them.
    let config = plugin_config_for_run(&db, &plugin_id).await;

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
    let limits = plugin_limits(&db, &plugin_id).await;
    let store: Arc<dyn PluginStore> = Arc::new(DbPluginStore { db: db.clone() });
    tokio::spawn(async move {
        let result = run_incremental_sync_cycle(
            &db_spawn,
            &pid,
            files,
            config,
            Some(batch_callback),
            limits,
            store,
            "manual",
        )
        .await;

        match result {
            Ok(cache_rows) => {
                let count = cache_rows.len();

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
    /// Exact decimal; serializes as a JSON number so the frontend contract
    /// (`price: number | null`) is unchanged (N-6.1).
    #[serde(with = "decimal_money::option")]
    pub price: Option<Decimal>,
    pub currency: Option<String>,
    pub quantity: Option<i64>,
    pub sku: Option<String>,
    /// All image URLs, most representative first (N-7.2).
    pub images: Vec<String>,
    /// First image — the pre-freeze single-URL shape, kept so the frontend's
    /// `image_url` keeps working unchanged.
    pub image_url: Option<String>,
    pub url: Option<String>,
    pub upc: Option<String>,
    pub brand: Option<String>,
    pub mpn: Option<String>,
    pub weight: Option<nisaba_core::types::Weight>,
    pub dimensions: Option<nisaba_core::types::Dimensions>,
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
        .map(|(plugin_name, variant_id, c)| {
            let image_url = c.primary_image().map(str::to_string);
            let images = c.images_resolved();
            VendorListingForProduct {
                plugin_name,
                variant_id,
                vendor_item_id: c.vendor_item_id,
                title: c.title,
                price: c.price,
                currency: c.currency,
                quantity: c.quantity,
                sku: c.sku,
                image_url,
                images,
                url: c.url,
                upc: c.upc,
                brand: c.brand,
                mpn: c.mpn,
                weight: c.weight,
                dimensions: c.dimensions,
                extras: c.extras,
                group_key: c.group_key,
                variant_attributes: c.variant_attributes,
                fetched_at: c.fetched_at,
            }
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
    /// Vendor stock on hand. `None` = unknown (the vendor gave no stock
    /// figure) — it must reach the vendor-data row as NULL, never be coerced
    /// to 0 (N-7.2). The concrete product/variant quantities use 0 for
    /// unknown: nothing confirmed in stock.
    #[serde(default)]
    pub quantity: Option<i64>,
    /// Exact decimal (N-6.1/N-7.2). The frontend sends a JSON number, which
    /// the decimal module accepts; `currency` is required with a price.
    #[serde(default, with = "decimal_money::option")]
    pub price: Option<Decimal>,
    pub currency: Option<String>,
    /// All image URLs, most representative first (N-7.2).
    #[serde(default)]
    pub images: Vec<String>,
    pub url: Option<String>,
    #[serde(default)]
    pub upc: Option<String>,
    #[serde(default)]
    pub brand: Option<String>,
    #[serde(default)]
    pub mpn: Option<String>,
    #[serde(default)]
    pub weight: Option<nisaba_core::types::Weight>,
    #[serde(default)]
    pub dimensions: Option<nisaba_core::types::Dimensions>,
    #[serde(default)]
    pub extras: HashMap<String, String>,
    #[serde(default)]
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

    // N-7.2: a price without a currency is refused at import, like at sync
    // time — the item is skipped with a warning, not imported half-formed.
    let mut valid_items: Vec<&VendorImportItem> = Vec::with_capacity(items.len());
    for item in &items {
        if item.price.is_some() && item.currency.as_deref().is_none_or(str::is_empty) {
            tracing::warn!(
                plugin = %plugin_id,
                vendor_item_id = %item.vendor_item_id,
                "import: item has a price but no currency; skipping"
            );
            continue;
        }
        valid_items.push(item);
    }

    // Group items by group_key
    let mut groups: HashMap<String, Vec<&VendorImportItem>> = HashMap::new();
    let mut standalone: Vec<&VendorImportItem> = Vec::new();

    for item in valid_items {
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
        // N-7.2: unknown vendor stock contributes nothing confirmed; the
        // vendor-data row below still records it as NULL.
        let total_qty: i64 = group_items.iter().map(|i| i.quantity.unwrap_or(0)).sum();

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
                quantity: item.quantity.unwrap_or(0),
                on_hand_quantity: 0,
                image_url: item.images.first().cloned(),
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
                // N-7.2: unknown vendor stock stays NULL here — never coerced
                // to 0. Only the concrete variant/product quantities use 0.
                quantity: item.quantity,
                sku: Some(item.sku.clone()),
                images: item.images.clone(),
                image_url: None,
                url: item.url.clone(),
                upc: item.upc.clone(),
                brand: item.brand.clone(),
                mpn: item.mpn.clone(),
                weight: item.weight.clone(),
                dimensions: item.dimensions.clone(),
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
            quantity: item.quantity.unwrap_or(0),
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
            quantity: item.quantity.unwrap_or(0),
            on_hand_quantity: 0,
            image_url: item.images.first().cloned(),
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
            // N-7.2: unknown vendor stock stays NULL here — never coerced to 0.
            quantity: item.quantity,
            sku: Some(item.sku.clone()),
            images: item.images.clone(),
            image_url: None,
            url: item.url.clone(),
            upc: item.upc.clone(),
            brand: item.brand.clone(),
            mpn: item.mpn.clone(),
            weight: item.weight.clone(),
            dimensions: item.dimensions.clone(),
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

/// Guard against overlapping auto vendor syncs, scoped **per company**
/// (N-7.5). The pre-N-7.5 process-wide `AUTO_VENDOR_SYNC_RUNNING` flag made
/// one company's sync skip every other company's for its whole duration,
/// logged only at debug. The guard is now keyed by the company database's
/// path — each company has its own `Db` (and its own sync engine) — so a
/// running sync blocks only another run for the *same* company.
static AUTO_VENDOR_SYNC_RUNNING_FOR: std::sync::OnceLock<
    std::sync::Mutex<std::collections::HashSet<String>>,
> = std::sync::OnceLock::new();

fn auto_sync_running_set() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    AUTO_VENDOR_SYNC_RUNNING_FOR.get_or_init(|| std::sync::Mutex::new(Default::default()))
}

/// RAII guard: dropping it releases the company's auto-sync slot, including
/// on early return or panic.
struct AutoSyncGuard {
    key: String,
}

impl Drop for AutoSyncGuard {
    fn drop(&mut self) {
        if let Ok(mut set) = auto_sync_running_set().lock() {
            set.remove(&self.key);
        }
    }
}

/// Claim the auto-sync slot for one company (identified by its database
/// path); `None` when that company already has a run in flight. Other
/// companies' slots are independent.
fn try_begin_auto_sync(company_key: &str) -> Option<AutoSyncGuard> {
    let mut set = auto_sync_running_set().lock().ok()?;
    if !set.insert(company_key.to_string()) {
        return None;
    }
    Some(AutoSyncGuard {
        key: company_key.to_string(),
    })
}

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

    // Never run a plugin that is not installed+enabled and approved. A code or
    // allowlist change resets the plugin to pending, which revokes its run
    // eligibility until an admin re-approves the new code.
    let installs = db.list_installed_plugins().await.unwrap_or_default();
    let install = installs.iter().find(|i| i.plugin_id == plugin_id);
    if !nisaba_core::plugins::plugin_run_allowed(&plugin, install) {
        tracing::warn!(
            plugin = %plugin_id,
            status = %plugin.status,
            "Auto vendor sync: refusing to run plugin that is not installed+approved"
        );
        if let Some(ref app) = app {
            let _ = app.emit(
                "vendor-sync",
                VendorSyncEvent {
                    plugin_id: plugin_id.clone(),
                    status: "skipped".to_string(),
                    listing_count: 0,
                    message: "Plugin is not installed and approved; sync skipped".to_string(),
                },
            );
        }
        return;
    }

    // Secrets are injected from the OS keyring here; config_json never holds them.
    let config = plugin_config_for_run(&db, &plugin_id).await;

    let files = match parse_files_json(&plugin.files_json) {
        Ok(f) => f,
        Err(e) => {
            tracing::warn!(plugin = %plugin_id, "Auto vendor sync: {e}");
            return;
        }
    };

    tracing::info!(plugin = %plugin.display_name, version = %plugin.version, "Auto vendor sync starting");

    let limits = plugin_limits(&db, &plugin_id).await;
    let store: Arc<dyn PluginStore> = Arc::new(DbPluginStore { db: db.clone() });
    match run_incremental_sync_cycle(&db, &plugin_id, files, config, None, limits, store, "auto")
        .await
    {
        Ok(cache_rows) => {
            let count = cache_rows.len();

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

/// Run vendor sync for ALL installed+enabled plugins in one company.
/// Skips only if *this company's* auto-sync is already running (N-7.5):
/// another company's run no longer blocks this one. Plugins with a
/// per-plugin interval are run only when that interval has elapsed since
/// their last recorded run; manual runs are never gated.
pub async fn run_vendor_sync_all(db: Arc<Db>, app: Option<tauri::AppHandle>) {
    let Some(_guard) = try_begin_auto_sync(db.path()) else {
        tracing::debug!(
            company_db = %db.path(),
            "Auto vendor sync: skipping, already running for this company"
        );
        return;
    };

    let installs = match db.list_installed_plugins().await {
        Ok(list) => list,
        Err(e) => {
            tracing::warn!("Auto vendor sync: failed to list plugins: {e}");
            return;
        }
    };

    // Only installed+enabled plugins with a current approval may run; a code or
    // allowlist change resets a plugin to pending, which keeps it out of the run.
    let approved: std::collections::HashSet<String> = db
        .list_registry_plugins()
        .await
        .unwrap_or_default()
        .into_iter()
        .filter(|p| p.status == "approved")
        .map(|p| p.id)
        .collect();
    let active: Vec<_> = installs
        .into_iter()
        .filter(|i| i.installed && i.enabled && approved.contains(&i.plugin_id))
        .collect();

    if active.is_empty() {
        return;
    }

    tracing::info!(
        count = active.len(),
        "Auto vendor sync: refreshing all plugins"
    );

    let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
    for install in active {
        // Per-plugin schedule (N-7.5): a plugin with its own interval runs
        // at most once per interval, measured from its last recorded run.
        if let Some(interval) = install.sync_interval_minutes {
            let last = db
                .last_plugin_run_started_at(&install.plugin_id)
                .await
                .ok()
                .flatten();
            if !nisaba_core::plugins::plugin_sync_due(Some(interval), last.as_deref(), &now) {
                tracing::info!(
                    plugin = %install.plugin_id,
                    interval_minutes = interval,
                    "Auto vendor sync: skipping plugin, its interval has not elapsed"
                );
                continue;
            }
        }
        run_vendor_sync_single(db.clone(), install.plugin_id, app.clone()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn zip_of(entries: &[(&str, &[u8])]) -> Vec<u8> {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, content) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(content).unwrap();
        }
        writer.finish().unwrap().into_inner()
    }

    const INDEX: &[u8] = b"export const metadata = { name: 'z', version: '1' };";

    // ── N-7.5: per-company auto-sync guard ──

    #[test]
    fn auto_sync_guard_is_scoped_per_company() {
        // One company's running sync blocks only that company, never others
        // (the pre-N-7.5 process-wide flag skipped every company).
        let a = try_begin_auto_sync("company-a.db").expect("first claim succeeds");
        assert!(try_begin_auto_sync("company-a.db").is_none());
        let b = try_begin_auto_sync("company-b.db").expect("another company is unaffected");
        // Dropping releases exactly that company's slot.
        drop(a);
        let a2 = try_begin_auto_sync("company-a.db").expect("released on drop");
        drop(a2);
        drop(b);
    }

    #[test]
    fn a_normal_plugin_zip_unpacks_with_its_root_folder_stripped() {
        let bytes = zip_of(&[
            ("plugin-1.0/index.ts", INDEX),
            ("plugin-1.0/lib/util.ts", b"export const x = 1;"),
            ("plugin-1.0/icon.png", b"\x89PNG"),
            ("plugin-1.0/notes.bin", b"ignored"),
        ]);
        let files = extract_zip_to_files_map(&bytes).unwrap();
        let mut names: Vec<&str> = files.keys().map(String::as_str).collect();
        names.sort();
        assert_eq!(names, ["icon.png", "index.ts", "lib/util.ts"]);
        assert!(files["icon.png"].starts_with("data:image/png;base64,"));
    }

    #[test]
    fn a_root_folder_is_only_stripped_when_every_entry_shares_it() {
        let bytes = zip_of(&[("index.ts", INDEX), ("lib/util.ts", b"x")]);
        let files = extract_zip_to_files_map(&bytes).unwrap();
        assert!(files.contains_key("lib/util.ts"));
    }

    #[test]
    fn entries_with_unsafe_paths_are_refused() {
        for name in [
            "../escape.ts",
            "/tmp/absolute.ts",
            "lib/../../escape.ts",
            "..\\escape.ts",
        ] {
            let bytes = zip_of(&[("index.ts", INDEX), (name, b"x")]);
            let err = extract_zip_to_files_map(&bytes).unwrap_err();
            assert!(err.contains("unsafe path"), "{name}: {err}");
        }
    }

    #[test]
    fn a_zip_bomb_is_refused_by_what_it_decompresses_to() {
        // 1 MiB of zeros deflates to about a kilobyte.
        let zeros = vec![0u8; 1024 * 1024];
        let bytes = zip_of(&[("index.ts", INDEX), ("bomb.ts", &zeros)]);
        assert!(bytes.len() < 16 * 1024, "fixture is {} bytes", bytes.len());

        let limits = ZipLimits {
            max_entries: 10,
            max_unpacked_bytes: 512 * 1024,
        };
        let err = extract_zip_with_limits(&bytes, &limits).unwrap_err();
        assert!(err.contains("unpacks to more than"), "{err}");

        // The same archive is fine under a budget it fits in.
        let roomy = ZipLimits {
            max_entries: 10,
            max_unpacked_bytes: 2 * 1024 * 1024,
        };
        assert_eq!(extract_zip_with_limits(&bytes, &roomy).unwrap().len(), 2);
    }

    #[test]
    fn a_zip_with_too_many_entries_is_refused() {
        let names: Vec<String> = (0..20).map(|i| format!("f{i}.ts")).collect();
        let mut entries: Vec<(&str, &[u8])> = vec![("index.ts", INDEX)];
        entries.extend(names.iter().map(|n| (n.as_str(), b"x" as &[u8])));
        let limits = ZipLimits {
            max_entries: 10,
            max_unpacked_bytes: 1024 * 1024,
        };
        let err = extract_zip_with_limits(&zip_of(&entries), &limits).unwrap_err();
        assert!(err.contains("entries"), "{err}");
    }

    // ── N-7.2: listing_to_cache ──

    fn cache_listing_json() -> serde_json::Value {
        serde_json::json!({
            "vendor_item_id": "VI-1",
            "title": "Widget",
            "price": "19.99",
            "currency": "USD",
            "quantity": null,
            "sku": "W-1",
            "images": ["https://cdn/1.jpg", "https://cdn/2.jpg"],
            "url": "https://vendor/1",
            "upc": "001234567890",
            "brand": "Acme",
            "mpn": "ACM-1",
            "weight": { "value": 1.5, "unit": "lb" },
            "dimensions": { "length": 12.0, "width": 9.0, "height": 4.0, "unit": "in" },
            "extras": {},
            "group_key": null,
            "variant_attributes": {}
        })
    }

    fn parse_listing(v: serde_json::Value) -> nisaba_vendor_runtime::VendorListing {
        serde_json::from_value(v).expect("test listing must parse")
    }

    #[test]
    fn listing_to_cache_keeps_the_new_schema_and_unknown_quantity() {
        let l = parse_listing(cache_listing_json());
        let c = listing_to_cache("plug", &l, "2026-10-21 00:00:00").unwrap();
        assert_eq!(c.price.map(|d| d.to_string()).as_deref(), Some("19.99"));
        assert_eq!(c.currency.as_deref(), Some("USD"));
        assert_eq!(c.quantity, None, "unknown must stay unknown, not become 0");
        assert_eq!(c.images, ["https://cdn/1.jpg", "https://cdn/2.jpg"]);
        assert_eq!(c.image_url, None, "legacy column stays NULL on new rows");
        assert_eq!(c.upc.as_deref(), Some("001234567890"));
        assert_eq!(c.brand.as_deref(), Some("Acme"));
        assert_eq!(c.mpn.as_deref(), Some("ACM-1"));
        let w = c.weight.unwrap();
        assert_eq!((w.value, w.unit.as_str()), (1.5, "lb"));
        let d = c.dimensions.unwrap();
        assert_eq!((d.length, d.unit.as_str()), (12.0, "in"));
    }

    #[test]
    fn listing_to_cache_refuses_a_price_without_currency() {
        let mut v = cache_listing_json();
        v["currency"] = serde_json::Value::Null;
        let l = parse_listing(v);
        let err = listing_to_cache("plug", &l, "2026-10-21 00:00:00").unwrap_err();
        assert!(err.contains("no currency"), "{err}");

        // …and an empty currency is the same as none.
        let mut v = cache_listing_json();
        v["currency"] = serde_json::json!("");
        let l = parse_listing(v);
        assert!(listing_to_cache("plug", &l, "2026-10-21 00:00:00").is_err());
    }

    #[test]
    fn listing_to_cache_accepts_no_price_at_all() {
        let mut v = cache_listing_json();
        v["price"] = serde_json::Value::Null;
        v["currency"] = serde_json::Value::Null;
        let l = parse_listing(v);
        let c = listing_to_cache("plug", &l, "2026-10-21 00:00:00").unwrap();
        assert_eq!(c.price, None);
    }

    #[test]
    fn cache_rows_or_skip_drops_only_the_bad_listing() {
        let good = parse_listing(cache_listing_json());
        let mut bad_json = cache_listing_json();
        bad_json["vendor_item_id"] = serde_json::json!("VI-2");
        bad_json["currency"] = serde_json::Value::Null;
        let bad = parse_listing(bad_json);
        let rows = cache_rows_or_skip("plug", &[good, bad], "2026-10-21 00:00:00");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].vendor_item_id, "VI-1");
    }

    #[test]
    fn cache_to_result_exposes_images_and_first_image_alias() {
        let l = parse_listing(cache_listing_json());
        let c = listing_to_cache("plug", &l, "2026-10-21 00:00:00").unwrap();
        let r = cache_to_result(c);
        assert_eq!(r.images, ["https://cdn/1.jpg", "https://cdn/2.jpg"]);
        assert_eq!(r.image_url.as_deref(), Some("https://cdn/1.jpg"));
        assert_eq!(r.upc.as_deref(), Some("001234567890"));
    }
}
