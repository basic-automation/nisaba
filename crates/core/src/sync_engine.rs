use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::db::Db;
use crate::error::SyncError;
use crate::traits::PlatformAdapter;
use crate::types::{NewSyncEvent, Platform, PlatformMapping, QuantityDelta};

// ── Shared adapter map type ───────────────────────────────────

/// Shared, rebuildable map of platform adapters. Both the sync engine and the
/// TUI hold a reference; the TUI can swap in a fresh map when the config changes.
pub type SharedAdapters = Arc<RwLock<HashMap<Platform, Arc<dyn PlatformAdapter>>>>;

// ── Messages between sync engine and TUI ──────────────────────

/// Events sent from the sync engine to the UI (Tauri frontend).
#[derive(Debug, Clone, serde::Serialize)]
pub enum SyncEngineEvent {
    /// A full sync cycle completed successfully.
    CycleComplete {
        cycle_id: String,
        products_synced: usize,
        changes_pushed: usize,
    },
    /// A quantity was updated on a target platform.
    QuantityUpdated {
        product_id: String,
        product_name: String,
        platform: Platform,
        old_quantity: i64,
        new_quantity: i64,
    },
    /// An error occurred on a specific platform.
    PlatformError { platform: Platform, message: String },
    /// Auth expired / needs refresh on a platform.
    AuthExpired { platform: Platform },
    /// New unmapped listings were detected.
    UnmappedListingsDetected { count: usize },
}

/// Commands sent from the TUI to the sync engine.
#[derive(Debug, Clone)]
pub enum SyncCommand {
    /// Trigger an immediate sync cycle.
    SyncNow,
    /// Pause the sync scheduler.
    Pause,
    /// Resume the sync scheduler.
    Resume,
    /// Reload config and adapters.
    Reload,
}

// ── Sync Engine ───────────────────────────────────────────────

/// Callback invoked after each sync cycle to refresh vendor plugin data.
/// The closure returns a future that runs all approved vendor plugins,
/// updates product_vendor_data, and recalculates quantities.
pub type VendorSyncFn = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

pub struct SyncEngine {
    db: Arc<Db>,
    adapters: SharedAdapters,
    event_tx: mpsc::Sender<SyncEngineEvent>,
    max_retries: u32,
    vendor_sync_fn: Option<VendorSyncFn>,
}

impl SyncEngine {
    pub fn new(
        db: Arc<Db>,
        adapters: SharedAdapters,
        event_tx: mpsc::Sender<SyncEngineEvent>,
        max_retries: u32,
    ) -> Self {
        Self {
            db,
            adapters,
            event_tx,
            max_retries,
            vendor_sync_fn: None,
        }
    }

    /// Attach a callback to run vendor plugin syncs after each platform cycle.
    pub fn with_vendor_sync(mut self, f: VendorSyncFn) -> Self {
        self.vendor_sync_fn = Some(f);
        self
    }

    /// Run a single sync cycle: poll → detect → resolve → push → record.
    pub async fn run_cycle(&self) -> Result<(usize, usize), SyncError> {
        let cycle_id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        info!(cycle_id = %cycle_id, "Starting sync cycle");

        // Snapshot the current adapters for this cycle
        let adapters = self.adapters.read().await;

        if adapters.is_empty() {
            info!("No platforms enabled, skipping sync cycle");
            return Ok((0, 0));
        }

        // Get all tracked products with their active mappings
        let products = self.db.list_products().await?;
        let all_mappings = self.db.list_all_active_mappings().await?;

        // Group mappings by product
        let mut mappings_by_product: HashMap<String, Vec<PlatformMapping>> = HashMap::new();
        for mapping in &all_mappings {
            mappings_by_product
                .entry(mapping.product_id.clone())
                .or_default()
                .push(mapping.clone());
        }

        // ── Step 1: Poll all enabled platforms ────────────────
        let mut platform_quantities: HashMap<(Platform, String), i64> = HashMap::new();

        for (platform, adapter) in adapters.iter() {
            match self.poll_with_retry(adapter.as_ref()).await {
                Ok(items) => {
                    for item in items {
                        platform_quantities
                            .insert((*platform, item.platform_item_id.clone()), item.quantity);
                    }
                }
                Err(e) => {
                    warn!(platform = %platform, error = %e, "Failed to poll platform");
                    let _ = self
                        .event_tx
                        .send(SyncEngineEvent::PlatformError {
                            platform: *platform,
                            message: e.to_string(),
                        })
                        .await;
                }
            }
        }

        // ── Step 1b: Detect sales on stock-mode platforms ──────
        // For platforms that use stock modes (e.g. XMR Bazaar), detect sales
        // via scraping instead of quantity polling.
        let mut injected_deltas: HashMap<String, Vec<QuantityDelta>> = HashMap::new();

        for (platform, adapter) in adapters.iter() {
            if !adapter.capabilities().has_stock_mode_inventory {
                continue;
            }

            match adapter.detect_sales().await {
                Ok(sales) => {
                    for sale in sales {
                        let order_id = sale.order_id.as_deref().unwrap_or("");

                        // DB-level deduplication: skip already-processed orders
                        if !order_id.is_empty() && self.db.is_xmr_order_processed(order_id).await? {
                            debug!(
                                order_id,
                                platform = %platform,
                                "Skipping already-processed order"
                            );
                            continue;
                        }

                        // Look up which product this listing maps to
                        let mapping = self
                            .db
                            .get_mapping_by_platform_item(platform.as_str(), &sale.platform_item_id)
                            .await?;

                        let Some(mapping) = mapping else {
                            warn!(
                                platform = %platform,
                                item_id = %sale.platform_item_id,
                                "Sale detected for unmapped listing, skipping"
                            );
                            continue;
                        };

                        info!(
                            platform = %platform,
                            product_id = %mapping.product_id,
                            item_id = %sale.platform_item_id,
                            units = sale.units_sold,
                            order_id,
                            "Injecting sale delta from stock-mode platform"
                        );

                        // Inject a negative delta for this sale
                        injected_deltas
                            .entry(mapping.product_id.clone())
                            .or_default()
                            .push(QuantityDelta {
                                product_id: mapping.product_id.clone(),
                                platform: *platform,
                                previous: 0,
                                current: 0,
                                delta: -sale.units_sold,
                            });

                        // Mark order as processed in DB
                        if !order_id.is_empty() {
                            self.db
                                .mark_xmr_order_processed(
                                    order_id,
                                    Some(&mapping.product_id),
                                    &sale.platform_item_id,
                                    sale.units_sold,
                                )
                                .await?;
                        }
                    }
                }
                Err(e) => {
                    warn!(platform = %platform, error = %e, "Failed to detect sales");
                    let _ = self
                        .event_tx
                        .send(SyncEngineEvent::PlatformError {
                            platform: *platform,
                            message: e.to_string(),
                        })
                        .await;
                }
            }
        }

        let mut products_synced = 0;
        let mut changes_pushed = 0;

        for product in &products {
            if !product.is_tracked {
                continue;
            }

            let Some(mappings) = mappings_by_product.get(&product.id) else {
                continue;
            };

            // ── Step 2: Resolve quantity across normal platforms ──
            // Strategy: if any platform's stock INCREASED since last snapshot
            // (restock), use max minus decreases on other platforms. Otherwise
            // use min (conservative — prevents overselling).
            let mut min_qty: Option<i64> = None;
            let mut max_qty: Option<i64> = None;
            let mut any_increased = false;
            let mut total_decreases: i64 = 0;

            for mapping in mappings {
                if !adapters.contains_key(&mapping.platform) {
                    continue;
                }

                // Skip stock-mode platforms (XMR Bazaar) in quantity computation
                if adapters
                    .get(&mapping.platform)
                    .map(|a| a.capabilities().has_stock_mode_inventory)
                    .unwrap_or(false)
                {
                    continue;
                }

                let platform_key = (mapping.platform, mapping.platform_item_id.clone());
                let Some(&current_qty) = platform_quantities.get(&platform_key) else {
                    continue;
                };

                // Compare against last snapshot to detect increases/decreases
                let snapshot = self
                    .db
                    .get_snapshot(&product.id, mapping.platform.as_str())
                    .await?;

                if let Some(snap) = &snapshot {
                    let delta = current_qty - snap.last_known_quantity;
                    if delta > 0 {
                        any_increased = true;
                    } else if delta < 0 {
                        total_decreases += delta; // negative
                    }
                }

                min_qty = Some(min_qty.map_or(current_qty, |m: i64| m.min(current_qty)));
                max_qty = Some(max_qty.map_or(current_qty, |m: i64| m.max(current_qty)));

                // Update the polled snapshot
                self.db
                    .upsert_snapshot(&product.id, mapping.platform.as_str(), current_qty, &now)
                    .await?;
            }

            // If restock detected, use max minus sales on other platforms.
            // Otherwise use min (lowest-stock-wins for sales).
            let base_qty = if any_increased {
                max_qty.map(|m| (m + total_decreases).max(0))
            } else {
                min_qty
            };

            // Apply XMR Bazaar sale decrements (from Step 1b)
            let xmr_sales: i64 = injected_deltas
                .remove(&product.id)
                .map(|ds| ds.iter().map(|d| d.delta).sum())
                .unwrap_or(0); // negative number

            // New canonical = base qty + XMR sales (negative), floor at 0
            let new_canonical = match base_qty {
                Some(b) => (b + xmr_sales).max(0),
                None => (product.quantity + xmr_sales).max(0), // no platforms reported
            };

            if new_canonical == product.quantity {
                // Record history even when no change (for time-series continuity)
                self.db
                    .record_inventory_history(&product.id, product.quantity, &cycle_id)
                    .await?;
                continue;
            }

            info!(
                product = %product.name,
                old = product.quantity,
                new = new_canonical,
                strategy = if any_increased { "restock (max - decreases)" } else { "sales (min)" },
                base = ?base_qty,
                xmr_sales,
                "Resolved quantity change"
            );

            // Update canonical quantity in DB
            self.db
                .update_product_quantity(&product.id, new_canonical)
                .await?;

            // ── Step 4: Push to all mapped platforms ──────────
            for mapping in mappings {
                // Skip disabled platforms
                let Some(adapter) = adapters.get(&mapping.platform) else {
                    continue;
                };

                // Stock-mode-aware push logic
                if adapter.capabilities().has_stock_mode_inventory {
                    let snapshot = self
                        .db
                        .get_snapshot(&product.id, mapping.platform.as_str())
                        .await?;

                    let current_tag = snapshot
                        .as_ref()
                        .and_then(|s| s.version_tag.as_deref())
                        .unwrap_or("unknown");

                    let desired = if new_canonical > 0 {
                        "active"
                    } else {
                        "out_of_stock"
                    };

                    if current_tag == desired {
                        debug!(
                            product = %product.name,
                            platform = %mapping.platform,
                            state = desired,
                            "Stock mode already correct, skipping push"
                        );
                        continue;
                    }

                    match adapter
                        .set_quantity(&mapping.platform_item_id, new_canonical)
                        .await
                    {
                        Ok(()) => {
                            changes_pushed += 1;

                            // Update version_tag to reflect new stock mode state
                            self.db
                                .update_snapshot_version_tag(
                                    &product.id,
                                    mapping.platform.as_str(),
                                    desired,
                                )
                                .await?;

                            // Ensure snapshot exists
                            self.db
                                .upsert_snapshot(
                                    &product.id,
                                    mapping.platform.as_str(),
                                    new_canonical,
                                    &now,
                                )
                                .await?;

                            self.db
                                .insert_sync_event(&NewSyncEvent {
                                    product_id: &product.id,
                                    source_platform: "sync_engine",
                                    target_platform: mapping.platform.as_str(),
                                    old_quantity: product.quantity,
                                    new_quantity: new_canonical,
                                    event_type: "stock_mode_change",
                                    sync_cycle_id: &cycle_id,
                                    error_message: None,
                                })
                                .await?;

                            let _ = self
                                .event_tx
                                .send(SyncEngineEvent::QuantityUpdated {
                                    product_id: product.id.clone(),
                                    product_name: product.name.clone(),
                                    platform: mapping.platform,
                                    old_quantity: product.quantity,
                                    new_quantity: new_canonical,
                                })
                                .await;
                        }
                        Err(e) => {
                            warn!(
                                product = %product.name,
                                platform = %mapping.platform,
                                error = %e,
                                "Failed to update stock mode"
                            );

                            self.db
                                .insert_sync_event(&NewSyncEvent {
                                    product_id: &product.id,
                                    source_platform: "sync_engine",
                                    target_platform: mapping.platform.as_str(),
                                    old_quantity: product.quantity,
                                    new_quantity: new_canonical,
                                    event_type: "push_error",
                                    sync_cycle_id: &cycle_id,
                                    error_message: Some(&e.to_string()),
                                })
                                .await?;

                            let _ = self
                                .event_tx
                                .send(SyncEngineEvent::PlatformError {
                                    platform: mapping.platform,
                                    message: e.to_string(),
                                })
                                .await;
                        }
                    }

                    continue;
                }

                // Normal (numeric quantity) push logic
                let platform_key = (mapping.platform, mapping.platform_item_id.clone());
                let current_on_platform = platform_quantities.get(&platform_key).copied();

                // Only push if the platform doesn't already have the correct quantity
                if current_on_platform == Some(new_canonical) {
                    continue;
                }

                match adapter
                    .set_quantity(&mapping.platform_item_id, new_canonical)
                    .await
                {
                    Ok(()) => {
                        changes_pushed += 1;

                        // Update snapshot to pushed value (prevents sync loop)
                        self.db
                            .update_snapshot_pushed(
                                &product.id,
                                mapping.platform.as_str(),
                                new_canonical,
                                &now,
                            )
                            .await?;

                        // Log the sync event
                        self.db
                            .insert_sync_event(&NewSyncEvent {
                                product_id: &product.id,
                                source_platform: "sync_engine",
                                target_platform: mapping.platform.as_str(),
                                old_quantity: product.quantity,
                                new_quantity: new_canonical,
                                event_type: "quantity_change",
                                sync_cycle_id: &cycle_id,
                                error_message: None,
                            })
                            .await?;

                        let _ = self
                            .event_tx
                            .send(SyncEngineEvent::QuantityUpdated {
                                product_id: product.id.clone(),
                                product_name: product.name.clone(),
                                platform: mapping.platform,
                                old_quantity: product.quantity,
                                new_quantity: new_canonical,
                            })
                            .await;
                    }
                    Err(e) => {
                        warn!(
                            product = %product.name,
                            platform = %mapping.platform,
                            error = %e,
                            "Failed to push quantity"
                        );

                        self.db
                            .insert_sync_event(&NewSyncEvent {
                                product_id: &product.id,
                                source_platform: "sync_engine",
                                target_platform: mapping.platform.as_str(),
                                old_quantity: product.quantity,
                                new_quantity: new_canonical,
                                event_type: "push_error",
                                sync_cycle_id: &cycle_id,
                                error_message: Some(&e.to_string()),
                            })
                            .await?;

                        let _ = self
                            .event_tx
                            .send(SyncEngineEvent::PlatformError {
                                platform: mapping.platform,
                                message: e.to_string(),
                            })
                            .await;
                    }
                }
            }

            // ── Step 5: Record history ────────────────────────
            self.db
                .record_inventory_history(&product.id, new_canonical, &cycle_id)
                .await?;

            products_synced += 1;
        }

        // Drop the read lock before sending event
        drop(adapters);

        info!(
            cycle_id = %cycle_id,
            products_synced,
            changes_pushed,
            "Sync cycle complete"
        );

        let _ = self
            .event_tx
            .send(SyncEngineEvent::CycleComplete {
                cycle_id,
                products_synced,
                changes_pushed,
            })
            .await;

        Ok((products_synced, changes_pushed))
    }

    /// Poll a platform with retry logic.
    async fn poll_with_retry(
        &self,
        adapter: &dyn PlatformAdapter,
    ) -> Result<Vec<crate::types::PlatformInventoryItem>, SyncError> {
        let mut last_err = None;

        for attempt in 0..=self.max_retries {
            if attempt > 0 {
                let delay = std::time::Duration::from_millis(500 * 2u64.pow(attempt - 1));
                debug!(
                    platform = %adapter.platform(),
                    attempt,
                    delay_ms = delay.as_millis(),
                    "Retrying poll"
                );
                tokio::time::sleep(delay).await;
            }

            match adapter.fetch_inventory().await {
                Ok(items) => return Ok(items),
                Err(e) => {
                    warn!(
                        platform = %adapter.platform(),
                        attempt,
                        error = %e,
                        "Poll attempt failed"
                    );

                    // Check if auth is expired and try to refresh
                    if matches!(e, SyncError::AuthError { .. }) {
                        if let Err(auth_err) = adapter.refresh_auth().await {
                            warn!(
                                platform = %adapter.platform(),
                                error = %auth_err,
                                "Auth refresh failed"
                            );
                            let _ = self
                                .event_tx
                                .send(SyncEngineEvent::AuthExpired {
                                    platform: adapter.platform(),
                                })
                                .await;
                        }
                    }

                    last_err = Some(e);
                }
            }
        }

        Err(last_err.unwrap_or_else(|| SyncError::Other("Poll failed with no error".into())))
    }

    /// Run the engine loop, listening for commands and executing cycles on a timer.
    pub async fn run(self, mut cmd_rx: mpsc::Receiver<SyncCommand>, interval_secs: u64) {
        // Startup: recalculate all vendor-sourced product quantities so that
        // persisted product_vendor_data is applied BEFORE the first sync cycle
        // polls platforms and potentially overwrites quantities.
        match self.db.recalc_all_vendor_sourced_products().await {
            Ok(n) if n > 0 => info!(
                count = n,
                "Startup: recalculated vendor-sourced product quantities"
            ),
            Ok(_) => {}
            Err(e) => warn!(error = %e, "Startup: failed to recalculate vendor-sourced products"),
        }

        let mut paused = false;
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        // Don't try to catch up missed ticks
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if !paused {
                        if let Err(e) = self.run_cycle().await {
                            error!(error = %e, "Sync cycle failed");
                        }
                        if let Some(ref sync_fn) = self.vendor_sync_fn {
                            (sync_fn)().await;
                        }
                    }
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(SyncCommand::SyncNow) => {
                            info!("Manual sync triggered");
                            if let Err(e) = self.run_cycle().await {
                                error!(error = %e, "Manual sync cycle failed");
                            }
                            if let Some(ref sync_fn) = self.vendor_sync_fn {
                                (sync_fn)().await;
                            }
                        }
                        Some(SyncCommand::Pause) => {
                            info!("Sync paused");
                            paused = true;
                        }
                        Some(SyncCommand::Resume) => {
                            info!("Sync resumed");
                            paused = false;
                        }
                        Some(SyncCommand::Reload) => {
                            info!("Config reloaded — adapters will be used on next cycle");
                        }
                        None => {
                            info!("Command channel closed, shutting down sync engine");
                            break;
                        }
                    }
                }
            }
        }
    }
}
