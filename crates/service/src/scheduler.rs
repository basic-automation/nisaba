use std::sync::Arc;

use anyhow::Result;
use tokio::sync::mpsc;
use tokio_cron_scheduler::{Job, JobScheduler};
use tracing::{error, info};

use nisaba_core::db::Db;
use nisaba_core::sync_engine::{SharedAdapters, SyncCommand, SyncEngine, SyncEngineEvent};

/// Run the sync engine on a cron schedule.
pub async fn run_scheduled(
    db: Arc<Db>,
    adapters: SharedAdapters,
    cron_expr: &str,
    max_retries: u32,
) -> Result<()> {
    let (event_tx, mut event_rx) = mpsc::channel::<SyncEngineEvent>(64);
    let (cmd_tx, cmd_rx) = mpsc::channel::<SyncCommand>(16);

    let engine = SyncEngine::new(db, adapters, event_tx, max_retries);

    // Spawn event logger
    tokio::spawn(async move {
        while let Some(event) = event_rx.recv().await {
            match event {
                SyncEngineEvent::CycleComplete {
                    cycle_id,
                    products_synced,
                    changes_pushed,
                } => {
                    info!(
                        cycle_id,
                        products_synced,
                        changes_pushed,
                        "Sync cycle complete"
                    );
                }
                SyncEngineEvent::QuantityUpdated {
                    product_name,
                    platform,
                    old_quantity,
                    new_quantity,
                    ..
                } => {
                    info!(
                        product = product_name,
                        platform = %platform,
                        old = old_quantity,
                        new = new_quantity,
                        "Quantity updated"
                    );
                }
                SyncEngineEvent::PlatformError { platform, message } => {
                    error!(platform = %platform, message, "Platform error");
                }
                SyncEngineEvent::AuthExpired { platform } => {
                    error!(platform = %platform, "Auth expired");
                }
                SyncEngineEvent::UnmappedListingsDetected { count } => {
                    info!(count, "New unmapped listings detected");
                }
            }
        }
    });

    // Set up cron scheduler
    let sched = JobScheduler::new().await?;

    let cmd_tx_clone = cmd_tx.clone();
    sched
        .add(Job::new_async(cron_expr, move |_uuid, _lock| {
            let tx = cmd_tx_clone.clone();
            Box::pin(async move {
                if let Err(e) = tx.send(SyncCommand::SyncNow).await {
                    error!("Failed to send sync command: {e}");
                }
            })
        })?)
        .await?;

    sched.start().await?;
    info!(schedule = cron_expr, "Scheduler started");

    // Run the engine loop (blocks until cmd channel closes)
    engine.run(cmd_rx, 300).await;

    Ok(())
}
