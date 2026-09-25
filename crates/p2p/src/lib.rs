pub mod client;
pub mod error;
pub mod server;
pub mod sync;
pub mod types;

use std::sync::Arc;

use tokio::sync::{mpsc, RwLock};
use tracing::{error, info, warn};

use nisaba_core::db::Db;

use crate::client::P2PClient;
use crate::error::P2PError;
use crate::server::{build_router, P2PState};
use crate::sync::{load_full_sync_payload, merge_remote_payload};
use crate::types::*;

/// Orchestrates onion service, peer syncing, and platform sync coordination.
pub struct P2PManager {
    db: Arc<Db>,
    our_onion: Arc<RwLock<Option<String>>>,
    company_secret: Arc<RwLock<Option<String>>>,
    event_tx: mpsc::Sender<P2PEvent>,
}

impl P2PManager {
    pub fn new(db: Arc<Db>, event_tx: mpsc::Sender<P2PEvent>) -> Self {
        Self {
            db,
            our_onion: Arc::new(RwLock::new(None)),
            company_secret: Arc::new(RwLock::new(None)),
            event_tx,
        }
    }

    /// Get our onion address (if service is running).
    pub async fn our_onion(&self) -> Option<String> {
        self.our_onion.read().await.clone()
    }

    /// Get the company secret.
    pub async fn company_secret(&self) -> Option<String> {
        self.company_secret.read().await.clone()
    }

    /// Start the Tor onion service in a background task.
    /// Returns once the onion address is known.
    pub async fn start_onion_service(&self, secret: String) -> Result<(), P2PError> {
        let db = self.db.clone();
        let our_onion = self.our_onion.clone();
        let company_secret_lock = self.company_secret.clone();
        let event_tx = self.event_tx.clone();
        let secret_clone = secret.clone();

        *company_secret_lock.write().await = Some(secret);

        tokio::spawn(async move {
            let db_for_router = db.clone();
            let secret_for_router = secret_clone.clone();
            let our_onion_for_router = our_onion.clone();
            let event_tx_clone = event_tx.clone();

            // The router's own copy of the onion address stays empty; the address is
            // published through `our_onion` below, which the manager reads.
            let state = Arc::new(P2PState {
                db: db_for_router,
                company_secret: secret_for_router,
                our_onion: String::new(),
            });

            let router = build_router(state);

            // `serve()` runs the service for the lifetime of the task.
            let serve_handle = tokio::spawn(async move {
                if let Err(e) = onyums::serve(router, "nisaba").await {
                    // onyums::serve returns an error on clean shutdown too
                    warn!("Onion service exited: {}", e);
                }
            });

            // The address only becomes available once the service is up, so poll for it.
            let mut attempts = 0;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                let name = onyums::get_onion_name();
                if !name.is_empty() {
                    info!(onion = %name, "Onion service ready");

                    *our_onion_for_router.write().await = Some(name.clone());

                    if let Err(e) = db.update_company_onion(&name).await {
                        error!("Failed to save onion address to DB: {}", e);
                    }

                    let _ = event_tx_clone
                        .send(P2PEvent::OnionServiceReady {
                            onion_address: name,
                        })
                        .await;
                    break;
                }

                attempts += 1;
                if attempts > 60 {
                    error!("Timed out waiting for onion service to start (120s)");
                    break;
                }
            }

            let _ = serve_handle.await;
        });

        Ok(())
    }

    /// Sync with all authorized peers.
    pub async fn sync_all_peers(&self) -> Result<(), P2PError> {
        let our_onion = self.our_onion.read().await.clone();
        let secret = self.company_secret.read().await.clone();

        let our_onion = our_onion.ok_or(P2PError::OnionNotReady)?;
        let secret = secret.ok_or(P2PError::NoCompany)?;

        let peers = self.db.list_peers().await?;

        for (addr, name, _role, is_authorized, _added, _last_seen) in &peers {
            if !is_authorized {
                continue;
            }

            let _ = self
                .event_tx
                .send(P2PEvent::SyncStarted { peer: addr.clone() })
                .await;

            match self.sync_with_peer(addr, &secret, &our_onion).await {
                Ok(summary) => {
                    self.db.update_sync_state(addr, true, None).await?;
                    self.db.update_peer_last_seen(addr).await?;

                    let _ = self
                        .event_tx
                        .send(P2PEvent::SyncCompleted {
                            peer: addr.clone(),
                            summary,
                        })
                        .await;
                }
                Err(e) => {
                    warn!(peer = %addr, name = %name, error = %e, "Sync with peer failed");
                    self.db
                        .update_sync_state(addr, false, Some(&e.to_string()))
                        .await?;

                    let _ = self
                        .event_tx
                        .send(P2PEvent::SyncFailed {
                            peer: addr.clone(),
                            error: e.to_string(),
                        })
                        .await;
                }
            }
        }

        Ok(())
    }

    /// Sync with a single peer: send our state, receive + merge theirs.
    async fn sync_with_peer(
        &self,
        peer_onion: &str,
        secret: &str,
        our_onion: &str,
    ) -> Result<MergeSummary, P2PError> {
        let payload = load_full_sync_payload(&self.db).await?;

        // Load our stable peer_id if available
        let our_peer_id = self.db.get_company_peer_id().await.ok().flatten();

        let response = P2PClient::sync_with_peer(
            peer_onion,
            secret,
            payload,
            our_onion,
            our_peer_id.as_deref(),
        )
        .await?;

        // Merge the responder's payload into our DB
        let merge_summary = merge_remote_payload(&self.db, &response.payload).await?;

        info!(
            peer = %peer_onion,
            products_created = merge_summary.products_created,
            products_updated = merge_summary.products_updated,
            "Merged response from peer"
        );

        Ok(merge_summary)
    }

    /// Check if it's our turn to perform the platform sync (round-robin).
    pub async fn is_our_turn_to_sync(&self) -> Result<bool, P2PError> {
        let our_onion = match self.our_onion.read().await.clone() {
            Some(o) => o,
            None => return Ok(true), // No P2P = always our turn
        };

        // Check if enough time has elapsed since last platform sync
        let last_sync_at = self.db.get_p2p_meta("last_platform_sync_at").await?;
        if let Some(ref ts) = last_sync_at {
            if let Ok(last) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%dT%H:%M:%S%.f%z")
                .or_else(|_| chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S"))
            {
                let elapsed = chrono::Utc::now().naive_utc().signed_duration_since(last);
                if elapsed < chrono::Duration::minutes(5) {
                    return Ok(false); // Too soon since last platform sync
                }
            }
        }

        // Build online roster: authorized peers seen within 5 minutes + ourselves
        let peers = self.db.list_peers().await?;
        let now = chrono::Utc::now();
        let threshold = chrono::Duration::minutes(5);

        let mut online_roster: Vec<String> = vec![our_onion.clone()];
        for (addr, _, _, is_authorized, _, last_seen) in &peers {
            if !is_authorized {
                continue;
            }
            if let Some(ts) = last_seen {
                if let Ok(seen) = chrono::NaiveDateTime::parse_from_str(ts, "%Y-%m-%d %H:%M:%S") {
                    if now.signed_duration_since(seen.and_utc()) < threshold {
                        online_roster.push(addr.clone());
                    }
                }
            }
        }

        online_roster.sort();
        online_roster.dedup();

        // Determine who's next after the last syncer
        let last_syncer = self.db.get_p2p_meta("last_platform_sync_by").await?;

        let next_idx = match &last_syncer {
            Some(last) => {
                match online_roster.iter().position(|a| a == last) {
                    Some(idx) => (idx + 1) % online_roster.len(),
                    None => 0, // Last syncer went offline, start from beginning
                }
            }
            None => 0,
        };

        let designated = &online_roster[next_idx];
        Ok(designated == &our_onion)
    }

    /// Record that we just completed a platform sync and push results to all peers.
    pub async fn record_platform_sync_and_push(&self) -> Result<(), P2PError> {
        let our_onion = self
            .our_onion
            .read()
            .await
            .clone()
            .ok_or(P2PError::OnionNotReady)?;

        let now = chrono::Utc::now().to_rfc3339();
        self.db.set_p2p_meta("last_platform_sync_at", &now).await?;
        self.db
            .set_p2p_meta("last_platform_sync_by", &our_onion)
            .await?;

        // Immediately push to all peers
        info!("Pushing platform sync results to all peers");
        self.sync_all_peers().await?;

        Ok(())
    }

    /// Run the periodic sync loop. Listens for commands and syncs on interval.
    pub async fn run_periodic_sync(
        self: Arc<Self>,
        mut cmd_rx: mpsc::Receiver<P2PCommand>,
        interval_secs: u64,
    ) {
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let Err(e) = self.sync_all_peers().await {
                        error!(error = %e, "Periodic P2P sync failed");
                    }
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(P2PCommand::SyncNow) => {
                            info!("Manual P2P sync triggered");
                            if let Err(e) = self.sync_all_peers().await {
                                error!(error = %e, "Manual P2P sync failed");
                            }
                        }
                        Some(P2PCommand::Stop) | None => {
                            info!("P2P sync loop stopping");
                            break;
                        }
                    }
                }
            }
        }
    }
}
