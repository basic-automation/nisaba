use std::sync::Arc;

use tauri::State;
use tokio::sync::mpsc;
use tracing::info;

use nisaba_core::config::CompanyRegistryEntry;
use nisaba_core::crypto;
use nisaba_core::db::Db;
use nisaba_p2p::types::{CompanyInfo, P2PCommand, P2PEvent, PeerInfo};
use nisaba_p2p::P2PManager;

use crate::state::{AppState, CompanyContext};

/// Start the P2P manager for a company context.
async fn start_p2p_for_company(
    ctx: &mut CompanyContext,
    secret: &str,
    interval_secs: u64,
) -> Result<(), String> {
    let (event_tx, mut event_rx) = mpsc::channel::<P2PEvent>(256);
    let manager = Arc::new(P2PManager::new(ctx.db.clone(), event_tx));

    manager
        .start_onion_service(secret.to_string())
        .await
        .map_err(|e| e.to_string())?;

    let (cmd_tx, cmd_rx) = mpsc::channel(32);

    // Drain P2P events
    tokio::spawn(async move { while let Some(_event) = event_rx.recv().await {} });

    let manager_clone = manager.clone();
    tokio::spawn(async move {
        manager_clone.run_periodic_sync(cmd_rx, interval_secs).await;
    });

    ctx.p2p = Some(manager);
    ctx.p2p_cmd_tx = Some(cmd_tx);

    Ok(())
}

#[tauri::command]
pub async fn create_company(
    state: State<'_, AppState>,
    name: String,
) -> Result<CompanyInfo, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let secret = uuid::Uuid::new_v4().to_string();
    let peer_id = uuid::Uuid::new_v4().to_string();
    let secret_hash = crypto::hash_secret(&secret);

    // Store secret in OS keyring
    crypto::store_secret_in_keyring(&id, &secret).map_err(|e| e.to_string())?;

    // Extract config values we need (drop the lock before calling get_company)
    let (max_retries, sync_schedule, p2p_interval) = {
        let config = state.config.read().await;
        (
            config.general.max_retries,
            config.general.sync_schedule.clone(),
            config.company.sync_interval_secs,
        )
    };

    // Determine company DB path
    let companies_dir = state.config_dir.join("companies");
    std::fs::create_dir_all(&companies_dir)
        .map_err(|e| format!("Failed to create companies dir: {e}"))?;
    let db_path = companies_dir.join(format!("{id}.db"));
    let db_path_str = db_path.to_string_lossy().to_string();

    // Open and migrate company DB
    let db = Arc::new(
        Db::open(&db_path_str)
            .await
            .map_err(|e| format!("Failed to open company DB: {e}"))?,
    );
    db.migrate()
        .await
        .map_err(|e| format!("Failed to migrate company DB: {e}"))?;

    // Create company row in company DB
    db.create_company(&id, &name, &secret_hash, "admin")
        .await
        .map_err(|e| e.to_string())?;
    db.set_company_peer_id(&peer_id)
        .await
        .map_err(|e| e.to_string())?;

    // Register in app DB
    state
        .app_db
        .register_company(&id, &name, "admin", &secret_hash, &db_path_str)
        .await
        .map_err(|e| e.to_string())?;
    state
        .app_db
        .set_app_setting("active_company_id", &id)
        .await
        .map_err(|e| e.to_string())?;

    info!(company = %name, "Company created");

    // Build adapters
    let adapters = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));

    // Sync engine
    let (event_tx, _event_rx) = mpsc::channel(256);
    let (cmd_tx, cmd_rx) = mpsc::channel(32);
    let engine = nisaba_core::sync_engine::SyncEngine::new(
        db.clone(),
        adapters.clone(),
        event_tx,
        max_retries,
    );
    let interval_secs = super::super::parse_interval(&sync_schedule);
    tokio::spawn(engine.run(cmd_rx, interval_secs));

    let mut ctx = CompanyContext {
        role: "admin".to_string(),
        db: db.clone(),
        adapters,
        sync_cmd_tx: cmd_tx,
        sync_paused: tokio::sync::Mutex::new(false),
        p2p: None,
        p2p_cmd_tx: None,
    };

    // Start P2P
    let _ = start_p2p_for_company(&mut ctx, &secret, p2p_interval).await;

    // Set active and store context
    *state.active_company_id.write().await = Some(id.clone());
    state.companies.write().await.insert(id.clone(), ctx);

    get_company(state).await
}

#[tauri::command]
pub async fn join_company(
    state: State<'_, AppState>,
    admin_onion: String,
    secret: String,
    name: String,
) -> Result<CompanyInfo, String> {
    let id = uuid::Uuid::new_v4().to_string();
    let peer_id = uuid::Uuid::new_v4().to_string();
    let secret_hash = crypto::hash_secret(&secret);

    // Store secret in keyring
    crypto::store_secret_in_keyring(&id, &secret).map_err(|e| e.to_string())?;

    // Extract config values
    let (max_retries, sync_schedule, p2p_interval) = {
        let config = state.config.read().await;
        (
            config.general.max_retries,
            config.general.sync_schedule.clone(),
            config.company.sync_interval_secs,
        )
    };

    // Create company DB
    let companies_dir = state.config_dir.join("companies");
    std::fs::create_dir_all(&companies_dir)
        .map_err(|e| format!("Failed to create companies dir: {e}"))?;
    let db_path = companies_dir.join(format!("{id}.db"));
    let db_path_str = db_path.to_string_lossy().to_string();

    let db = Arc::new(
        Db::open(&db_path_str)
            .await
            .map_err(|e| format!("Failed to open company DB: {e}"))?,
    );
    db.migrate()
        .await
        .map_err(|e| format!("Failed to migrate company DB: {e}"))?;

    db.create_company(&id, &name, &secret_hash, "member")
        .await
        .map_err(|e| e.to_string())?;
    db.set_company_peer_id(&peer_id)
        .await
        .map_err(|e| e.to_string())?;

    // Register in app DB
    state
        .app_db
        .register_company(&id, &name, "member", &secret_hash, &db_path_str)
        .await
        .map_err(|e| e.to_string())?;
    state
        .app_db
        .set_app_setting("active_company_id", &id)
        .await
        .map_err(|e| e.to_string())?;

    // Build context
    let adapters = Arc::new(tokio::sync::RwLock::new(std::collections::HashMap::new()));
    let (event_tx, _event_rx) = mpsc::channel(256);
    let (cmd_tx, cmd_rx) = mpsc::channel(32);
    let engine = nisaba_core::sync_engine::SyncEngine::new(
        db.clone(),
        adapters.clone(),
        event_tx,
        max_retries,
    );
    let interval_secs = super::super::parse_interval(&sync_schedule);
    tokio::spawn(engine.run(cmd_rx, interval_secs));

    let mut ctx = CompanyContext {
        role: "member".to_string(),
        db: db.clone(),
        adapters,
        sync_cmd_tx: cmd_tx,
        sync_paused: tokio::sync::Mutex::new(false),
        p2p: None,
        p2p_cmd_tx: None,
    };

    let _ = start_p2p_for_company(&mut ctx, &secret, p2p_interval).await;

    // Wait for onion address
    let mut our_onion = None;
    if let Some(ref manager) = ctx.p2p {
        for _ in 0..60 {
            if let Some(addr) = manager.our_onion().await {
                our_onion = Some(addr);
                break;
            }
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        }
    }

    let our_onion = our_onion.ok_or("Timed out waiting for onion service")?;

    // Add admin as peer
    let admin_peer_id = uuid::Uuid::new_v4().to_string();
    db.insert_peer_v2(&admin_peer_id, &admin_onion, "admin", "admin", true)
        .await
        .map_err(|e| e.to_string())?;

    // Send join request
    let hostname = format!("{}.onion", our_onion);
    let _ = nisaba_p2p::client::P2PClient::join_company(&admin_onion, &secret, &hostname, &name)
        .await
        .map_err(|e| e.to_string())?;

    info!("Joined company via admin {}", admin_onion);

    *state.active_company_id.write().await = Some(id.clone());
    state.companies.write().await.insert(id.clone(), ctx);

    get_company(state).await
}

#[tauri::command]
pub async fn leave_company(state: State<'_, AppState>, id: Option<String>) -> Result<(), String> {
    let company_id = if let Some(id) = id {
        id
    } else {
        state
            .active_company_id
            .read()
            .await
            .clone()
            .ok_or("No active company")?
    };

    // Enforce: admin can't leave if they're the only admin and there are peers
    {
        let companies = state.companies.read().await;
        if let Some(ctx) = companies.get(&company_id) {
            if ctx.role == "admin" {
                let peers = ctx.db.list_peers_v2().await.map_err(|e| e.to_string())?;
                let authorized_peers: Vec<_> = peers.iter().filter(|p| p.4).collect();
                if !authorized_peers.is_empty() {
                    let other_admins = authorized_peers.iter().filter(|p| p.3 == "admin").count();
                    if other_admins == 0 {
                        return Err(
                            "You are the only admin. Promote another peer to admin before leaving."
                                .to_string(),
                        );
                    }
                }
            }
        }
    }

    // Stop P2P for this company
    {
        let companies = state.companies.read().await;
        if let Some(ctx) = companies.get(&company_id) {
            if let Some(ref tx) = ctx.p2p_cmd_tx {
                let _ = tx.send(P2PCommand::Stop).await;
            }
        }
    }

    // Remove from companies map
    state.companies.write().await.remove(&company_id);

    // Get DB path before unregistering
    let registry = state
        .app_db
        .list_registered_companies()
        .await
        .map_err(|e| e.to_string())?;
    let db_path = registry
        .iter()
        .find(|e| e.id == company_id)
        .map(|e| e.db_path.clone());

    // Unregister from app DB
    state
        .app_db
        .unregister_company(&company_id)
        .await
        .map_err(|e| e.to_string())?;

    // Delete secret from keyring
    let _ = crypto::delete_secret_from_keyring(&company_id);

    // Delete company DB file
    if let Some(path) = db_path {
        let _ = std::fs::remove_file(&path);
    }

    // Update active company
    let remaining = state.companies.read().await;
    let new_active = remaining.keys().next().cloned();
    drop(remaining);
    *state.active_company_id.write().await = new_active.clone();
    if let Some(ref id) = new_active {
        let _ = state.app_db.set_app_setting("active_company_id", id).await;
    }

    info!(company = %company_id, "Left company");
    Ok(())
}

#[tauri::command]
pub async fn get_company(state: State<'_, AppState>) -> Result<CompanyInfo, String> {
    let db = state.active_db().await?;
    let p2p = state.active_p2p().await?;

    let our_onion = if let Some(ref m) = p2p {
        m.our_onion().await.unwrap_or_default()
    } else {
        String::new()
    };

    let info = nisaba_p2p::server::build_company_info(&db, &our_onion)
        .await
        .map_err(|e| e.to_string())?;

    info.ok_or_else(|| "No company configured".to_string())
}

#[tauri::command]
pub async fn get_onion_address(state: State<'_, AppState>) -> Result<Option<String>, String> {
    let p2p = state.active_p2p().await?;
    if let Some(ref m) = p2p {
        Ok(m.our_onion().await)
    } else {
        Ok(None)
    }
}

#[tauri::command]
pub async fn get_company_secret(state: State<'_, AppState>) -> Result<Option<String>, String> {
    // Admin only
    let role = state.active_role().await?;
    if role != "admin" {
        return Err("Admin access required to view company secret".to_string());
    }

    let id = state
        .active_company_id
        .read()
        .await
        .clone()
        .ok_or("No active company")?;

    match crypto::load_secret_from_keyring(&id) {
        Ok(secret) => Ok(Some(secret)),
        Err(e) => {
            tracing::warn!(company_id = %id, error = %e, "Failed to load secret from keyring");
            Err(format!("Failed to load secret: {e}"))
        }
    }
}

#[tauri::command]
pub async fn add_company_peer(
    state: State<'_, AppState>,
    onion_address: String,
    name: String,
) -> Result<(), String> {
    let db = state.active_db().await?;
    let peer_id = uuid::Uuid::new_v4().to_string();
    db.insert_peer_v2(&peer_id, &onion_address, &name, "member", true)
        .await
        .map_err(|e| e.to_string())?;
    // Also insert into legacy table for backward compat
    let _ = db.insert_peer(&onion_address, &name, "member", true).await;
    info!(peer = %onion_address, "Peer added");
    Ok(())
}

#[tauri::command]
pub async fn remove_company_peer(
    state: State<'_, AppState>,
    onion_address: String,
) -> Result<(), String> {
    let db = state.active_db().await?;
    // Find peer_id by onion address in v2 table
    if let Ok(Some((peer_id, _, _, _, _))) = db.get_peer_by_onion(&onion_address).await {
        db.remove_peer_v2(&peer_id)
            .await
            .map_err(|e| e.to_string())?;
    }
    // Also remove from legacy table
    let _ = db.remove_peer(&onion_address).await;
    info!(peer = %onion_address, "Peer removed");
    Ok(())
}

#[tauri::command]
pub async fn approve_company_peer(
    state: State<'_, AppState>,
    onion_address: String,
) -> Result<(), String> {
    let db = state.active_db().await?;
    if let Ok(Some((peer_id, _, _, _, _))) = db.get_peer_by_onion(&onion_address).await {
        db.approve_peer_v2(&peer_id)
            .await
            .map_err(|e| e.to_string())?;
    }
    let _ = db.approve_peer(&onion_address).await;
    info!(peer = %onion_address, "Peer approved");
    Ok(())
}

#[tauri::command]
pub async fn reject_company_peer(
    state: State<'_, AppState>,
    onion_address: String,
) -> Result<(), String> {
    let db = state.active_db().await?;
    if let Ok(Some((peer_id, _, _, _, _))) = db.get_peer_by_onion(&onion_address).await {
        db.reject_peer_v2(&peer_id)
            .await
            .map_err(|e| e.to_string())?;
    }
    let _ = db.reject_peer(&onion_address).await;
    info!(peer = %onion_address, "Peer rejected");
    Ok(())
}

#[tauri::command]
pub async fn set_peer_role(
    state: State<'_, AppState>,
    onion_address: String,
    role: String,
) -> Result<(), String> {
    if role != "admin" && role != "member" {
        return Err("Role must be 'admin' or 'member'".to_string());
    }
    let db = state.active_db().await?;
    if let Ok(Some((peer_id, _, _, _, _))) = db.get_peer_by_onion(&onion_address).await {
        db.update_peer_role_v2(&peer_id, &role)
            .await
            .map_err(|e| e.to_string())?;
    }
    let _ = db.update_peer_role(&onion_address, &role).await;
    info!(peer = %onion_address, role = %role, "Peer role updated");
    Ok(())
}

#[tauri::command]
pub async fn list_company_peers(state: State<'_, AppState>) -> Result<Vec<PeerInfo>, String> {
    let info = get_company(state).await?;
    Ok(info.peers)
}

#[tauri::command]
pub async fn trigger_p2p_sync(state: State<'_, AppState>) -> Result<(), String> {
    let tx = state.active_p2p_cmd_tx().await?;
    if let Some(tx) = tx {
        tx.send(P2PCommand::SyncNow)
            .await
            .map_err(|e| e.to_string())?;
        Ok(())
    } else {
        Err("P2P not running".to_string())
    }
}

// ── Company Logo ─────────────────────────────────────────────

#[tauri::command]
pub async fn update_company_logo(state: State<'_, AppState>, logo: String) -> Result<(), String> {
    let db = state.active_db().await?;
    let now = chrono::Utc::now().to_rfc3339();
    db.set_company_logo(&logo, &now)
        .await
        .map_err(|e| e.to_string())?;

    // Also update the registry entry so CompanySwitcher gets the logo
    if let Some(id) = state.active_company_id.read().await.clone() {
        let _ = state.app_db.update_registry_logo(&id, &logo).await;
    }

    Ok(())
}

#[tauri::command]
pub async fn remove_company_logo(state: State<'_, AppState>) -> Result<(), String> {
    let db = state.active_db().await?;
    let now = chrono::Utc::now().to_rfc3339();
    db.set_company_logo("", &now)
        .await
        .map_err(|e| e.to_string())?;

    // Clear from registry too
    if let Some(id) = state.active_company_id.read().await.clone() {
        let _ = state.app_db.update_registry_logo(&id, "").await;
    }

    Ok(())
}

// ── Multi-company commands ───────────────────────────────────

#[tauri::command]
pub async fn list_companies(
    state: State<'_, AppState>,
) -> Result<Vec<CompanyRegistryEntry>, String> {
    state
        .app_db
        .list_registered_companies()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn get_active_company_id(state: State<'_, AppState>) -> Result<Option<String>, String> {
    Ok(state.active_company_id.read().await.clone())
}

#[tauri::command]
pub async fn switch_company(state: State<'_, AppState>, id: String) -> Result<String, String> {
    // Verify company exists
    let companies = state.companies.read().await;
    if !companies.contains_key(&id) {
        return Err(format!("Company {id} not found"));
    }
    drop(companies);

    // Switch active
    *state.active_company_id.write().await = Some(id.clone());

    // Persist
    let _ = state.app_db.set_app_setting("active_company_id", &id).await;

    info!(company = %id, "Switched active company");

    Ok(id)
}
