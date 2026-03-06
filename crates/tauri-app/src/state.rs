use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;

use nisaba_core::config::AppConfig;
use nisaba_core::db::Db;
use nisaba_core::sync_engine::{SharedAdapters, SyncCommand};
use nisaba_p2p::types::P2PCommand;
use nisaba_p2p::P2PManager;
use tokio::sync::{mpsc, Mutex, RwLock};

use crate::log_capture::LogBuffer;

/// Per-company runtime context. Each company has its own DB, adapters, sync engine, and P2P.
pub struct CompanyContext {
    pub role: String,
    pub db: Arc<Db>,
    pub adapters: SharedAdapters,
    pub sync_cmd_tx: mpsc::Sender<SyncCommand>,
    pub sync_paused: Mutex<bool>,
    pub p2p: Option<Arc<P2PManager>>,
    pub p2p_cmd_tx: Option<mpsc::Sender<P2PCommand>>,
}

/// Global application state supporting multiple companies.
pub struct AppState {
    /// The app/master database (company registry, app settings).
    pub app_db: Arc<Db>,
    /// Local-only config from the TOML file (general, database, log settings).
    pub config: RwLock<AppConfig>,
    /// Path to the config TOML file.
    pub config_path: PathBuf,
    /// Directory containing the config file (for resolving company DB paths).
    pub config_dir: PathBuf,
    /// Shared log buffer.
    pub log_buffer: LogBuffer,
    /// All loaded companies keyed by company ID.
    pub companies: RwLock<HashMap<String, CompanyContext>>,
    /// Currently active company ID.
    pub active_company_id: RwLock<Option<String>>,
    /// Plugin IDs currently running a vendor sync.
    pub vendor_syncing: Mutex<HashSet<String>>,
}

impl AppState {
    /// Resolve the active company ID as an owned String.
    async fn resolve_active_id(&self) -> Result<String, String> {
        let id = self.active_company_id.read().await;
        id.as_ref().cloned().ok_or_else(|| "No active company".to_string())
    }

    /// Get the active company's DB.
    pub async fn active_db(&self) -> Result<Arc<Db>, String> {
        let id = self.resolve_active_id().await?;
        let companies = self.companies.read().await;
        let ctx = companies.get(&id).ok_or("Active company not found")?;
        Ok(ctx.db.clone())
    }

    /// Get the active company's adapters.
    pub async fn active_adapters(&self) -> Result<SharedAdapters, String> {
        let id = self.resolve_active_id().await?;
        let companies = self.companies.read().await;
        let ctx = companies.get(&id).ok_or("Active company not found")?;
        Ok(ctx.adapters.clone())
    }

    /// Get the active company's sync command sender.
    pub async fn active_sync_tx(&self) -> Result<mpsc::Sender<SyncCommand>, String> {
        let id = self.resolve_active_id().await?;
        let companies = self.companies.read().await;
        let ctx = companies.get(&id).ok_or("Active company not found")?;
        Ok(ctx.sync_cmd_tx.clone())
    }

    /// Get the active company's sync paused state.
    pub async fn active_sync_paused(&self) -> Result<bool, String> {
        let id = self.resolve_active_id().await?;
        let companies = self.companies.read().await;
        let ctx = companies.get(&id).ok_or("Active company not found")?;
        let paused = *ctx.sync_paused.lock().await;
        Ok(paused)
    }

    /// Set the active company's sync paused state.
    pub async fn set_active_sync_paused(&self, paused: bool) -> Result<(), String> {
        let id = self.resolve_active_id().await?;
        let companies = self.companies.read().await;
        let ctx = companies.get(&id).ok_or("Active company not found")?;
        *ctx.sync_paused.lock().await = paused;
        Ok(())
    }

    /// Get the active company's P2P manager.
    pub async fn active_p2p(&self) -> Result<Option<Arc<P2PManager>>, String> {
        let id = self.resolve_active_id().await?;
        let companies = self.companies.read().await;
        let ctx = companies.get(&id).ok_or("Active company not found")?;
        Ok(ctx.p2p.clone())
    }

    /// Get the active company's P2P command sender.
    pub async fn active_p2p_cmd_tx(&self) -> Result<Option<mpsc::Sender<P2PCommand>>, String> {
        let id = self.resolve_active_id().await?;
        let companies = self.companies.read().await;
        let ctx = companies.get(&id).ok_or("Active company not found")?;
        Ok(ctx.p2p_cmd_tx.clone())
    }

    /// Get the active company's role.
    pub async fn active_role(&self) -> Result<String, String> {
        let id = self.resolve_active_id().await?;
        let companies = self.companies.read().await;
        let ctx = companies.get(&id).ok_or("Active company not found")?;
        Ok(ctx.role.clone())
    }
}
