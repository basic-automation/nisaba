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
    /// A change a dry-run cycle resolved but deliberately did not make.
    DryRunChange {
        product_id: String,
        product_name: String,
        platform: Platform,
        current_quantity: i64,
        planned_quantity: i64,
    },
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
    dry_run: bool,
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
            dry_run: false,
        }
    }

    /// Resolve every cycle as normal but make no changes at all: no
    /// `set_quantity` against any platform, and no local writes either.
    ///
    /// The local half matters as much as the remote half. A dry run that still
    /// marked XMR Bazaar orders processed, moved snapshots, or updated the
    /// canonical quantity would leave the database believing work had happened,
    /// and the next real cycle would skip exactly the changes the dry run was
    /// asked to preview. Each change it *would* have made is reported as a
    /// [`SyncEngineEvent::DryRunChange`], and `run_cycle`'s second return value
    /// counts them.
    pub fn dry_run(mut self) -> Self {
        self.dry_run = true;
        self
    }

    /// Whether this engine is in dry-run mode.
    pub fn is_dry_run(&self) -> bool {
        self.dry_run
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

                        // Mark order as processed in DB. A dry run must not:
                        // consuming the order here would make the real cycle
                        // that follows skip the very sale it previewed.
                        if !order_id.is_empty() && !self.dry_run {
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
                if !self.dry_run {
                    self.db
                        .upsert_snapshot(&product.id, mapping.platform.as_str(), current_qty, &now)
                        .await?;
                }
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
                if !self.dry_run {
                    self.db
                        .record_inventory_history(&product.id, product.quantity, &cycle_id)
                        .await?;
                }
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
            if !self.dry_run {
                self.db
                    .update_product_quantity(&product.id, new_canonical)
                    .await?;
            }

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

                    if self.dry_run {
                        changes_pushed += 1;
                        let _ = self
                            .event_tx
                            .send(SyncEngineEvent::DryRunChange {
                                product_id: product.id.clone(),
                                product_name: product.name.clone(),
                                platform: mapping.platform,
                                current_quantity: product.quantity,
                                planned_quantity: new_canonical,
                            })
                            .await;
                        continue;
                    }

                    match adapter
                        .set_quantity(&mapping.platform_item_id, new_canonical)
                        .await
                    {
                        Ok(()) => {
                            changes_pushed += 1;

                            // Create the snapshot row first: `update_snapshot_version_tag`
                            // is a plain UPDATE, so tagging before the row exists
                            // silently drops the tag. The next cycle would then read
                            // `version_tag = NULL`, treat the state as unknown, and
                            // re-push the stock mode to the live listing — every
                            // cycle, forever.
                            self.db
                                .upsert_snapshot(
                                    &product.id,
                                    mapping.platform.as_str(),
                                    new_canonical,
                                    &now,
                                )
                                .await?;

                            // Record the stock mode we just pushed.
                            self.db
                                .update_snapshot_version_tag(
                                    &product.id,
                                    mapping.platform.as_str(),
                                    desired,
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

                if self.dry_run {
                    changes_pushed += 1;
                    let _ = self
                        .event_tx
                        .send(SyncEngineEvent::DryRunChange {
                            product_id: product.id.clone(),
                            product_name: product.name.clone(),
                            platform: mapping.platform,
                            current_quantity: current_on_platform.unwrap_or(product.quantity),
                            planned_quantity: new_canonical,
                        })
                        .await;
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
            if !self.dry_run {
                self.db
                    .record_inventory_history(&product.id, new_canonical, &cycle_id)
                    .await?;
            }

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
        if self.dry_run {
            info!("Dry run: skipping startup vendor-sourced quantity recalculation");
        } else {
            match self.db.recalc_all_vendor_sourced_products().await {
                Ok(n) if n > 0 => info!(
                    count = n,
                    "Startup: recalculated vendor-sourced product quantities"
                ),
                Ok(_) => {}
                Err(e) => {
                    warn!(error = %e, "Startup: failed to recalculate vendor-sourced products")
                }
            }
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
                            if !self.dry_run {
                                (sync_fn)().await;
                            }
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
                                if !self.dry_run {
                                    (sync_fn)().await;
                                }
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

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    use async_trait::async_trait;

    use super::*;
    use crate::types::{
        PlatformCapabilities, PlatformInventoryItem, PlatformListing, Product, SaleDetection,
        SyncEvent,
    };

    // ── Fake adapter ──────────────────────────────────────────

    /// One scripted answer to a `fetch_inventory` call. Kept as a cloneable
    /// enum rather than a `Result<_, SyncError>` because `SyncError` is not
    /// `Clone` and the tests need to replay the *same* error variant.
    #[derive(Clone)]
    enum Poll {
        Items(Vec<PlatformInventoryItem>),
        Network(&'static str),
        Auth(&'static str),
    }

    impl Poll {
        fn items(pairs: &[(&str, i64)]) -> Self {
            Poll::Items(
                pairs
                    .iter()
                    .map(|(id, quantity)| PlatformInventoryItem {
                        platform_item_id: (*id).to_string(),
                        quantity: *quantity,
                    })
                    .collect(),
            )
        }

        fn into_result(self, platform: Platform) -> Result<Vec<PlatformInventoryItem>, SyncError> {
            match self {
                Poll::Items(items) => Ok(items),
                Poll::Network(message) => Err(SyncError::NetworkError {
                    platform,
                    message: message.into(),
                }),
                Poll::Auth(message) => Err(SyncError::AuthError {
                    platform,
                    message: message.into(),
                }),
            }
        }
    }

    /// A `PlatformAdapter` that answers from canned data and records every
    /// write it is asked to make. Nothing here touches a network or a real
    /// marketplace: `set_quantity` only appends to `pushes`.
    struct FakeAdapter {
        platform: Platform,
        capabilities: PlatformCapabilities,
        /// One entry consumed per poll. The last entry repeats once the queue
        /// is down to one, so "fails twice then succeeds" and "always fails"
        /// are both expressible.
        polls: Mutex<Vec<Poll>>,
        sales: Mutex<Vec<SaleDetection>>,
        /// When set, every `set_quantity` fails with this message.
        push_error: Option<String>,
        pushes: Mutex<Vec<(String, i64)>>,
        poll_calls: AtomicUsize,
        refresh_calls: AtomicUsize,
        detect_sales_calls: AtomicUsize,
    }

    impl FakeAdapter {
        fn new(platform: Platform) -> Self {
            Self {
                platform,
                capabilities: PlatformCapabilities::default(),
                polls: Mutex::new(vec![Poll::Items(Vec::new())]),
                sales: Mutex::new(Vec::new()),
                push_error: None,
                pushes: Mutex::new(Vec::new()),
                poll_calls: AtomicUsize::new(0),
                refresh_calls: AtomicUsize::new(0),
                detect_sales_calls: AtomicUsize::new(0),
            }
        }

        /// Report these quantities on every poll.
        fn with_quantities(self, pairs: &[(&str, i64)]) -> Self {
            *self.polls.lock().unwrap() = vec![Poll::items(pairs)];
            self
        }

        /// Answer polls from this script, one entry per call.
        fn with_poll_script(self, script: Vec<Poll>) -> Self {
            *self.polls.lock().unwrap() = script;
            self
        }

        fn with_stock_mode(mut self) -> Self {
            self.capabilities.has_stock_mode_inventory = true;
            self
        }

        fn with_sales(self, sales: Vec<SaleDetection>) -> Self {
            *self.sales.lock().unwrap() = sales;
            self
        }

        fn failing_pushes(mut self, message: &str) -> Self {
            self.push_error = Some(message.to_string());
            self
        }

        fn pushes(&self) -> Vec<(String, i64)> {
            self.pushes.lock().unwrap().clone()
        }

        fn arm_sales(&self, sales: Vec<SaleDetection>) {
            *self.sales.lock().unwrap() = sales;
        }
    }

    #[async_trait]
    impl PlatformAdapter for FakeAdapter {
        fn as_any(&self) -> &dyn std::any::Any {
            self
        }

        fn platform(&self) -> Platform {
            self.platform
        }

        async fn fetch_inventory(&self) -> Result<Vec<PlatformInventoryItem>, SyncError> {
            self.poll_calls.fetch_add(1, Ordering::SeqCst);
            let next = {
                let mut queue = self.polls.lock().unwrap();
                if queue.len() > 1 {
                    queue.remove(0)
                } else {
                    queue.first().cloned().unwrap_or(Poll::Items(Vec::new()))
                }
            };
            next.into_result(self.platform)
        }

        async fn fetch_all_listings(&self) -> Result<Vec<PlatformListing>, SyncError> {
            Ok(Vec::new())
        }

        async fn set_quantity(&self, platform_item_id: &str, qty: i64) -> Result<(), SyncError> {
            if let Some(message) = &self.push_error {
                return Err(SyncError::ApiError {
                    platform: self.platform,
                    message: message.clone(),
                });
            }
            self.pushes
                .lock()
                .unwrap()
                .push((platform_item_id.to_string(), qty));
            Ok(())
        }

        async fn is_authenticated(&self) -> bool {
            true
        }

        async fn refresh_auth(&self) -> Result<(), SyncError> {
            self.refresh_calls.fetch_add(1, Ordering::SeqCst);
            Err(SyncError::AuthError {
                platform: self.platform,
                message: "fake adapter cannot refresh".into(),
            })
        }

        async fn detect_sales(&self) -> Result<Vec<SaleDetection>, SyncError> {
            self.detect_sales_calls.fetch_add(1, Ordering::SeqCst);
            // Sales are reported once and then consumed, which is how a scrape
            // of a "recent orders" page behaves after the order ages out.
            Ok(std::mem::take(&mut *self.sales.lock().unwrap()))
        }

        fn capabilities(&self) -> PlatformCapabilities {
            self.capabilities.clone()
        }
    }

    // ── Harness ───────────────────────────────────────────────

    /// A sync engine wired to a fresh temp-file DB and a set of fake adapters.
    struct Harness {
        db: Arc<Db>,
        engine: SyncEngine,
        events: mpsc::Receiver<SyncEngineEvent>,
        adapters: Vec<Arc<FakeAdapter>>,
        _dir: tempfile::TempDir,
    }

    impl Harness {
        /// Turn this harness's engine into a dry-run engine. Call before
        /// `run_cycle`; everything else about the harness is unchanged.
        fn into_dry_run(mut self) -> Self {
            self.engine = self.engine.dry_run();
            self
        }

        async fn new(fakes: Vec<FakeAdapter>, max_retries: u32) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let path = dir.path().join("sync-engine-test.db");
            let db = Db::open(path.to_str().unwrap()).await.unwrap();
            db.migrate().await.unwrap();
            let db = Arc::new(db);

            let mut map: HashMap<Platform, Arc<dyn PlatformAdapter>> = HashMap::new();
            let mut handles = Vec::new();
            for fake in fakes {
                let fake = Arc::new(fake);
                map.insert(fake.platform, fake.clone() as Arc<dyn PlatformAdapter>);
                handles.push(fake);
            }

            // Buffer generously: `run_cycle` awaits on send, so a full channel
            // would deadlock the cycle rather than fail the test.
            let (tx, rx) = mpsc::channel(256);
            let adapters: SharedAdapters = Arc::new(RwLock::new(map));

            Self {
                engine: SyncEngine::new(db.clone(), adapters, tx, max_retries),
                db,
                events: rx,
                adapters: handles,
                _dir: dir,
            }
        }

        fn adapter(&self, platform: Platform) -> Arc<FakeAdapter> {
            self.adapters
                .iter()
                .find(|a| a.platform == platform)
                .expect("adapter not registered in harness")
                .clone()
        }

        async fn add_product(&self, id: &str, quantity: i64, mappings: &[(Platform, &str)]) {
            self.add_product_inner(id, quantity, true, mappings).await;
        }

        async fn add_untracked_product(
            &self,
            id: &str,
            quantity: i64,
            mappings: &[(Platform, &str)],
        ) {
            self.add_product_inner(id, quantity, false, mappings).await;
        }

        async fn add_product_inner(
            &self,
            id: &str,
            quantity: i64,
            is_tracked: bool,
            mappings: &[(Platform, &str)],
        ) {
            self.db
                .insert_product(&Product {
                    id: id.to_string(),
                    canonical_sku: format!("SKU-{id}"),
                    name: format!("Product {id}"),
                    quantity,
                    low_stock_threshold: None,
                    is_tracked,
                    has_variants: false,
                    created_at: String::new(),
                    updated_at: String::new(),
                })
                .await
                .unwrap();

            for (platform, item_id) in mappings {
                self.db
                    .insert_mapping(id, platform.as_str(), item_id, None, "v1")
                    .await
                    .unwrap();
            }
        }

        fn drain_events(&mut self) -> Vec<SyncEngineEvent> {
            let mut out = Vec::new();
            while let Ok(event) = self.events.try_recv() {
                out.push(event);
            }
            out
        }

        async fn quantity(&self, product_id: &str) -> i64 {
            self.db
                .get_product(product_id)
                .await
                .unwrap()
                .expect("product missing")
                .quantity
        }

        async fn events_of_type(&self, product_id: &str, event_type: &str) -> Vec<SyncEvent> {
            self.db
                .list_recent_sync_events(200)
                .await
                .unwrap()
                .into_iter()
                .filter(|e| e.product_id == product_id && e.event_type == event_type)
                .collect()
        }
    }

    fn sale(item_id: &str, units: i64, order_id: &str) -> SaleDetection {
        SaleDetection {
            platform_item_id: item_id.to_string(),
            units_sold: units,
            order_id: Some(order_id.to_string()),
            sold_at: None,
        }
    }

    // ── Quantity resolution ───────────────────────────────────

    #[tokio::test]
    async fn empty_adapter_map_is_a_no_op_cycle() {
        let mut harness = Harness::new(Vec::new(), 0).await;
        harness.add_product("p1", 5, &[]).await;

        let (synced, pushed) = harness.engine.run_cycle().await.unwrap();

        assert_eq!((synced, pushed), (0, 0));
        // The engine returns before emitting, so there is no CycleComplete.
        assert!(harness.drain_events().is_empty());
        assert_eq!(harness.quantity("p1").await, 5);
    }

    #[tokio::test]
    async fn lowest_stock_wins_when_no_platform_restocked() {
        // eBay says 3, Squarespace says 7, and neither has a snapshot to
        // compare against, so nothing counts as a restock: the conservative
        // minimum (3) becomes canonical and is pushed to Squarespace only.
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 3)]),
                FakeAdapter::new(Platform::Squarespace).with_quantities(&[("S-1", 7)]),
            ],
            0,
        )
        .await;
        harness
            .add_product(
                "p1",
                10,
                &[(Platform::Ebay, "E-1"), (Platform::Squarespace, "S-1")],
            )
            .await;

        let (synced, pushed) = harness.engine.run_cycle().await.unwrap();

        assert_eq!(synced, 1);
        assert_eq!(
            pushed, 1,
            "only Squarespace is out of step with the resolved 3"
        );
        assert_eq!(harness.quantity("p1").await, 3);
        assert_eq!(
            harness.adapter(Platform::Squarespace).pushes(),
            vec![("S-1".to_string(), 3)]
        );
        assert!(
            harness.adapter(Platform::Ebay).pushes().is_empty(),
            "eBay already reported 3, so it must not be written to"
        );

        let recorded = harness.events_of_type("p1", "quantity_change").await;
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].target_platform, "squarespace");
        assert_eq!(
            (recorded[0].old_quantity, recorded[0].new_quantity),
            (10, 3)
        );
    }

    #[tokio::test]
    async fn a_restock_raises_canonical_net_of_sales_elsewhere() {
        // Snapshots say both platforms held 5. This cycle eBay reports 12 (a
        // restock, +7) and Squarespace reports 3 (two sold, -2). Restock wins,
        // so canonical = max(12) + decreases(-2) = 10.
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 12)]),
                FakeAdapter::new(Platform::Squarespace).with_quantities(&[("S-1", 3)]),
            ],
            0,
        )
        .await;
        harness
            .add_product(
                "p1",
                5,
                &[(Platform::Ebay, "E-1"), (Platform::Squarespace, "S-1")],
            )
            .await;
        harness
            .db
            .upsert_snapshot("p1", "ebay", 5, "2026-01-01 00:00:00")
            .await
            .unwrap();
        harness
            .db
            .upsert_snapshot("p1", "squarespace", 5, "2026-01-01 00:00:00")
            .await
            .unwrap();

        harness.engine.run_cycle().await.unwrap();

        assert_eq!(harness.quantity("p1").await, 10);
        assert_eq!(
            harness.adapter(Platform::Ebay).pushes(),
            vec![("E-1".to_string(), 10)]
        );
        assert_eq!(
            harness.adapter(Platform::Squarespace).pushes(),
            vec![("S-1".to_string(), 10)]
        );
    }

    #[tokio::test]
    async fn resolved_quantity_never_goes_negative() {
        // A restock reading smaller than the decreases it is netted against
        // must floor at 0, never push a negative quantity to a platform.
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 1)]),
                FakeAdapter::new(Platform::Squarespace).with_quantities(&[("S-1", 0)]),
            ],
            0,
        )
        .await;
        harness
            .add_product(
                "p1",
                20,
                &[(Platform::Ebay, "E-1"), (Platform::Squarespace, "S-1")],
            )
            .await;
        harness
            .db
            .upsert_snapshot("p1", "ebay", 0, "2026-01-01 00:00:00")
            .await
            .unwrap();
        harness
            .db
            .upsert_snapshot("p1", "squarespace", 9, "2026-01-01 00:00:00")
            .await
            .unwrap();

        harness.engine.run_cycle().await.unwrap();

        assert_eq!(harness.quantity("p1").await, 0);
        for (item, pushed) in harness.adapter(Platform::Ebay).pushes() {
            assert!(pushed >= 0, "pushed {pushed} to {item}");
        }
    }

    #[tokio::test]
    async fn an_agreeing_cycle_pushes_nothing_but_still_records_history() {
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 4)])],
            0,
        )
        .await;
        harness
            .add_product("p1", 4, &[(Platform::Ebay, "E-1")])
            .await;

        let (synced, pushed) = harness.engine.run_cycle().await.unwrap();

        assert_eq!((synced, pushed), (0, 0));
        assert!(harness.adapter(Platform::Ebay).pushes().is_empty());
        let history = harness.db.get_inventory_history("p1", None).await.unwrap();
        assert_eq!(
            history.len(),
            1,
            "history is recorded even on a no-change cycle"
        );
        assert_eq!(history[0].quantity, 4);
    }

    #[tokio::test]
    async fn untracked_products_are_left_alone() {
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 2)])],
            0,
        )
        .await;
        harness
            .add_untracked_product("p1", 9, &[(Platform::Ebay, "E-1")])
            .await;

        let (synced, pushed) = harness.engine.run_cycle().await.unwrap();

        assert_eq!((synced, pushed), (0, 0));
        assert_eq!(harness.quantity("p1").await, 9);
        assert!(harness.adapter(Platform::Ebay).pushes().is_empty());
    }

    // ── Partial failure ───────────────────────────────────────

    #[tokio::test]
    async fn one_platform_down_does_not_abort_the_cycle() {
        // Squarespace is unreachable. The cycle must still resolve from eBay's
        // reading and report the failure as an event rather than returning Err.
        let mut harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 2)]),
                FakeAdapter::new(Platform::Squarespace)
                    .with_poll_script(vec![Poll::Network("connection refused")]),
            ],
            0,
        )
        .await;
        harness
            .add_product(
                "p1",
                8,
                &[(Platform::Ebay, "E-1"), (Platform::Squarespace, "S-1")],
            )
            .await;

        let (synced, _) = harness.engine.run_cycle().await.unwrap();

        assert_eq!(synced, 1);
        assert_eq!(harness.quantity("p1").await, 2);

        let events = harness.drain_events();
        assert!(
            events.iter().any(|e| matches!(
                e,
                SyncEngineEvent::PlatformError {
                    platform: Platform::Squarespace,
                    ..
                }
            )),
            "the unreachable platform must surface a PlatformError"
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, SyncEngineEvent::CycleComplete { .. })),
            "the cycle must still complete"
        );

        // Documenting current behaviour, not endorsing it: the platform whose
        // poll failed is still written to in the same cycle, with a canonical
        // quantity resolved without any reading from it. See the "partial
        // failure semantics" item in ROADMAP.md Phase 4.
        assert_eq!(
            harness.adapter(Platform::Squarespace).pushes(),
            vec![("S-1".to_string(), 2)]
        );
    }

    #[tokio::test]
    async fn a_failed_push_is_recorded_and_does_not_abort_the_cycle() {
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 1)]),
                FakeAdapter::new(Platform::Squarespace)
                    .with_quantities(&[("S-1", 6)])
                    .failing_pushes("listing is locked"),
            ],
            0,
        )
        .await;
        harness
            .add_product(
                "p1",
                6,
                &[(Platform::Ebay, "E-1"), (Platform::Squarespace, "S-1")],
            )
            .await;

        harness.engine.run_cycle().await.unwrap();

        // Canonical still moves to the conservative minimum — a failed push
        // does not roll back the local truth.
        assert_eq!(harness.quantity("p1").await, 1);

        let errors = harness.events_of_type("p1", "push_error").await;
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].target_platform, "squarespace");
        assert!(errors[0]
            .error_message
            .as_deref()
            .unwrap_or_default()
            .contains("listing is locked"));
    }

    // ── Retry and auth ────────────────────────────────────────

    #[tokio::test]
    async fn poll_retries_then_succeeds() {
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::Ebay).with_poll_script(vec![
                Poll::Network("timeout"),
                Poll::Network("timeout"),
                Poll::items(&[("E-1", 5)]),
            ])],
            2,
        )
        .await;

        let adapter = harness.adapter(Platform::Ebay);
        let items = harness
            .engine
            .poll_with_retry(adapter.as_ref())
            .await
            .unwrap();

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].quantity, 5);
        assert_eq!(adapter.poll_calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn poll_gives_up_after_max_retries_and_returns_the_last_error() {
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::Ebay)
                .with_poll_script(vec![Poll::Network("still down")])],
            2,
        )
        .await;

        let adapter = harness.adapter(Platform::Ebay);
        let err = harness
            .engine
            .poll_with_retry(adapter.as_ref())
            .await
            .unwrap_err();

        assert!(err.to_string().contains("still down"));
        assert_eq!(
            adapter.poll_calls.load(Ordering::SeqCst),
            3,
            "max_retries = 2 means one attempt plus two retries"
        );
    }

    #[tokio::test]
    async fn an_auth_error_triggers_a_refresh_and_an_auth_expired_event() {
        let mut harness = Harness::new(
            vec![FakeAdapter::new(Platform::Ebay)
                .with_poll_script(vec![Poll::Auth("token expired")])],
            0,
        )
        .await;

        let adapter = harness.adapter(Platform::Ebay);
        let err = harness
            .engine
            .poll_with_retry(adapter.as_ref())
            .await
            .unwrap_err();

        assert!(matches!(err, SyncError::AuthError { .. }));
        assert_eq!(adapter.refresh_calls.load(Ordering::SeqCst), 1);
        assert!(
            harness.drain_events().iter().any(|e| matches!(
                e,
                SyncEngineEvent::AuthExpired {
                    platform: Platform::Ebay
                }
            )),
            "a failed refresh must tell the UI the platform needs re-authentication"
        );
    }

    // ── Stock-mode platforms ──────────────────────────────────

    #[tokio::test]
    async fn a_stock_mode_sale_decrements_canonical_and_is_deduplicated() {
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::XmrBazaar)
                .with_stock_mode()
                .with_sales(vec![sale("X-1", 2, "order-42")])],
            0,
        )
        .await;
        harness
            .add_product("p1", 5, &[(Platform::XmrBazaar, "X-1")])
            .await;

        harness.engine.run_cycle().await.unwrap();

        assert_eq!(
            harness.quantity("p1").await,
            3,
            "two units sold on a stock-mode platform must come off canonical"
        );
        assert!(
            harness.db.is_xmr_order_processed("order-42").await.unwrap(),
            "the order must be marked processed so a later cycle cannot double-count it"
        );

        // Replay the same order. The DB-level dedup must swallow it.
        harness
            .adapter(Platform::XmrBazaar)
            .arm_sales(vec![sale("X-1", 2, "order-42")]);

        harness.engine.run_cycle().await.unwrap();

        assert_eq!(
            harness.quantity("p1").await,
            3,
            "replaying a processed order must not decrement again"
        );
    }

    #[tokio::test]
    async fn a_sale_on_an_unmapped_listing_is_skipped() {
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::XmrBazaar)
                .with_stock_mode()
                .with_sales(vec![sale("X-UNKNOWN", 3, "order-99")])],
            0,
        )
        .await;
        harness
            .add_product("p1", 5, &[(Platform::XmrBazaar, "X-1")])
            .await;

        harness.engine.run_cycle().await.unwrap();

        assert_eq!(harness.quantity("p1").await, 5);
        assert!(
            !harness.db.is_xmr_order_processed("order-99").await.unwrap(),
            "an unmapped sale is not consumed, so it can be picked up once the listing is mapped"
        );
    }

    #[tokio::test]
    async fn selling_the_last_unit_flips_a_stock_mode_listing_out_of_stock() {
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::XmrBazaar)
                .with_stock_mode()
                .with_sales(vec![sale("X-1", 1, "order-1")])],
            0,
        )
        .await;
        harness
            .add_product("p1", 1, &[(Platform::XmrBazaar, "X-1")])
            .await;

        harness.engine.run_cycle().await.unwrap();

        assert_eq!(harness.quantity("p1").await, 0);
        assert_eq!(
            harness.adapter(Platform::XmrBazaar).pushes(),
            vec![("X-1".to_string(), 0)],
            "selling the last unit must flip the listing out of stock"
        );
        assert_eq!(
            harness
                .events_of_type("p1", "stock_mode_change")
                .await
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn a_stock_mode_push_is_not_repeated_once_the_state_matches() {
        // Regression guard: the engine skips a stock-mode push when the
        // snapshot's version_tag already equals the desired state. That tag
        // has to actually survive the first push, or every subsequent cycle
        // re-writes the listing on a live marketplace for no reason.
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::XmrBazaar)
                .with_stock_mode()
                .with_sales(vec![sale("X-1", 1, "order-1")])],
            0,
        )
        .await;
        harness
            .add_product("p1", 1, &[(Platform::XmrBazaar, "X-1")])
            .await;

        harness.engine.run_cycle().await.unwrap();

        let snapshot = harness
            .db
            .get_snapshot("p1", "xmrbazaar")
            .await
            .unwrap()
            .expect("the first push must leave a snapshot behind");
        assert_eq!(
            snapshot.version_tag.as_deref(),
            Some("out_of_stock"),
            "the pushed stock mode must be persisted on the snapshot"
        );

        // A second cycle with nothing new to report must be a complete no-op
        // against the platform.
        harness.engine.run_cycle().await.unwrap();

        assert_eq!(
            harness.adapter(Platform::XmrBazaar).pushes().len(),
            1,
            "the stock mode was already correct, so the second cycle must not push again"
        );
    }

    // ── Dry run ───────────────────────────────────────────────

    #[tokio::test]
    async fn a_dry_run_writes_to_no_platform_and_reports_what_it_would_do() {
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 3)]),
                FakeAdapter::new(Platform::Squarespace).with_quantities(&[("S-1", 7)]),
            ],
            0,
        )
        .await;
        harness
            .add_product(
                "p1",
                10,
                &[(Platform::Ebay, "E-1"), (Platform::Squarespace, "S-1")],
            )
            .await;

        let mut harness = harness.into_dry_run();
        assert!(harness.engine.is_dry_run());

        let (synced, would_push) = harness.engine.run_cycle().await.unwrap();

        assert_eq!(synced, 1);
        assert_eq!(would_push, 1, "Squarespace is the one platform out of step");

        assert!(
            harness.adapter(Platform::Ebay).pushes().is_empty()
                && harness.adapter(Platform::Squarespace).pushes().is_empty(),
            "a dry run must never call set_quantity"
        );

        let planned: Vec<_> = harness
            .drain_events()
            .into_iter()
            .filter_map(|e| match e {
                SyncEngineEvent::DryRunChange {
                    platform,
                    current_quantity,
                    planned_quantity,
                    ..
                } => Some((platform, current_quantity, planned_quantity)),
                _ => None,
            })
            .collect();
        assert_eq!(planned, vec![(Platform::Squarespace, 7, 3)]);
    }

    #[tokio::test]
    async fn a_dry_run_leaves_the_local_database_untouched() {
        // Just as important as not writing to a platform: if a dry run moved
        // the canonical quantity or the snapshots, the real cycle that follows
        // would see the work as already done and skip it.
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 2)])],
            0,
        )
        .await;
        harness
            .add_product("p1", 9, &[(Platform::Ebay, "E-1")])
            .await;

        let harness = harness.into_dry_run();
        harness.engine.run_cycle().await.unwrap();

        assert_eq!(
            harness.quantity("p1").await,
            9,
            "canonical quantity unchanged"
        );
        assert!(
            harness
                .db
                .get_snapshot("p1", "ebay")
                .await
                .unwrap()
                .is_none(),
            "no snapshot was written"
        );
        assert!(
            harness
                .db
                .get_inventory_history("p1", None)
                .await
                .unwrap()
                .is_empty(),
            "no history row was written"
        );
        assert!(
            harness
                .db
                .list_recent_sync_events(10)
                .await
                .unwrap()
                .is_empty(),
            "no sync event was recorded"
        );
    }

    #[tokio::test]
    async fn a_dry_run_does_not_consume_a_stock_mode_order() {
        // The XMR Bazaar dedup table is the subtle one: marking the order
        // processed during a preview would make the next real cycle skip the
        // sale entirely, so the unit would never come off the shelf.
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::XmrBazaar)
                .with_stock_mode()
                .with_sales(vec![sale("X-1", 2, "order-42")])],
            0,
        )
        .await;
        harness
            .add_product("p1", 5, &[(Platform::XmrBazaar, "X-1")])
            .await;

        let harness = harness.into_dry_run();
        harness.engine.run_cycle().await.unwrap();

        assert_eq!(harness.quantity("p1").await, 5);
        assert!(harness.adapter(Platform::XmrBazaar).pushes().is_empty());
        assert!(
            !harness.db.is_xmr_order_processed("order-42").await.unwrap(),
            "a previewed order must still be available to the next real cycle"
        );
    }

    #[tokio::test]
    async fn a_dry_run_still_polls_every_platform() {
        // Reading is the whole point — the preview is worthless if it does not
        // ask the platforms what they currently hold.
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 3)])],
            0,
        )
        .await;
        harness
            .add_product("p1", 3, &[(Platform::Ebay, "E-1")])
            .await;

        let adapter = harness.adapter(Platform::Ebay);
        let harness = harness.into_dry_run();
        harness.engine.run_cycle().await.unwrap();

        assert_eq!(adapter.poll_calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_real_cycle_after_a_dry_run_makes_the_change_the_dry_run_predicted() {
        // The contract that makes a preview worth trusting.
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 3)]),
                FakeAdapter::new(Platform::Squarespace).with_quantities(&[("S-1", 7)]),
            ],
            0,
        )
        .await;
        harness
            .add_product(
                "p1",
                10,
                &[(Platform::Ebay, "E-1"), (Platform::Squarespace, "S-1")],
            )
            .await;

        let squarespace = harness.adapter(Platform::Squarespace);
        let db = harness.db.clone();
        let adapters = harness.engine.adapters.clone();
        let (tx, _rx) = mpsc::channel(256);

        let dry = harness.into_dry_run();
        let (_, would_push) = dry.engine.run_cycle().await.unwrap();
        assert_eq!(would_push, 1);
        assert!(squarespace.pushes().is_empty());

        // Same DB, same adapters, same canned readings — but for real.
        let real = SyncEngine::new(db.clone(), adapters, tx, 0);
        let (_, pushed) = real.run_cycle().await.unwrap();

        assert_eq!(pushed, would_push, "the preview matched the real cycle");
        assert_eq!(squarespace.pushes(), vec![("S-1".to_string(), 3)]);
        assert_eq!(db.get_product("p1").await.unwrap().unwrap().quantity, 3);
    }

    #[tokio::test]
    async fn detect_sales_is_only_called_on_stock_mode_platforms() {
        // `PlatformAdapter::detect_sales` has a default impl returning an empty
        // list, and eBay, Squarespace and Amazon all inherit it. That default is
        // correct precisely because the engine gates the call on
        // `has_stock_mode_inventory` — it is never reached on a numeric platform,
        // so those three are not silently reporting "no sales" every cycle.
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 3)]),
                FakeAdapter::new(Platform::XmrBazaar).with_stock_mode(),
            ],
            0,
        )
        .await;
        harness
            .add_product(
                "p1",
                3,
                &[(Platform::Ebay, "E-1"), (Platform::XmrBazaar, "X-1")],
            )
            .await;

        harness.engine.run_cycle().await.unwrap();

        assert_eq!(
            harness
                .adapter(Platform::Ebay)
                .detect_sales_calls
                .load(Ordering::SeqCst),
            0,
            "a numeric platform must never be asked to detect sales"
        );
        assert_eq!(
            harness
                .adapter(Platform::XmrBazaar)
                .detect_sales_calls
                .load(Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn stock_mode_platforms_do_not_participate_in_quantity_resolution() {
        // XMR Bazaar has no meaningful numeric quantity. Even when it reports
        // one, canonical must come from the numeric platforms alone — here
        // eBay's 4, not XMR's 99.
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 4)]),
                FakeAdapter::new(Platform::XmrBazaar)
                    .with_stock_mode()
                    .with_quantities(&[("X-1", 99)]),
            ],
            0,
        )
        .await;
        harness
            .add_product(
                "p1",
                20,
                &[(Platform::Ebay, "E-1"), (Platform::XmrBazaar, "X-1")],
            )
            .await;

        harness.engine.run_cycle().await.unwrap();

        assert_eq!(harness.quantity("p1").await, 4);
    }
}
