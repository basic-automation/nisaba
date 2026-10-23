use nisaba_core::sync_engine::SyncCommand;
use nisaba_core::types::SyncEvent;
use tauri::State;

use crate::state::AppState;

#[tauri::command]
pub async fn trigger_sync(state: State<'_, AppState>) -> Result<(), String> {
    let tx = state.active_sync_tx().await?;
    tx.send(SyncCommand::SyncNow)
        .await
        .map_err(|e| e.to_string())
}

/// N-11.6: run one dry-run cycle and report what it *would* change.
/// The engine resolves everything as normal but writes to no platform and
/// no local tables; each planned change arrives as a `DryRunChange` event
/// bracketed by `DryRunStarted`/`DryRunComplete` on the `sync-event` stream.
#[tauri::command]
pub async fn preview_sync(state: State<'_, AppState>) -> Result<(), String> {
    let tx = state.active_sync_tx().await?;
    tx.send(SyncCommand::PreviewSync)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn pause_sync(state: State<'_, AppState>) -> Result<(), String> {
    let tx = state.active_sync_tx().await?;
    tx.send(SyncCommand::Pause)
        .await
        .map_err(|e| e.to_string())?;
    state.set_active_sync_paused(true).await?;
    Ok(())
}

#[tauri::command]
pub async fn resume_sync(state: State<'_, AppState>) -> Result<(), String> {
    let tx = state.active_sync_tx().await?;
    tx.send(SyncCommand::Resume)
        .await
        .map_err(|e| e.to_string())?;
    state.set_active_sync_paused(false).await?;
    Ok(())
}

#[tauri::command]
pub async fn get_sync_status(state: State<'_, AppState>) -> Result<bool, String> {
    let paused = state.active_sync_paused().await?;
    Ok(!paused)
}

#[tauri::command]
pub async fn get_recent_events(
    state: State<'_, AppState>,
    limit: Option<i64>,
) -> Result<Vec<SyncEvent>, String> {
    let db = state.active_db().await?;
    db.list_recent_sync_events(limit.unwrap_or(50))
        .await
        .map_err(|e| e.to_string())
}
