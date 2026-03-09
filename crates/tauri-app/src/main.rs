// Prevents additional console window on Windows in release
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod events;
mod log_capture;
mod state;

use std::collections::HashMap;
use std::sync::Arc;

use nisaba_core::config::{AmazonConfig, AppConfig, EbayConfig, SquarespaceConfig, XmrBazaarConfig};
use nisaba_core::crypto;
use nisaba_core::db::Db;
use nisaba_core::sync_engine::SyncEngine;
use nisaba_p2p::types::{P2PCommand, P2PEvent};
use nisaba_p2p::P2PManager;
use tauri::{Emitter, Manager};
use tokio::sync::{mpsc, Mutex, RwLock};
use tracing::{info, warn};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::Layer as _;

use crate::commands::config::build_adapters;
use crate::log_capture::{CaptureLayer, LogBuffer};
use crate::state::{AppState, CompanyContext};

fn main() {
    // Install rustls crypto provider before any TLS usage.
    // Both `ring` and `aws-lc-rs` features are enabled (from different deps),
    // so rustls cannot auto-detect — we pick ring explicitly.
    rustls::crypto::ring::default_provider()
        .install_default()
        .expect("Failed to install rustls crypto provider");

    // Shared log buffer (keeps last 2000 lines)
    let log_buffer = LogBuffer::new(2000);

    // Initialize tracing with both stdout and capture layer
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer().with_filter(
                tracing_subscriber::EnvFilter::try_from_default_env()
                    .unwrap_or_else(|_| "info,nisaba=debug".parse().unwrap()),
            ),
        )
        .with(CaptureLayer::new(log_buffer.clone()))
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .register_uri_scheme_protocol("cachedimg", commands::images::handle_protocol_request)
        .setup(move |app| {
            let handle = app.handle().clone();

            // Enable real-time log emission to frontend
            log_capture::set_app_handle(handle.clone());

            // ── Phase 1: Manage state synchronously in setup ────────
            // This runs via block_on so state is guaranteed available
            // before the webview makes any IPC calls.
            let log_buffer = log_buffer.clone();
            let (config, config_path, config_dir, app_db, active_id) =
                tauri::async_runtime::block_on(async {
                    let (config, config_path) = AppConfig::resolve_and_load(None)
                        .expect("Failed to load config");

                    info!("Loaded config from {}", config_path.display());

                    let config_dir = config_path
                        .parent()
                        .map(|p| p.to_path_buf())
                        .unwrap_or_else(|| std::path::PathBuf::from("."));

                    let app_db = Arc::new(
                        Db::open(&config.database.path)
                            .await
                            .expect("Failed to open database"),
                    );
                    app_db.migrate().await.expect("Failed to run migrations");
                    info!("App database ready at {}", config.database.path);

                    let active_id = app_db
                        .get_app_setting("active_company_id")
                        .await
                        .unwrap_or(None);

                    (config, config_path, config_dir, app_db, active_id)
                });

            let state = AppState {
                app_db: app_db.clone(),
                config: RwLock::new(config.clone()),
                config_path,
                config_dir,
                log_buffer,
                companies: RwLock::new(HashMap::new()),
                active_company_id: RwLock::new(active_id.clone()),
                vendor_syncing: tokio::sync::Mutex::new(std::collections::HashSet::new()),
            };
            handle.manage(state);
            info!("App state managed — frontend commands available");

            // ── Phase 2: Company initialization (slow, in background) ──
            tauri::async_runtime::spawn(async move {
                let registry = app_db
                    .list_registered_companies()
                    .await
                    .unwrap_or_default();

                let mut companies = HashMap::new();

                for entry in &registry {
                    // Share app_db only when the company uses the exact same file.
                    // Otherwise open the company's own database.
                    let shared = if entry.db_path == config.database.path {
                        Some(&app_db)
                    } else {
                        None
                    };
                    match init_company_context(&entry.id, &entry.name, &entry.role, &entry.db_path, &config, handle.clone(), shared).await {
                        Ok(ctx) => {
                            info!(company = %entry.name, id = %entry.id, "Company initialized");
                            companies.insert(entry.id.clone(), ctx);
                        }
                        Err(e) => {
                            warn!(company = %entry.name, error = %e, "Failed to initialize company");
                        }
                    }
                }

                // If no companies registered but old company table has data, do legacy init
                if companies.is_empty() {
                    if let Ok(Some((id, name, secret, _onion, role))) = app_db.get_company().await {
                        info!("Migrating legacy single-company to multi-company");

                        if let Err(e) = crypto::store_secret_in_keyring(&id, &secret) {
                            warn!("Failed to store secret in keyring: {e}");
                        }

                        let secret_hash = crypto::hash_secret(&secret);
                        let _ = app_db.update_company_secret(&secret_hash).await;

                        let db_path = config.database.path.clone();
                        let _ = app_db.register_company(&id, &name, &role, &secret_hash, &db_path).await;
                        let _ = app_db.set_app_setting("active_company_id", &id).await;

                        match init_company_context(&id, &name, &role, &db_path, &config, handle.clone(), Some(&app_db)).await {
                            Ok(ctx) => {
                                companies.insert(id.clone(), ctx);
                            }
                            Err(e) => {
                                warn!("Failed to init legacy company: {e}");
                            }
                        }
                    }
                }

                // Determine final active company
                let final_active_id = if let Some(ref id) = active_id {
                    if companies.contains_key(id) {
                        Some(id.clone())
                    } else {
                        companies.keys().next().cloned()
                    }
                } else {
                    companies.keys().next().cloned()
                };

                // Update state with initialized companies
                let state: tauri::State<'_, AppState> = handle.state();
                *state.companies.write().await = companies;
                *state.active_company_id.write().await = final_active_id;

                // Tell frontend that companies are fully initialized
                let _ = handle.emit("companies-ready", ());

                info!("Nisaba app initialized");
            });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            // Products
            commands::products::list_products,
            commands::products::get_product,
            commands::products::create_product,
            commands::products::update_product,
            commands::products::delete_product,
            commands::products::get_mappings,
            commands::products::create_mapping,
            commands::products::delete_mapping,
            // Sync
            commands::sync::trigger_sync,
            commands::sync::pause_sync,
            commands::sync::resume_sync,
            commands::sync::get_sync_status,
            commands::sync::get_recent_events,
            // Listings
            commands::listings::fetch_all_listings,
            commands::listings::get_cached_platform_listings,
            commands::listings::fetch_unmapped_listings,
            commands::listings::diff_listings,
            commands::listings::migrate_listing,
            commands::listings::create_listing_on_platform,
            commands::listings::update_listing_on_platform,
            commands::listings::cache_listing_detail,
            commands::listings::get_cached_listings,
            commands::listings::ebay_get_category_suggestions,
            commands::listings::ebay_get_category_aspects,
            commands::listings::ebay_get_business_policies,
            // Config
            commands::config::get_config,
            commands::config::update_config,
            commands::config::get_company_platform_config,
            commands::config::update_company_platform_config,
            commands::config::get_platform_capabilities,
            // Analytics
            commands::analytics::get_product_analytics,
            commands::analytics::get_all_analytics,
            // Auth
            commands::auth::start_ebay_auth,
            commands::auth::get_ebay_auth_url,
            commands::auth::complete_ebay_auth,
            commands::auth::start_amazon_auth,
            commands::auth::complete_amazon_auth,
            commands::auth::check_platform_auth,
            commands::auth::get_platform_health,
            // Photos
            commands::photos::get_product_photos,
            commands::photos::sync_photo,
            // Pricing
            commands::pricing::get_product_prices,
            commands::pricing::get_all_product_prices,
            commands::pricing::update_price,
            // Logs
            commands::logs::get_logs,
            commands::pricing::get_pricing_history,
            // Export / Import
            commands::export_import::export_data,
            commands::export_import::import_data,
            // Company / P2P
            commands::company::create_company,
            commands::company::join_company,
            commands::company::leave_company,
            commands::company::get_company,
            commands::company::get_onion_address,
            commands::company::get_company_secret,
            commands::company::add_company_peer,
            commands::company::remove_company_peer,
            commands::company::approve_company_peer,
            commands::company::reject_company_peer,
            commands::company::set_peer_role,
            commands::company::list_company_peers,
            commands::company::trigger_p2p_sync,
            // Company Logo
            commands::company::update_company_logo,
            commands::company::remove_company_logo,
            // Multi-company
            commands::company::list_companies,
            commands::company::get_active_company_id,
            commands::company::switch_company,
            // Product Variants
            commands::products::list_variants,
            commands::products::create_variant,
            commands::products::update_variant,
            commands::products::delete_variant,
            commands::products::update_variant_quantity,
            commands::products::get_all_variant_skus,
            commands::products::get_all_product_first_photos,
            commands::products::get_product_cache_freshness,
            commands::products::get_all_cache_freshness,
            // Vendor plugins
            commands::vendors::list_registry_plugins,
            commands::vendors::submit_vendor_plugin,
            commands::vendors::import_vendor_plugin,
            commands::vendors::approve_vendor_plugin,
            commands::vendors::reject_vendor_plugin,
            commands::vendors::set_vendor_plugin_config,
            commands::vendors::set_vendor_include_stock,
            commands::vendors::get_vendor_plugin_config,
            commands::vendors::remove_registry_plugin,
            commands::vendors::install_vendor_plugin,
            commands::vendors::uninstall_vendor_plugin,
            commands::vendors::fetch_vendor_listings,
            commands::vendors::get_cached_vendor_listings,
            commands::vendors::sync_vendor_listings,
            commands::vendors::get_vendor_sync_status,
            commands::vendors::import_vendor_listings,
            commands::vendors::get_vendor_listings_for_product,
            // Image cache
            commands::images::load_image_cache_manifest,
            commands::images::cache_images,
            commands::images::get_cached_images,
            commands::images::generate_missing_thumbnails,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Nisaba");
}

/// Initialize a single company's runtime context.
///
/// When `shared_db` is provided, the existing DB instance is reused directly
/// to avoid opening the same turso file twice (which causes MVCC checkpoint
/// corruption). Callers must only pass `Some` when the company uses the same
/// database file as the app database.
async fn init_company_context(
    id: &str,
    name: &str,
    role: &str,
    db_path: &str,
    config: &AppConfig,
    handle: tauri::AppHandle,
    shared_db: Option<&Arc<Db>>,
) -> Result<CompanyContext, String> {
    let db = if let Some(existing) = shared_db {
        existing.clone()
    } else {
        let new_db = Arc::new(
            Db::open(db_path)
                .await
                .map_err(|e| format!("Failed to open company DB: {e}"))?,
        );
        new_db
            .migrate()
            .await
            .map_err(|e| format!("Failed to migrate company DB: {e}"))?;
        new_db
    };

    // Load secret from keyring and decrypt company config
    let company_config = match crypto::load_secret_from_keyring(id) {
        Ok(secret) => {
            let key = crypto::derive_key(&secret);
            match db.get_company_config().await {
                Ok(Some((encrypted_json, _updated_at))) => {
                    match crypto::decrypt(&encrypted_json, &key) {
                        Ok(json) => {
                            serde_json::from_str::<serde_json::Value>(&json).ok()
                        }
                        Err(e) => {
                            warn!(company = %name, "Failed to decrypt config: {e}");
                            None
                        }
                    }
                }
                _ => None,
            }
        }
        Err(e) => {
            warn!(company = %name, "No secret in keyring: {e}");
            None
        }
    };

    // Build a config for adapter construction by overlaying company config on app config
    let adapter_config = build_adapter_config(config, &company_config);

    // Build adapters
    let adapter_map = build_adapters(&adapter_config, &db).await;
    let adapters = Arc::new(RwLock::new(adapter_map));

    // Sync engine channels
    let (event_tx, event_rx) = mpsc::channel(256);
    let (cmd_tx, cmd_rx) = mpsc::channel(32);

    // Build sync engine
    let engine = SyncEngine::new(
        db.clone(),
        adapters.clone(),
        event_tx,
        config.general.max_retries,
    );

    let interval_secs = parse_interval(&config.general.sync_schedule);

    // Spawn sync engine
    tokio::spawn(engine.run(cmd_rx, interval_secs));

    // Bridge sync events to Tauri frontend (tagged with company_id)
    tokio::spawn(events::bridge_sync_events(handle.clone(), event_rx));

    // P2P startup
    let mut p2p_manager: Option<Arc<P2PManager>> = None;
    let mut p2p_cmd_tx: Option<mpsc::Sender<P2PCommand>> = None;

    if let Ok(Some((_, _, secret, _, _))) = db.get_company().await {
        // For legacy compat, the secret in DB might be a hash now.
        // Try loading from keyring first.
        let actual_secret = crypto::load_secret_from_keyring(id).unwrap_or(secret);

        let (p2p_event_tx, mut p2p_event_rx) = mpsc::channel::<P2PEvent>(256);
        let manager = Arc::new(P2PManager::new(db.clone(), p2p_event_tx));

        let handle_clone = handle.clone();
        tokio::spawn(async move {
            while let Some(event) = p2p_event_rx.recv().await {
                let _ = handle_clone.emit("p2p-event", &event);
            }
        });

        match manager.start_onion_service(actual_secret).await {
            Ok(()) => {
                info!(company = %name, "P2P onion service starting");

                let (tx, rx) = mpsc::channel(32);
                let mgr_clone = manager.clone();
                let interval = config.company.sync_interval_secs;
                tokio::spawn(async move {
                    mgr_clone.run_periodic_sync(rx, interval).await;
                });

                p2p_cmd_tx = Some(tx);
                p2p_manager = Some(manager);
            }
            Err(e) => {
                warn!(company = %name, "Failed to start P2P: {e}");
            }
        }
    }

    Ok(CompanyContext {
        role: role.to_string(),
        db,
        adapters,
        sync_cmd_tx: cmd_tx,
        sync_paused: Mutex::new(false),
        p2p: p2p_manager,
        p2p_cmd_tx,
    })
}

/// Build an AppConfig for adapter construction by overlaying company-specific platform config.
///
/// Starts with all platforms DISABLED. Only the company's own saved config can enable platforms.
/// General/database/alerts settings are inherited from the base (global) config.
fn build_adapter_config(
    base: &AppConfig,
    company_config: &Option<serde_json::Value>,
) -> AppConfig {
    let mut config = base.clone();

    // Always start with platforms disabled — each company must explicitly enable its own.
    config.ebay = EbayConfig::default();
    config.squarespace = SquarespaceConfig::default();
    config.xmrbazaar = XmrBazaarConfig::default();
    config.amazon = AmazonConfig::default();

    if let Some(cc) = company_config {
        if let Some(ebay) = cc.get("ebay") {
            if let Ok(e) = serde_json::from_value(ebay.clone()) {
                config.ebay = e;
            }
        }
        if let Some(sq) = cc.get("squarespace") {
            if let Ok(s) = serde_json::from_value(sq.clone()) {
                config.squarespace = s;
            }
        }
        if let Some(xmr) = cc.get("xmrbazaar") {
            if let Ok(x) = serde_json::from_value(xmr.clone()) {
                config.xmrbazaar = x;
            }
        }
        if let Some(amazon) = cc.get("amazon") {
            if let Ok(a) = serde_json::from_value(amazon.clone()) {
                config.amazon = a;
            }
        }
        if let Some(alerts) = cc.get("alerts") {
            if let Ok(a) = serde_json::from_value(alerts.clone()) {
                config.alerts = a;
            }
        }
    }

    config
}

/// Parse the sync schedule string to an interval in seconds.
/// Supports "0 */N * * * *" format or falls back to 300s (5min).
fn parse_interval(schedule: &str) -> u64 {
    let parts: Vec<&str> = schedule.split_whitespace().collect();
    if parts.len() >= 2 {
        let minute_part = parts[1];
        if let Some(stripped) = minute_part.strip_prefix("*/") {
            if let Ok(minutes) = stripped.parse::<u64>() {
                return minutes * 60;
            }
        }
    }
    300 // default 5 minutes
}
