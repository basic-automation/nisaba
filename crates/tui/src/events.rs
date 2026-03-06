use std::collections::HashMap;
use std::time::Duration;

use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use tokio::sync::mpsc;

use nisaba_core::sync_engine::SyncEngineEvent;
use nisaba_core::types::{Platform, PlatformListing};

/// All events that the TUI main loop handles.
pub enum AppEvent {
    /// A terminal input event (key press, mouse, resize).
    Terminal(Event),
    /// An event from the sync engine.
    Sync(SyncEngineEvent),
    /// A periodic UI refresh tick.
    Tick,
    /// Background fetch of unmapped listings completed.
    ListingsFetched(HashMap<Platform, Vec<PlatformListing>>),
    /// Background fetch encountered an error.
    ListingsFetchError(String),
}

/// Spawn a background task that reads terminal events and forwards them to a channel.
pub fn spawn_terminal_event_reader(tx: mpsc::Sender<AppEvent>) {
    tokio::spawn(async move {
        let mut reader = EventStream::new();
        while let Some(event) = reader.next().await {
            if let Ok(event) = event {
                if tx.send(AppEvent::Terminal(event)).await.is_err() {
                    break;
                }
            }
        }
    });
}

/// Spawn a task that bridges sync engine events into AppEvents.
pub fn spawn_sync_event_bridge(
    mut sync_rx: mpsc::Receiver<SyncEngineEvent>,
    tx: mpsc::Sender<AppEvent>,
) {
    tokio::spawn(async move {
        while let Some(event) = sync_rx.recv().await {
            if tx.send(AppEvent::Sync(event)).await.is_err() {
                break;
            }
        }
    });
}

/// Spawn a tick timer.
pub fn spawn_tick(tx: mpsc::Sender<AppEvent>, interval: Duration) {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(interval);
        loop {
            tick.tick().await;
            if tx.send(AppEvent::Tick).await.is_err() {
                break;
            }
        }
    });
}
