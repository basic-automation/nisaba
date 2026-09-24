use nisaba_core::sync_engine::SyncEngineEvent;
use tauri::{AppHandle, Emitter};
use tokio::sync::mpsc;
use tracing::{debug, warn};

/// Bridge sync engine events to Tauri frontend events.
/// Receives SyncEngineEvent from the mpsc channel and emits them via Tauri.
pub async fn bridge_sync_events(app: AppHandle, mut event_rx: mpsc::Receiver<SyncEngineEvent>) {
    while let Some(event) = event_rx.recv().await {
        debug!(?event, "Bridging sync event to frontend");
        if let Err(e) = app.emit("sync-event", &event) {
            warn!("Failed to emit sync event to frontend: {e}");
        }
    }
}
