use std::collections::HashMap;
use std::sync::Arc;

use nisaba_core::config::AppConfig;
use nisaba_core::crypto;
use nisaba_core::sync_engine::SyncCommand;
use nisaba_core::traits::PlatformAdapter;
use nisaba_core::types::{Platform, PlatformCapabilities};
use nisaba_amazon::AmazonAdapter;
use nisaba_ebay::EbayAdapter;
use nisaba_squarespace::SquarespaceAdapter;
use nisaba_xmrbazaar::XmrBazaarAdapter;
use tauri::State;
use tracing::info;

use crate::state::AppState;

#[tauri::command]
pub async fn get_config(state: State<'_, AppState>) -> Result<AppConfig, String> {
    Ok(state.config.read().await.clone())
}

#[tauri::command]
pub async fn update_config(
    state: State<'_, AppState>,
    config: AppConfig,
) -> Result<(), String> {
    config
        .save(&state.config_path)
        .map_err(|e| e.to_string())?;

    // Rebuild adapters overlaying company config on the new TOML config
    let db = state.active_db().await?;
    let company_config = load_company_config_value(&state).await;
    let adapter_config = crate::build_adapter_config(&config, &company_config);
    let new_adapters = build_adapters(&adapter_config, &db).await;
    {
        let adapters = state.active_adapters().await?;
        let mut adapters = adapters.write().await;
        *adapters = new_adapters;
    }

    // Notify sync engine
    if let Ok(tx) = state.active_sync_tx().await {
        let _ = tx.send(SyncCommand::Reload).await;
    }

    *state.config.write().await = config;
    info!("Config updated and adapters rebuilt");

    Ok(())
}

#[tauri::command]
pub async fn get_company_platform_config(
    state: State<'_, AppState>,
) -> Result<serde_json::Value, String> {
    // Default platform config template — used both as fallback when no config
    // exists and to fill in missing keys for configs saved before new platforms
    // were added.
    let defaults = serde_json::json!({
        "ebay": {
            "enabled": false,
            "client_id": "",
            "client_secret": "",
            "redirect_uri": "",
            "environment": "sandbox"
        },
        "squarespace": {
            "enabled": false,
            "api_key": "",
            "user_agent": ""
        },
        "xmrbazaar": {
            "enabled": false,
            "username": "",
            "password": "",
            "base_url": "https://xmrbazaar.com"
        },
        "amazon": {
            "enabled": false,
            "client_id": "",
            "client_secret": "",
            "refresh_token": "",
            "seller_id": "",
            "region": "NA",
            "marketplace_ids": []
        },
        "alerts": {
            "default_low_stock_threshold": 5
        }
    });

    // Try to load encrypted company config and merge in defaults for any
    // keys missing from the stored config (e.g. a new platform added after
    // the config was first saved).
    match load_company_config_value(&state).await {
        Some(serde_json::Value::Object(mut stored)) => {
            if let serde_json::Value::Object(defs) = defaults {
                for (key, default_val) in defs {
                    stored.entry(key).or_insert(default_val);
                }
            }
            Ok(serde_json::Value::Object(stored))
        }
        _ => Ok(defaults),
    }
}

#[tauri::command]
pub async fn update_company_platform_config(
    state: State<'_, AppState>,
    config: serde_json::Value,
) -> Result<(), String> {
    let company_id = {
        let id = state.active_company_id.read().await;
        id.as_ref().cloned().ok_or_else(|| "No active company".to_string())?
    };

    // Load secret from keyring and encrypt
    let secret = crypto::load_secret_from_keyring(&company_id)
        .map_err(|e| format!("Failed to load company secret: {e}"))?;
    let key = crypto::derive_key(&secret);

    let json_str = serde_json::to_string(&config)
        .map_err(|e| format!("Failed to serialize config: {e}"))?;
    let encrypted = crypto::encrypt(&json_str, &key);

    // Save to company DB
    let db = state.active_db().await?;
    let now = chrono::Utc::now().to_rfc3339();
    db.set_company_config(&encrypted, &now)
        .await
        .map_err(|e| format!("Failed to save company config: {e}"))?;

    // Rebuild adapters: overlay new company config on TOML
    let base_config = state.config.read().await.clone();
    let adapter_config = crate::build_adapter_config(&base_config, &Some(config));
    let new_adapters = build_adapters(&adapter_config, &db).await;
    {
        let adapters = state.active_adapters().await?;
        let mut adapters = adapters.write().await;
        *adapters = new_adapters;
    }

    // Notify sync engine
    if let Ok(tx) = state.active_sync_tx().await {
        let _ = tx.send(SyncCommand::Reload).await;
    }

    info!("Company platform config updated and adapters rebuilt");
    Ok(())
}

/// Load the decrypted company config value for the active company.
/// Returns None if no company is active, no secret in keyring, or no config saved.
async fn load_company_config_value(state: &AppState) -> Option<serde_json::Value> {
    let company_id = state.active_company_id.read().await.clone()?;
    let secret = crypto::load_secret_from_keyring(&company_id).ok()?;
    let key = crypto::derive_key(&secret);
    let db = state.active_db().await.ok()?;

    let (encrypted_json, _updated_at) = db.get_company_config().await.ok()??;
    let json = crypto::decrypt(&encrypted_json, &key).ok()?;
    serde_json::from_str(&json).ok()
}

#[tauri::command]
pub async fn get_platform_capabilities(
    state: State<'_, AppState>,
) -> Result<HashMap<String, PlatformCapabilities>, String> {
    let adapters = state.active_adapters().await?;
    let adapters = adapters.read().await;
    let mut caps = HashMap::new();
    for (platform, adapter) in adapters.iter() {
        caps.insert(platform.as_str().to_string(), adapter.capabilities());
    }
    Ok(caps)
}

/// Build platform adapters from config.
pub async fn build_adapters(
    config: &AppConfig,
    db: &Arc<nisaba_core::db::Db>,
) -> HashMap<Platform, Arc<dyn PlatformAdapter>> {
    let mut adapters: HashMap<Platform, Arc<dyn PlatformAdapter>> = HashMap::new();

    if config.ebay.enabled {
        match EbayAdapter::new(&config.ebay, db.clone()).await {
            Ok(adapter) => {
                adapters.insert(Platform::Ebay, Arc::new(adapter));
                info!("eBay adapter enabled");
            }
            Err(e) => {
                tracing::warn!("Failed to create eBay adapter: {e}");
            }
        }
    }

    if config.squarespace.enabled {
        let adapter = SquarespaceAdapter::new(&config.squarespace);
        adapters.insert(Platform::Squarespace, Arc::new(adapter));
        info!("Squarespace adapter enabled");
    }

    if config.xmrbazaar.enabled {
        let adapter = XmrBazaarAdapter::new(&config.xmrbazaar);
        // Auth is lazy — refresh_auth() will be called on first actual API use.
        // Skipping it here avoids a 500ms–2s network call that blocks startup.
        adapters.insert(Platform::XmrBazaar, Arc::new(adapter));
        info!("XMR Bazaar adapter enabled");
    }

    if config.amazon.enabled {
        match AmazonAdapter::new(&config.amazon, db.clone()).await {
            Ok(adapter) => {
                adapters.insert(Platform::Amazon, Arc::new(adapter));
                info!("Amazon adapter enabled");
            }
            Err(e) => {
                tracing::warn!("Failed to create Amazon adapter: {e}");
            }
        }
    }

    adapters
}
