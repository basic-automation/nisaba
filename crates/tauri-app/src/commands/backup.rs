//! N-8.1: database backup commands.
//!
//! Backups land next to the database (`<db-dir>/backups/`) and never leave
//! the machine. The app database is backed up automatically before every
//! migration and on the configured schedule (see the scheduler in
//! `main.rs`); these commands expose manual backup, listing and restore to
//! the UI. After a restore the UI restarts the app, because turso keeps the
//! old file's pages cached in open connections.

use std::path::Path;

use nisaba_core::db::{BackupConfig, BackupInfo, BackupReason};
use tauri::State;

use crate::state::AppState;

async fn current_backup_config(state: &State<'_, AppState>) -> BackupConfig {
    BackupConfig::from_database_config(&state.config.read().await.database)
}

/// List the app database's backups, newest first.
#[tauri::command]
pub async fn list_backups(state: State<'_, AppState>) -> Result<Vec<BackupInfo>, String> {
    let cfg = current_backup_config(&state).await;
    state
        .app_db
        .list_backups(&cfg)
        .await
        .map_err(|e| e.to_string())
}

/// Write a timestamped backup of the app database now. Returns the backup path.
#[tauri::command]
pub async fn backup_now(state: State<'_, AppState>) -> Result<String, String> {
    let cfg = current_backup_config(&state).await;
    let path = state
        .app_db
        .backup_now(&cfg, BackupReason::Manual)
        .await
        .map_err(|e| e.to_string())?;
    Ok(path.display().to_string())
}

/// Restore the app database from a backup file inside the backup directory.
///
/// Takes a `pre-restore` safety snapshot of the live database first. The
/// caller MUST restart the app afterwards (the UI does this via
/// `plugin:process|restart`): open turso connections keep the old file's
/// pages cached.
#[tauri::command]
pub async fn restore_backup(state: State<'_, AppState>, backup_path: String) -> Result<(), String> {
    let cfg = current_backup_config(&state).await;
    state
        .app_db
        .restore_backup(&cfg, Path::new(&backup_path))
        .await
        .map_err(|e| e.to_string())
}
