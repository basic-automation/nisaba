use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::{mpsc, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::conflict::resolve_deltas;
use crate::db::{ConsumedXmrOrder, Db};
use crate::error::SyncError;
use crate::traits::PlatformAdapter;
use crate::types::{NewSyncEvent, Platform, PlatformMapping, ProductVariant, QuantityDelta};

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
    /// Reload config and adapters, applying `interval_secs` as the new
    /// period between automatic sync cycles. A zero interval is ignored:
    /// it would panic the timer and kill the engine task.
    Reload { interval_secs: u64 },
}

// ── Sync Engine ───────────────────────────────────────────────

/// Callback invoked after each sync cycle to refresh vendor plugin data.
/// The closure returns a future that runs all approved vendor plugins,
/// updates product_vendor_data, and recalculates quantities.
///
/// The engine never awaits this future inline: [`SyncEngine::run`] spawns
/// it as its own task (see `spawn_vendor_sync`), because a plugin run may
/// last hours and must not stall the platform sync loop — Pause and Sync
/// Now stay responsive while it runs.
pub type VendorSyncFn = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// How long sync events are kept before the engine's housekeeping prunes
/// them. Events are an audit/debug trail for recent cycles; older ones only
/// grow the database.
pub const SYNC_EVENT_RETENTION_DAYS: i64 = 90;

/// Inventory history older than this is downsampled by housekeeping to one
/// row per product per day — enough for long-range trend charts without
/// keeping every intra-day change forever.
pub const HISTORY_DOWNSAMPLE_AFTER_DAYS: i64 = 30;

/// How often the running engine repeats housekeeping after the startup run.
const HOUSEKEEPING_INTERVAL: std::time::Duration = std::time::Duration::from_secs(24 * 60 * 60);

/// One numeric platform reading captured for ledger resolution: the listing's
/// current polled quantity plus its last-known snapshot quantity, if the
/// listing has ever been observed before.
struct ObservedReading {
    mapping: PlatformMapping,
    current_qty: i64,
    snapshot_qty: Option<i64>,
}

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
    /// The callback is spawned as an independent task, never awaited inline.
    pub fn with_vendor_sync(mut self, f: VendorSyncFn) -> Self {
        self.vendor_sync_fn = Some(f);
        self
    }

    /// Trigger a vendor refresh as its own task, without awaiting it.
    ///
    /// At most one engine-triggered vendor run is in flight at a time: if
    /// the previous spawned run has not finished, this trigger is skipped
    /// (the vendor callback itself — `run_vendor_sync_all` in the app —
    /// also refuses overlapping runs). A dry run never triggers one.
    fn spawn_vendor_sync(&self, slot: &mut Option<tokio::task::JoinHandle<()>>) {
        if self.dry_run {
            return;
        }
        let Some(sync_fn) = &self.vendor_sync_fn else {
            return;
        };
        if let Some(handle) = slot.as_ref() {
            if !handle.is_finished() {
                debug!("Vendor sync already running as its own task, skipping trigger");
                return;
            }
        }
        *slot = Some(tokio::spawn((sync_fn)()));
    }

    /// Run a single sync cycle: poll → detect → resolve → push → record.
    ///
    /// # Partial-failure policy: never push to an unpolled platform
    ///
    /// A platform that could not be read this cycle receives **no pushes**
    /// this cycle. For a numeric platform a successful reading is a
    /// `fetch_inventory` poll that returned this listing's quantity; for a
    /// stock-mode platform it is a successful poll *and* a successful
    /// `detect_sales` call (stock-mode platforms have no numeric reading —
    /// sale detection is their read). One platform being down never aborts
    /// the cycle: the remaining platforms still resolve and are pushed to,
    /// the failure surfaces as a [`SyncEngineEvent::PlatformError`], and the
    /// unread platform is simply skipped in the push step. Pushing to it
    /// anyway would overwrite live stock with a canonical value resolved
    /// without any reading from it — a platform that was merely slow to
    /// answer could lose real sales or be restocked on paper. The platform
    /// is reconciled on the next cycle in which it can be read again.
    pub async fn run_cycle(&self) -> Result<(usize, usize), SyncError> {
        let cycle_id = Uuid::new_v4().to_string();
        let now = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S").to_string();
        info!(cycle_id = %cycle_id, "Starting sync cycle");

        // Clone the adapter handles out of the shared map, then release
        // the lock before any network work begins. Holding the read lock
        // for the whole cycle (polls, retries and pushes included) made
        // every writer — saving config rebuilds the map under the write
        // lock — wait for the cycle to finish. The handles are `Arc`s, so
        // the clone is cheap, and a config save that swaps the map mid-cycle
        // does not disturb this cycle: it finishes on the handles it
        // started with, and the next cycle picks up the new map.
        let adapters: HashMap<Platform, Arc<dyn PlatformAdapter>> =
            self.adapters.read().await.clone();

        if adapters.is_empty() {
            info!("No platforms enabled, skipping sync cycle");
            return Ok((0, 0));
        }

        // Get all tracked products with their active mappings
        let products = self.db.list_products().await?;
        let all_mappings = self.db.list_all_active_mappings().await?;

        // Group mappings by (product, variant). Canonical stock, platform
        // readings and pushes are all keyed per variant: each variant's
        // listings resolve and push that variant's quantity, never the
        // product's total. Mappings without a variant (or whose variant row
        // no longer exists) fall into the product-level group (None), which
        // keeps the historical product-granularity behaviour.
        let mut groups: HashMap<(String, Option<String>), Vec<PlatformMapping>> = HashMap::new();
        for mapping in &all_mappings {
            groups
                .entry((mapping.product_id.clone(), mapping.variant_id.clone()))
                .or_default()
                .push(mapping.clone());
        }

        // ── Step 1: Poll all enabled platforms ────────────────
        let mut platform_quantities: HashMap<(Platform, String), i64> = HashMap::new();
        // Platforms whose poll succeeded this cycle. Together with
        // `platform_quantities` (numeric platforms) and
        // `sales_read_platforms` (stock-mode platforms) this is the record
        // the push step consults for the partial-failure policy documented
        // on `run_cycle`: no successful reading, no push.
        let mut polled_platforms: HashSet<Platform> = HashSet::new();

        for (platform, adapter) in adapters.iter() {
            match self.poll_with_retry(adapter.as_ref()).await {
                Ok(items) => {
                    polled_platforms.insert(*platform);
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
        // via scraping instead of quantity polling. Keyed per (product,
        // variant) so a sale on one variant's listing decrements that
        // variant's canonical stock, not the product's.
        let mut injected_deltas: HashMap<(String, Option<String>), Vec<QuantityDelta>> =
            HashMap::new();

        // XMR orders whose sales were injected above, keyed like the deltas.
        // An order is marked processed only in the same transaction as the
        // stock write that applies its sale (the group resolution below) —
        // never here. Marking here, ahead of the write, is what used to lose
        // sales when a cycle failed between the two, and it is what let a
        // dry run consume a sale the next real cycle then skipped.
        let mut pending_xmr_sales: HashMap<(String, Option<String>), Vec<ConsumedXmrOrder>> =
            HashMap::new();

        let tracked_product_ids: HashSet<String> = products
            .iter()
            .filter(|p| p.is_tracked)
            .map(|p| p.id.clone())
            .collect();

        // Stock-mode platforms whose sale detection succeeded this cycle.
        // Sale detection is a stock-mode platform's reading, so the push
        // step requires membership here (in addition to `polled_platforms`)
        // before it will write to such a platform.
        let mut sales_read_platforms: HashSet<Platform> = HashSet::new();

        for (platform, adapter) in adapters.iter() {
            if !adapter.capabilities().has_stock_mode_inventory {
                continue;
            }

            match adapter.detect_sales().await {
                Ok(sales) => {
                    sales_read_platforms.insert(*platform);
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

                        // A sale on an untracked product is neither applied
                        // nor consumed: the quantity resolution below skips
                        // untracked products, so marking the order processed
                        // would lose the sale. It stays pending and is picked
                        // up once the product is tracked.
                        if !tracked_product_ids.contains(&mapping.product_id) {
                            debug!(
                                platform = %platform,
                                product_id = %mapping.product_id,
                                order_id,
                                "Sale detected for untracked product, leaving it unconsumed"
                            );
                            continue;
                        }

                        info!(
                            platform = %platform,
                            product_id = %mapping.product_id,
                            item_id = %sale.platform_item_id,
                            units = sale.units_sold,
                            order_id,
                            "Injecting sale delta from stock-mode platform"
                        );

                        // Inject a negative delta against this listing's variant.
                        // The key uses the same normalization as the resolve
                        // loop below: a mapping that names a missing variant
                        // row lands in the product-level (None) group.
                        let delta_variant_id: Option<String> = match &mapping.variant_id {
                            Some(vid) => {
                                if self.db.get_variant(vid).await?.is_some() {
                                    Some(vid.clone())
                                } else {
                                    None
                                }
                            }
                            None => None,
                        };
                        injected_deltas
                            .entry((mapping.product_id.clone(), delta_variant_id.clone()))
                            .or_default()
                            .push(QuantityDelta {
                                product_id: mapping.product_id.clone(),
                                platform: *platform,
                                previous: 0,
                                current: 0,
                                delta: -sale.units_sold,
                            });

                        // Defer the order mark: it lands in the same
                        // transaction as the stock write that applies this
                        // sale (the group resolution below). A dry run never
                        // reaches that write, so a previewed order stays
                        // available to the next real cycle.
                        if !order_id.is_empty() {
                            pending_xmr_sales
                                .entry((mapping.product_id.clone(), delta_variant_id.clone()))
                                .or_default()
                                .push(ConsumedXmrOrder {
                                    order_id: order_id.to_string(),
                                    product_id: Some(mapping.product_id.clone()),
                                    listing_id: sale.platform_item_id.clone(),
                                    quantity: sale.units_sold,
                                });
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

            // This product's variant rows, for resolving per-variant canonical
            // stock. A mapping whose variant id names no existing row falls
            // back to the product-level group (`variant_id = None`), which
            // keeps the historical product-granularity behaviour.
            let variants = self.db.list_variants(&product.id).await?;
            let variant_by_id: HashMap<&str, &ProductVariant> =
                variants.iter().map(|v| (v.id.as_str(), v)).collect();

            // This product's (variant, mappings) groups.
            let product_groups: Vec<(&Option<String>, &Vec<PlatformMapping>)> = groups
                .iter()
                .filter(|((product_id, _), _)| product_id == &product.id)
                .map(|((_, variant_id), mappings)| (variant_id, mappings))
                .collect();

            if product_groups.is_empty() {
                continue;
            }

            // ── Step 2: Resolve quantity from the summed deltas ──
            // Every numeric platform reading becomes a delta against its
            // last-known snapshot, and the deltas are summed onto the
            // group's canonical quantity (`resolve_deltas`). Two sales on two
            // platforms in one cycle both decrement; a platform-side restock
            // nets against sales elsewhere. This replaces the old
            // minimum / max-minus-decreases derivation, which undercounted
            // multi-platform sales and let platform readings overwrite
            // stock changed in Nisaba.
            //
            // A listing with no snapshot is baselined by group state:
            // - no snapshot for ANY mapping in the group: this is the first
            //   observation of the group's inventory, so the group's
            //   canonical quantity is the baseline and pre-mapping platform
            //   activity still lands as a delta;
            // - otherwise the listing was newly mapped onto an already-synced
            //   group: it contributes no delta. Absorbing its reading would
            //   corrupt canonical with stock Nisaba never tracked; the push
            //   step below corrects the listing to canonical instead.
            let mut product_changed = false;
            // Pushes matter for the synced count even when canonical did not
            // move (failed-push retry, newly mapped listing fix).
            let pushed_before = changes_pushed;

            for (variant_id, mappings) in product_groups {
                let variant = variant_id
                    .as_deref()
                    .and_then(|id| variant_by_id.get(id).copied());
                // A mapping that names a missing variant row is treated as a
                // product-level mapping in every respect — canonical stock,
                // snapshots, pushes — not just for the quantity lookup.
                let group_variant_id: Option<&str> = variant.and(variant_id.as_deref());
                let canonical_old = variant.map(|v| v.quantity).unwrap_or(product.quantity);
                let is_variant_group = variant.is_some();
                let mut group_changed = false;
                // ── Step 2: Resolve quantity from the summed deltas ──
                // Every numeric platform reading becomes a delta against its
                // last-known snapshot; the deltas are summed onto the group's
                // canonical quantity below (replaces the old min /
                // max-minus-decreases derivation — see the comment above the
                // group loop).
                let mut readings: Vec<ObservedReading> = Vec::new();
                let mut any_snapshot = false;

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

                    // Compare against last snapshot to detect the platform's delta
                    let snapshot = self
                        .db
                        .get_snapshot(&product.id, mapping.platform.as_str(), group_variant_id)
                        .await?;
                    if snapshot.is_some() {
                        any_snapshot = true;
                    }

                    // Update the polled snapshot
                    if !self.dry_run {
                        self.db
                            .upsert_snapshot(
                                &product.id,
                                mapping.platform.as_str(),
                                group_variant_id,
                                current_qty,
                                &now,
                            )
                            .await?;
                    }

                    readings.push(ObservedReading {
                        mapping: mapping.clone(),
                        current_qty,
                        snapshot_qty: snapshot.map(|s| s.last_known_quantity),
                    });
                }

                let mut deltas: Vec<QuantityDelta> = Vec::new();
                for reading in &readings {
                    let previous = match reading.snapshot_qty {
                        Some(qty) => qty,
                        None if !any_snapshot => canonical_old,
                        // Newly mapped listing on an already-synced group:
                        // no delta; the push step corrects it to canonical.
                        None => continue,
                    };
                    let delta = reading.current_qty - previous;
                    if delta != 0 {
                        deltas.push(QuantityDelta {
                            product_id: product.id.clone(),
                            platform: reading.mapping.platform,
                            previous,
                            current: reading.current_qty,
                            delta,
                        });
                    }
                }

                // Fold in the stock-mode sale decrements (from Step 1b); they
                // are already deltas, so they sum onto the ledger like the
                // rest. Keyed per (product, variant) group.
                if let Some(xmr_deltas) = injected_deltas
                    .remove(&(product.id.clone(), group_variant_id.map(str::to_string)))
                {
                    deltas.extend(xmr_deltas);
                }

                // The XMR orders behind those deltas. They are consumed at
                // this group's write points below — in the same transaction
                // as the stock write that applies their sales.
                let group_xmr_sales = pending_xmr_sales
                    .remove(&(product.id.clone(), group_variant_id.map(str::to_string)))
                    .unwrap_or_default();

                let resolved = resolve_deltas(&product.id, canonical_old, deltas);
                // `new_canonical` is the fully resolved canonical for the
                // shared tail below (XMR deltas already folded in).
                let new_canonical = resolved.new_canonical;

                if new_canonical == canonical_old {
                    // No history row here: history records quantity *changes*
                    // only (see `Db::record_inventory_history`). Writing a row
                    // for every unchanged cycle would spam the table with
                    // ~288 identical rows per product per day.
                    // Falls through to the push step below: a platform whose
                    // reading disagrees with canonical still needs a push
                    // (failed-push retry, newly mapped listing correction).

                    // The sales were folded into the resolution even though
                    // the quantity did not move (already at the floor), so
                    // consume their orders here — in their own transaction,
                    // since there is no stock write to share one with.
                    if !self.dry_run && !group_xmr_sales.is_empty() {
                        if let Some(v) = variant {
                            self.db
                                .apply_variant_stock_and_consume_xmr_sales(
                                    &v.id,
                                    None,
                                    &group_xmr_sales,
                                )
                                .await?;
                        } else {
                            self.db
                                .apply_stock_and_consume_xmr_sales(
                                    &product.id,
                                    None,
                                    &group_xmr_sales,
                                )
                                .await?;
                        }
                    }
                } else {
                    if resolved.total_delta != 0 {
                        info!(
                            product = %product.name,
                            variant = ?variant_id,
                            old = canonical_old,
                            new = new_canonical,
                            total_delta = resolved.total_delta,
                            deltas = ?resolved.deltas,
                            "Resolved quantity change from summed deltas"
                        );
                    }

                    // Update canonical quantity in DB: the variant row for a
                    // variant group, the product row for the product-level
                    // group. The XMR orders whose sales this write applies
                    // are marked processed in the same transaction — either
                    // both commit or neither does, so a failed cycle can
                    // never consume an order whose sale it did not apply.
                    if !self.dry_run {
                        if let Some(v) = variant {
                            self.db
                                .apply_variant_stock_and_consume_xmr_sales(
                                    &v.id,
                                    Some(new_canonical),
                                    &group_xmr_sales,
                                )
                                .await?;
                        } else {
                            self.db
                                .apply_stock_and_consume_xmr_sales(
                                    &product.id,
                                    Some(new_canonical),
                                    &group_xmr_sales,
                                )
                                .await?;
                        }
                    }

                    product_changed = true;
                    group_changed = true;
                }

                // ── Step 4: Push to all mapped platforms ──────────
                for mapping in mappings {
                    // Skip disabled platforms
                    let Some(adapter) = adapters.get(&mapping.platform) else {
                        continue;
                    };

                    // Stock-mode-aware push logic
                    if adapter.capabilities().has_stock_mode_inventory {
                        // Partial-failure policy (see `run_cycle`): a
                        // stock-mode platform with no successful reading
                        // this cycle — its poll failed, or its sale
                        // detection (the stock-mode reading) failed — is
                        // never pushed to. Its live stock mode is unknown,
                        // and flipping it from a canonical value resolved
                        // without it could deactivate a live listing or
                        // reactivate one that sold out in the meantime.
                        if !polled_platforms.contains(&mapping.platform)
                            || !sales_read_platforms.contains(&mapping.platform)
                        {
                            debug!(
                                product = %product.name,
                                platform = %mapping.platform,
                                "No successful reading this cycle, skipping stock-mode push"
                            );
                            continue;
                        }

                        let snapshot = self
                            .db
                            .get_snapshot(&product.id, mapping.platform.as_str(), group_variant_id)
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
                                    current_quantity: canonical_old,
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
                                        group_variant_id,
                                        new_canonical,
                                        &now,
                                    )
                                    .await?;

                                // Record the stock mode we just pushed.
                                self.db
                                    .update_snapshot_version_tag(
                                        &product.id,
                                        mapping.platform.as_str(),
                                        group_variant_id,
                                        desired,
                                    )
                                    .await?;

                                self.db
                                    .insert_sync_event(&NewSyncEvent {
                                        product_id: &product.id,
                                        source_platform: "sync_engine",
                                        target_platform: mapping.platform.as_str(),
                                        old_quantity: canonical_old,
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
                                        old_quantity: canonical_old,
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
                                        old_quantity: canonical_old,
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

                    // Normal (numeric quantity) push logic.
                    //
                    // Partial-failure policy (see `run_cycle`): a platform
                    // with no successful reading this cycle is skipped — both
                    // when its whole poll failed (`polled_platforms`) and
                    // when the poll succeeded but returned no quantity for
                    // this listing (`platform_quantities`). Its state is
                    // unknown, and pushing blind would overwrite live stock
                    // with a number derived from its peers.
                    if !polled_platforms.contains(&mapping.platform) {
                        debug!(
                            product = %product.name,
                            platform = %mapping.platform,
                            "Platform was not polled this cycle, skipping push"
                        );
                        continue;
                    }
                    let platform_key = (mapping.platform, mapping.platform_item_id.clone());
                    let Some(current_on_platform) = platform_quantities.get(&platform_key).copied()
                    else {
                        debug!(
                            product = %product.name,
                            platform = %mapping.platform,
                            "No reading this cycle, skipping push"
                        );
                        continue;
                    };

                    // Only push if the platform doesn't already have the correct quantity
                    if current_on_platform == new_canonical {
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
                                current_quantity: current_on_platform,
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
                                    group_variant_id,
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
                                    old_quantity: canonical_old,
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
                                    old_quantity: canonical_old,
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
                                    old_quantity: canonical_old,
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
                // Product-level group only, and only when the canonical
                // quantity actually changed this cycle.
                if group_changed && !is_variant_group && !self.dry_run {
                    self.db
                        .record_inventory_history(&product.id, new_canonical, &cycle_id)
                        .await?;
                }
            }

            if product_changed || changes_pushed > pushed_before {
                products_synced += 1;
            }
        }

        // The cloned adapter snapshot is dropped here; the shared map's
        // lock was already released before the cycle's network work began.
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

    /// Prune old sync events and downsample old inventory history.
    ///
    /// This is the only caller of [`Db::prune_old_sync_events`] and
    /// [`Db::downsample_old_history`]: the engine runs it once at startup
    /// (from [`SyncEngine::run`]) and then once a day while it runs, so the
    /// `sync_events` and `inventory_history` tables stay bounded without any
    /// manual maintenance. A dry-run engine never calls this — pruning is a
    /// local write, and a dry run makes none.
    pub async fn run_housekeeping(&self) -> Result<(), SyncError> {
        let pruned = self
            .db
            .prune_old_sync_events(SYNC_EVENT_RETENTION_DAYS)
            .await?;
        let downsampled = self
            .db
            .downsample_old_history(HISTORY_DOWNSAMPLE_AFTER_DAYS)
            .await?;
        if pruned > 0 || downsampled > 0 {
            info!(
                pruned_sync_events = pruned,
                downsampled_history_rows = downsampled,
                "Housekeeping pruned old sync data"
            );
        }
        Ok(())
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

        // Startup housekeeping, repeated daily in the tick branch below.
        // Skipped for a dry-run engine: pruning is a local write.
        let mut last_housekeeping = tokio::time::Instant::now();
        if !self.dry_run {
            if let Err(e) = self.run_housekeeping().await {
                warn!(error = %e, "Startup housekeeping failed");
            }
        }

        let mut paused = false;
        // A zero period makes `tokio::time::interval` panic, which would
        // kill the engine task outright. Callers validate the configured
        // schedule before it gets here; this is a backstop so no caller
        // bug can take the engine down.
        let mut interval_secs = interval_secs;
        if interval_secs == 0 {
            warn!(
                fallback_secs = crate::config::DEFAULT_SYNC_INTERVAL_SECS,
                "Sync interval of 0s requested; falling back to the default"
            );
            interval_secs = crate::config::DEFAULT_SYNC_INTERVAL_SECS;
        }
        // The in-flight vendor refresh, spawned as its own task after each
        // cycle. The loop never awaits it, so commands (Pause, Sync Now)
        // are processed while a vendor run — which may last hours — is
        // still going.
        let mut vendor_task: Option<tokio::task::JoinHandle<()>> = None;
        let mut interval = tokio::time::interval(std::time::Duration::from_secs(interval_secs));
        // Don't try to catch up missed ticks
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if !self.dry_run && last_housekeeping.elapsed() >= HOUSEKEEPING_INTERVAL {
                        if let Err(e) = self.run_housekeeping().await {
                            warn!(error = %e, "Scheduled housekeeping failed");
                        }
                        last_housekeeping = tokio::time::Instant::now();
                    }
                    if !paused {
                        if let Err(e) = self.run_cycle().await {
                            error!(error = %e, "Sync cycle failed");
                        }
                        self.spawn_vendor_sync(&mut vendor_task);
                    }
                }
                cmd = cmd_rx.recv() => {
                    match cmd {
                        Some(SyncCommand::SyncNow) => {
                            info!("Manual sync triggered");
                            if let Err(e) = self.run_cycle().await {
                                error!(error = %e, "Manual sync cycle failed");
                            }
                            self.spawn_vendor_sync(&mut vendor_task);
                        }
                        Some(SyncCommand::Pause) => {
                            info!("Sync paused");
                            paused = true;
                        }
                        Some(SyncCommand::Resume) => {
                            info!("Sync resumed");
                            paused = false;
                        }
                        Some(SyncCommand::Reload { interval_secs: new_interval_secs }) => {
                            if new_interval_secs == 0 {
                                // Reject rather than panic: a zero period
                                // would kill this task (see the backstop in
                                // `run`'s setup). The current cadence stays.
                                warn!("Ignoring config reload with a zero sync interval");
                            } else if new_interval_secs != interval_secs {
                                info!(
                                    interval_secs = new_interval_secs,
                                    "Config reloaded — sync interval updated"
                                );
                                interval_secs = new_interval_secs;
                                interval = tokio::time::interval(
                                    std::time::Duration::from_secs(new_interval_secs),
                                );
                                // Don't try to catch up missed ticks
                                interval.set_missed_tick_behavior(
                                    tokio::time::MissedTickBehavior::Skip,
                                );
                                // A fresh interval's first tick completes
                                // immediately; reset it so the new period
                                // runs from now instead of firing a cycle
                                // the moment the config is saved.
                                interval.reset();
                            } else {
                                info!("Config reloaded — adapters will be used on next cycle");
                            }
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
        PlatformCapabilities, PlatformInventoryItem, PlatformListing, Product, ProductVariant,
        SaleDetection, SyncEvent,
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
        /// When set, `detect_sales` fails with this message.
        sales_error: Mutex<Option<String>>,
        /// When set, every `set_quantity` fails with this message.
        push_error: Mutex<Option<String>>,
        /// When set, every poll sleeps this long first, modelling a slow
        /// network (and, with retries, a long cycle).
        poll_delay: Mutex<Option<std::time::Duration>>,
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
                sales_error: Mutex::new(None),
                push_error: Mutex::new(None),
                poll_delay: Mutex::new(None),
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

        /// Make every `detect_sales` call fail, as if the platform were down.
        fn failing_sales_detection(self, message: &str) -> Self {
            *self.sales_error.lock().unwrap() = Some(message.to_string());
            self
        }

        fn failing_pushes(self, message: &str) -> Self {
            *self.push_error.lock().unwrap() = Some(message.to_string());
            self
        }

        /// Make every poll sleep before answering, as a slow network would.
        fn with_poll_delay(self, delay: std::time::Duration) -> Self {
            *self.poll_delay.lock().unwrap() = Some(delay);
            self
        }

        /// Flip push failures on or off mid-test, so one harness can run a
        /// fail-then-retry sequence across cycles.
        fn set_push_error(&self, message: Option<&str>) {
            *self.push_error.lock().unwrap() = message.map(str::to_string);
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
            let poll_delay = *self.poll_delay.lock().unwrap();
            if let Some(delay) = poll_delay {
                tokio::time::sleep(delay).await;
            }
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
            if let Some(message) = self.push_error.lock().unwrap().clone() {
                return Err(SyncError::ApiError {
                    platform: self.platform,
                    message,
                });
            }
            self.pushes
                .lock()
                .unwrap()
                .push((platform_item_id.to_string(), qty));
            // Model the platform applying our write: later polls report the
            // pushed quantity for that listing, the way a real marketplace
            // would after accepting a set_quantity call.
            let mut polls = self.polls.lock().unwrap();
            for poll in polls.iter_mut() {
                if let Poll::Items(items) = poll {
                    for item in items.iter_mut() {
                        if item.platform_item_id == platform_item_id {
                            item.quantity = qty;
                        }
                    }
                }
            }
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
            if let Some(message) = self.sales_error.lock().unwrap().clone() {
                return Err(SyncError::NetworkError {
                    platform: self.platform,
                    message,
                });
            }
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

        /// Insert a product variant row that the engine will treat as the
        /// canonical stock holder for mappings carrying its id.
        async fn add_variant(&self, variant_id: &str, product_id: &str, quantity: i64) {
            self.db
                .insert_variant(&ProductVariant {
                    id: variant_id.to_string(),
                    product_id: product_id.to_string(),
                    sku: format!("SKU-{variant_id}"),
                    name: format!("Variant {variant_id}"),
                    attributes: HashMap::new(),
                    quantity,
                    on_hand_quantity: quantity,
                    image_url: None,
                    sort_order: 0,
                    source_plugin_id: None,
                    source_vendor_item_id: None,
                    created_at: String::new(),
                    updated_at: String::new(),
                })
                .await
                .unwrap();
        }

        /// Map a platform listing to a specific variant of a product.
        async fn map_variant(
            &self,
            product_id: &str,
            platform: Platform,
            item_id: &str,
            variant_id: &str,
        ) {
            self.db
                .insert_mapping(product_id, platform.as_str(), item_id, None, variant_id)
                .await
                .unwrap();
        }

        async fn variant_quantity(&self, variant_id: &str) -> i64 {
            self.db
                .get_variant(variant_id)
                .await
                .unwrap()
                .expect("variant missing")
                .quantity
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
    async fn two_sales_on_two_platforms_in_one_cycle_both_decrement_canonical() {
        // The ledger acceptance case: both platforms were at 10, eBay sold 2
        // (10 -> 8) and Squarespace sold 1 (10 -> 9) in the same cycle. The
        // summed deltas are -2 + -1 = -3, so canonical goes 10 -> 7 and both
        // platforms are pushed down to 7. The old minimum-based resolution
        // answered 8 here, silently dropping the Squarespace sale.
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 8)]),
                FakeAdapter::new(Platform::Squarespace).with_quantities(&[("S-1", 9)]),
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
        harness
            .db
            .upsert_snapshot("p1", "ebay", None, 10, "2026-01-01 00:00:00")
            .await
            .unwrap();
        harness
            .db
            .upsert_snapshot("p1", "squarespace", None, 10, "2026-01-01 00:00:00")
            .await
            .unwrap();

        let (synced, pushed) = harness.engine.run_cycle().await.unwrap();

        assert_eq!(synced, 1);
        assert_eq!(pushed, 2, "both platforms disagree with the resolved 7");
        assert_eq!(harness.quantity("p1").await, 7);
        assert_eq!(
            harness.adapter(Platform::Ebay).pushes(),
            vec![("E-1".to_string(), 7)]
        );
        assert_eq!(
            harness.adapter(Platform::Squarespace).pushes(),
            vec![("S-1".to_string(), 7)]
        );

        let recorded = harness.events_of_type("p1", "quantity_change").await;
        assert_eq!(recorded.len(), 2);
    }

    #[tokio::test]
    async fn first_observation_uses_canonical_as_the_baseline() {
        // No snapshots exist yet: the canonical quantity is the baseline, so
        // pre-mapping platform activity still lands as a delta. Same numbers
        // as the seeded test above, without seeded snapshots.
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 8)]),
                FakeAdapter::new(Platform::Squarespace).with_quantities(&[("S-1", 9)]),
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

        assert_eq!((synced, pushed), (1, 2));
        assert_eq!(harness.quantity("p1").await, 7);
        assert_eq!(
            harness.adapter(Platform::Ebay).pushes(),
            vec![("E-1".to_string(), 7)]
        );
        assert_eq!(
            harness.adapter(Platform::Squarespace).pushes(),
            vec![("S-1".to_string(), 7)]
        );
    }

    #[tokio::test]
    async fn pushes_and_minimums_are_per_variant_not_per_product() {
        // Two variants of one product with different stock. Each platform
        // listing must converge to its own variant's quantity: variant A's
        // listings resolve across A's readings only (min 6 here), and
        // variant B's listing is never rewritten with A's (or the product's)
        // stock. Under the old product-granular behaviour the minimum across
        // all three listings (3) would have been pushed everywhere.
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-A", 10), ("E-B", 3)]),
                FakeAdapter::new(Platform::Squarespace).with_quantities(&[("S-A", 6)]),
            ],
            0,
        )
        .await;
        harness.add_product("p1", 13, &[]).await;
        harness.add_variant("va", "p1", 10).await;
        harness.add_variant("vb", "p1", 3).await;
        harness.map_variant("p1", Platform::Ebay, "E-A", "va").await;
        harness
            .map_variant("p1", Platform::Squarespace, "S-A", "va")
            .await;
        harness.map_variant("p1", Platform::Ebay, "E-B", "vb").await;

        let (synced, pushed) = harness.engine.run_cycle().await.unwrap();

        assert_eq!(synced, 1);
        assert_eq!(pushed, 1, "only E-A is out of step with variant A's 6");
        assert_eq!(harness.variant_quantity("va").await, 6);
        assert_eq!(
            harness.variant_quantity("vb").await,
            3,
            "variant B resolved from its own reading only"
        );
        assert_eq!(
            harness.quantity("p1").await,
            13,
            "the product total is not the sync target for variant-mapped listings"
        );
        assert_eq!(
            harness.adapter(Platform::Ebay).pushes(),
            vec![("E-A".to_string(), 6)],
            "ebay's variant-A listing gets 6; the variant-B listing is untouched"
        );
        assert!(
            harness.adapter(Platform::Squarespace).pushes().is_empty(),
            "S-A already reported 6, so it must not be written to"
        );

        let recorded = harness.events_of_type("p1", "quantity_change").await;
        assert_eq!(recorded.len(), 1);
        assert_eq!(recorded[0].target_platform, "ebay");
        assert_eq!(
            (recorded[0].old_quantity, recorded[0].new_quantity),
            (10, 6),
            "the event carries variant A's canonical, not the product's"
        );

        // A second cycle with the pushed quantities visible on the platforms
        // is stable: per-variant resolution converges, it does not oscillate.
        let (synced2, pushed2) = harness.engine.run_cycle().await.unwrap();
        assert_eq!((synced2, pushed2), (0, 0));
        assert_eq!(harness.adapter(Platform::Ebay).pushes().len(), 1);
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
            .upsert_snapshot("p1", "ebay", None, 5, "2026-01-01 00:00:00")
            .await
            .unwrap();
        harness
            .db
            .upsert_snapshot("p1", "squarespace", None, 5, "2026-01-01 00:00:00")
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
        // Deltas that would drive canonical below zero must floor at 0, and
        // the engine must never push a negative quantity to a platform.
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 0)]),
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
            .upsert_snapshot("p1", "ebay", None, 0, "2026-01-01 00:00:00")
            .await
            .unwrap();
        harness
            .db
            .upsert_snapshot("p1", "squarespace", None, 9, "2026-01-01 00:00:00")
            .await
            .unwrap();

        harness.engine.run_cycle().await.unwrap();

        // -5 + 0 = -5 against a canonical 5 floors at 0.
        assert_eq!(harness.quantity("p1").await, 0);
        assert!(
            harness.adapter(Platform::Ebay).pushes().is_empty(),
            "eBay already reads 0, so nothing is pushed there"
        );
        assert_eq!(
            harness.adapter(Platform::Squarespace).pushes(),
            vec![("S-1".to_string(), 0)]
        );
        for (item, pushed) in harness.adapter(Platform::Squarespace).pushes() {
            assert!(pushed >= 0, "pushed {pushed} to {item}");
        }
    }

    #[tokio::test]
    async fn an_agreeing_cycle_pushes_nothing_and_writes_no_history() {
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
        assert!(
            history.is_empty(),
            "a no-change cycle writes no history row, got {history:?}"
        );
    }

    #[tokio::test]
    async fn history_records_changes_only_across_cycles() {
        // Cycle 1: eBay drops to 2, canonical follows — one history row.
        // Cycle 2: everything agrees at 2 — no new row (this is the spam
        // the old every-cycle write produced, 288 rows/product/day).
        // Cycle 3: eBay drops to 1 — a second row with the new quantity.
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::Ebay).with_poll_script(vec![
                Poll::items(&[("E-1", 2)]),
                Poll::items(&[("E-1", 2)]),
                Poll::items(&[("E-1", 1)]),
            ])],
            0,
        )
        .await;
        harness
            .add_product("p1", 4, &[(Platform::Ebay, "E-1")])
            .await;

        harness.engine.run_cycle().await.unwrap();
        harness.engine.run_cycle().await.unwrap();
        harness.engine.run_cycle().await.unwrap();

        let history = harness.db.get_inventory_history("p1", None).await.unwrap();
        let quantities: Vec<i64> = history.iter().map(|h| h.quantity).collect();
        assert_eq!(quantities, vec![2, 1]);
    }

    #[tokio::test]
    async fn housekeeping_prunes_old_events_and_downsamples_old_history() {
        // The engine's housekeeping is the only caller of the Db pruning
        // methods: it must delete sync events past the retention window and
        // thin old history to one row per product per day, leaving recent
        // rows untouched.
        let harness = Harness::new(Vec::new(), 0).await;
        harness.add_product("p1", 4, &[]).await;

        let conn = harness.db.connect().await.unwrap();
        conn.execute(
            "INSERT INTO sync_events (product_id, source_platform, target_platform,
                old_quantity, new_quantity, event_type, sync_cycle_id, created_at)
             VALUES ('p1', 'sync_engine', 'ebay', 4, 3, 'quantity_change', 'old-cycle',
                     datetime('now', '-100 days'))",
            (),
        )
        .await
        .unwrap();
        conn.execute(
            "INSERT INTO sync_events (product_id, source_platform, target_platform,
                old_quantity, new_quantity, event_type, sync_cycle_id, created_at)
             VALUES ('p1', 'sync_engine', 'ebay', 3, 3, 'quantity_change', 'recent-cycle',
                     datetime('now'))",
            (),
        )
        .await
        .unwrap();
        // Three history rows on one day 40 days ago, plus one from today.
        for (quantity, recorded_at) in [
            (9, "datetime('now', '-40 days')"),
            (8, "datetime('now', '-40 days', '+1 hour')"),
            (7, "datetime('now', '-40 days', '+2 hours')"),
            (4, "datetime('now')"),
        ] {
            conn.execute(
                &format!(
                    "INSERT INTO inventory_history (product_id, quantity, sync_cycle_id, recorded_at)
                     VALUES ('p1', {quantity}, 'cyc', {recorded_at})"
                ),
                (),
            )
            .await
            .unwrap();
        }
        drop(conn);

        harness.engine.run_housekeeping().await.unwrap();

        let events = harness.db.list_recent_sync_events(10).await.unwrap();
        assert_eq!(events.len(), 1, "only the recent sync event survives");
        assert_eq!(events[0].sync_cycle_id, "recent-cycle");

        let history = harness.db.get_inventory_history("p1", None).await.unwrap();
        assert_eq!(
            history.len(),
            2,
            "old day downsampled to one row, today's row kept: {history:?}"
        );
        assert_eq!(
            history[0].quantity, 9,
            "the day's first row is the survivor"
        );
        assert_eq!(history[1].quantity, 4);
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

        // Partial-failure policy: a platform with no reading this cycle is
        // never pushed to. Its state is unknown, so writing the canonical
        // quantity — resolved without any reading from it — could overwrite
        // live stock with a number derived from its peers.
        assert!(
            harness.adapter(Platform::Squarespace).pushes().is_empty(),
            "no reading means no push, even though canonical moved"
        );
    }

    #[tokio::test]
    async fn a_listing_missing_from_a_successful_poll_receives_no_push() {
        // The poll succeeds but omits one listing (a partial page, a listing
        // suspended mid-cycle). That listing has no reading this cycle, so
        // the policy applies per listing, not just per platform: Squarespace
        // is pushed for the listing it did report, never for the one it did
        // not. S-2's canonical group is a second product whose only reading
        // would have come from S-2 itself.
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 2), ("E-2", 2)]),
                FakeAdapter::new(Platform::Squarespace).with_quantities(&[("S-1", 8)]),
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
        harness
            .add_product(
                "p2",
                8,
                &[(Platform::Ebay, "E-2"), (Platform::Squarespace, "S-2")],
            )
            .await;

        harness.engine.run_cycle().await.unwrap();

        // p1 resolves 8 -> 2 from eBay and pushes Squarespace's reported
        // listing; p2 also resolves to 2, but S-2 was never read.
        assert_eq!(
            harness.adapter(Platform::Squarespace).pushes(),
            vec![("S-1".to_string(), 2)],
            "only the listing actually read this cycle may be pushed"
        );
    }

    #[tokio::test]
    async fn a_stock_mode_platform_whose_sales_detection_fails_receives_no_push() {
        // Stock-mode platforms have no numeric reading: sale detection is
        // their read. When it fails, the platform is unread this cycle, so
        // the partial-failure policy forbids flipping its stock mode even
        // though eBay's reading drives canonical to zero — the desired
        // state for the unread listing would be `out_of_stock`.
        let mut harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 0)]),
                FakeAdapter::new(Platform::XmrBazaar)
                    .with_stock_mode()
                    .failing_sales_detection("sales page unreachable"),
            ],
            0,
        )
        .await;
        harness
            .add_product(
                "p1",
                1,
                &[(Platform::Ebay, "E-1"), (Platform::XmrBazaar, "X-1")],
            )
            .await;

        let (synced, pushed) = harness.engine.run_cycle().await.unwrap();

        assert_eq!(harness.quantity("p1").await, 0);
        assert_eq!((synced, pushed), (1, 0));
        assert!(
            harness.adapter(Platform::XmrBazaar).pushes().is_empty(),
            "no successful stock-mode reading means no stock-mode push"
        );
        let events = harness.drain_events();
        assert!(
            events.iter().any(|e| matches!(
                e,
                SyncEngineEvent::PlatformError {
                    platform: Platform::XmrBazaar,
                    ..
                }
            )),
            "the failed sale detection must surface a PlatformError"
        );
    }

    #[tokio::test]
    async fn a_stock_mode_platform_whose_poll_fails_receives_no_push() {
        // The other stock-mode read: if the poll itself fails, sale
        // detection succeeding is not enough — the platform was not
        // successfully read this cycle and receives no push.
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 0)]),
                FakeAdapter::new(Platform::XmrBazaar)
                    .with_stock_mode()
                    .with_poll_script(vec![Poll::Network("connection refused")]),
            ],
            0,
        )
        .await;
        harness
            .add_product(
                "p1",
                1,
                &[(Platform::Ebay, "E-1"), (Platform::XmrBazaar, "X-1")],
            )
            .await;

        harness.engine.run_cycle().await.unwrap();

        assert_eq!(harness.quantity("p1").await, 0);
        assert!(
            harness.adapter(Platform::XmrBazaar).pushes().is_empty(),
            "a failed poll means no stock-mode push, even if detect_sales succeeded"
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

    #[tokio::test]
    async fn a_failed_push_is_retried_on_the_next_cycle() {
        // Three cycles: the first push to Squarespace fails, the second cycle
        // retries it (the reading still disagrees with canonical), and the
        // third cycle is quiet once the platform agrees.
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 1)]),
                FakeAdapter::new(Platform::Squarespace)
                    .with_poll_script(vec![
                        Poll::items(&[("S-1", 6)]),
                        Poll::items(&[("S-1", 6)]),
                        Poll::items(&[("S-1", 1)]),
                    ])
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

        let squarespace = harness.adapter(Platform::Squarespace);

        // Cycle 1: canonical moves 6 -> 1, the Squarespace push fails.
        harness.engine.run_cycle().await.unwrap();
        assert_eq!(harness.quantity("p1").await, 1);
        assert!(squarespace.pushes().is_empty(), "the push failed");
        assert_eq!(harness.events_of_type("p1", "push_error").await.len(), 1);

        // Cycle 2: the listing still reads 6 against a canonical 1, so the
        // engine retries the push even though canonical did not move.
        squarespace.set_push_error(None);
        let (synced, pushed) = harness.engine.run_cycle().await.unwrap();
        assert_eq!(harness.quantity("p1").await, 1);
        assert_eq!(
            squarespace.pushes(),
            vec![("S-1".to_string(), 1)],
            "the failed push must be retried on the next cycle"
        );
        assert_eq!((synced, pushed), (1, 1));

        // Cycle 3: the platform now agrees with canonical — nothing to do.
        let (_, pushed) = harness.engine.run_cycle().await.unwrap();
        assert_eq!(pushed, 0);
        assert_eq!(squarespace.pushes().len(), 1);
        assert_eq!(harness.quantity("p1").await, 1);
    }

    #[tokio::test]
    async fn a_manual_adjustment_is_pushed_to_platforms_on_the_next_cycle() {
        // Stock changed in Nisaba — typed in by hand or raised by the vendor
        // sync — must reach the platforms, not be overwritten by them. The
        // product is fully synced at 7 on both platforms; then the canonical
        // quantity is raised to 15 outside the engine.
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 7)]),
                FakeAdapter::new(Platform::Squarespace).with_quantities(&[("S-1", 7)]),
            ],
            0,
        )
        .await;
        harness
            .add_product(
                "p1",
                7,
                &[(Platform::Ebay, "E-1"), (Platform::Squarespace, "S-1")],
            )
            .await;
        for platform in ["ebay", "squarespace"] {
            harness
                .db
                .upsert_snapshot("p1", platform, None, 7, "2026-01-01 00:00:00")
                .await
                .unwrap();
        }

        // The manual adjustment (or vendor restock): canonical 7 -> 15.
        harness.db.update_product_quantity("p1", 15).await.unwrap();

        let (synced, pushed) = harness.engine.run_cycle().await.unwrap();

        assert_eq!(harness.quantity("p1").await, 15);
        assert_eq!((synced, pushed), (1, 2));
        assert_eq!(
            harness.adapter(Platform::Ebay).pushes(),
            vec![("E-1".to_string(), 15)]
        );
        assert_eq!(
            harness.adapter(Platform::Squarespace).pushes(),
            vec![("S-1".to_string(), 15)]
        );
    }

    #[tokio::test]
    async fn a_newly_mapped_listing_is_corrected_to_canonical() {
        // The product is synced at 7 on eBay. A Squarespace listing reading 10
        // is mapped afterwards. Its reading must not be absorbed into canonical
        // as a phantom +3 restock; the listing is corrected down to 7.
        let harness = Harness::new(
            vec![
                FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 7)]),
                FakeAdapter::new(Platform::Squarespace).with_quantities(&[("S-1", 10)]),
            ],
            0,
        )
        .await;
        harness
            .add_product("p1", 7, &[(Platform::Ebay, "E-1")])
            .await;
        harness
            .db
            .upsert_snapshot("p1", "ebay", None, 7, "2026-01-01 00:00:00")
            .await
            .unwrap();

        // Map the new listing after the product already has snapshot state.
        harness
            .db
            .insert_mapping("p1", "squarespace", "S-1", None, "v1")
            .await
            .unwrap();

        let (synced, pushed) = harness.engine.run_cycle().await.unwrap();

        assert_eq!(
            harness.quantity("p1").await,
            7,
            "the new listing's reading must not move canonical"
        );
        assert_eq!((synced, pushed), (1, 1));
        assert_eq!(
            harness.adapter(Platform::Squarespace).pushes(),
            vec![("S-1".to_string(), 7)],
            "the new listing is corrected to canonical"
        );
        assert!(
            harness.adapter(Platform::Ebay).pushes().is_empty(),
            "eBay already agrees with canonical"
        );
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
            .get_snapshot("p1", "xmrbazaar", None)
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
        assert_eq!(
            would_push, 2,
            "both platforms are out of step with the resolved 0"
        );

        assert!(
            harness.adapter(Platform::Ebay).pushes().is_empty()
                && harness.adapter(Platform::Squarespace).pushes().is_empty(),
            "a dry run must never call set_quantity"
        );

        let mut planned: Vec<_> = harness
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
        planned.sort_by_key(|(platform, _, _)| platform.as_str().to_string());
        assert_eq!(
            planned,
            vec![(Platform::Ebay, 3, 0), (Platform::Squarespace, 7, 0),]
        );
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
                .get_snapshot("p1", "ebay", None)
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
    async fn a_dry_run_leaves_an_xmr_sale_for_the_real_cycle() {
        // The end-to-end dry-run guarantee for stock-mode sales: the preview
        // changes nothing and consumes nothing, so the real cycle that
        // follows applies the sale exactly once.
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

        assert_eq!(harness.quantity("p1").await, 5, "dry run changes nothing");
        assert!(
            !harness.db.is_xmr_order_processed("order-42").await.unwrap(),
            "a previewed order must still be available to the next real cycle"
        );

        // The fake consumes its sales once, like an order aging out of the
        // scrape; re-arm to model the next cycle seeing the order again.
        // (The real adapter returns its recent sales on every scrape — it
        // keeps no in-memory set, the DB dedup table is the only authority.)
        harness
            .adapter(Platform::XmrBazaar)
            .arm_sales(vec![sale("X-1", 2, "order-42")]);

        // Same DB, same adapters, same canned readings — but for real.
        let (tx, _rx) = mpsc::channel(256);
        let real = SyncEngine::new(harness.db.clone(), harness.engine.adapters.clone(), tx, 0);
        real.run_cycle().await.unwrap();

        assert_eq!(
            harness.quantity("p1").await,
            3,
            "the sale the dry run previewed must be applied by the real cycle"
        );
        assert!(
            harness.db.is_xmr_order_processed("order-42").await.unwrap(),
            "the real cycle consumes the order it applied"
        );
    }

    #[tokio::test]
    async fn an_xmr_sale_past_zero_stock_is_consumed_not_lost() {
        // Quantity cannot go below zero, but the sale was still observed and
        // folded into the resolution — the order must be consumed in the
        // same cycle, or the next cycle would re-apply it forever.
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::XmrBazaar)
                .with_stock_mode()
                .with_sales(vec![sale("X-1", 5, "order-7")])],
            0,
        )
        .await;
        harness
            .add_product("p1", 2, &[(Platform::XmrBazaar, "X-1")])
            .await;

        harness.engine.run_cycle().await.unwrap();

        assert_eq!(
            harness.quantity("p1").await,
            0,
            "the sale is floored at zero, not driven negative"
        );
        assert!(
            harness.db.is_xmr_order_processed("order-7").await.unwrap(),
            "the order must be consumed even though no stock was left to take"
        );

        // Replay: the dedup table must swallow it.
        harness
            .adapter(Platform::XmrBazaar)
            .arm_sales(vec![sale("X-1", 5, "order-7")]);
        harness.engine.run_cycle().await.unwrap();

        assert_eq!(harness.quantity("p1").await, 0);
    }

    #[tokio::test]
    async fn a_sale_on_an_untracked_product_is_neither_applied_nor_consumed() {
        // Untracked products are skipped by quantity resolution, so their
        // sales must stay pending: consuming the order without applying the
        // sale would lose it, and it can be applied once the product is
        // tracked.
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::XmrBazaar)
                .with_stock_mode()
                .with_sales(vec![sale("X-1", 2, "order-untracked")])],
            0,
        )
        .await;
        harness
            .add_untracked_product("p1", 5, &[(Platform::XmrBazaar, "X-1")])
            .await;

        harness.engine.run_cycle().await.unwrap();

        assert_eq!(harness.quantity("p1").await, 5);
        assert!(
            !harness
                .db
                .is_xmr_order_processed("order-untracked")
                .await
                .unwrap(),
            "an untracked sale must not be consumed, so it can be applied once the product is tracked"
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
        assert_eq!(would_push, 2);
        assert!(squarespace.pushes().is_empty());

        // Same DB, same adapters, same canned readings — but for real.
        let real = SyncEngine::new(db.clone(), adapters, tx, 0);
        let (_, pushed) = real.run_cycle().await.unwrap();

        assert_eq!(pushed, would_push, "the preview matched the real cycle");
        assert_eq!(
            squarespace.pushes(),
            vec![("S-1".to_string(), 0)],
            "Squarespace is corrected down to the ledger-resolved 0"
        );
        assert_eq!(db.get_product("p1").await.unwrap().unwrap().quantity, 0);
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

    // ── Adapter lock & vendor task (N-4.8) ────────────────────

    #[tokio::test]
    async fn a_running_cycle_does_not_block_the_adapter_write_lock() {
        // Config saving rebuilds the adapter map under the write lock
        // (`update_config` / `update_company_platform_config` in the app).
        // The cycle used to hold the read lock from its first poll to its
        // last push — retries included — so a save waited for the whole
        // cycle. Now the handles are cloned out and the lock released
        // before the network work begins: with a poll that sleeps for
        // 30 s, the write lock must still be granted effectively at once.
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::Ebay)
                .with_quantities(&[("E-1", 5)])
                .with_poll_delay(std::time::Duration::from_secs(30))],
            0,
        )
        .await;
        harness
            .add_product("p1", 5, &[(Platform::Ebay, "E-1")])
            .await;

        let shared = harness.engine.adapters.clone();
        let adapter = harness.adapter(Platform::Ebay);
        let cycle = tokio::spawn(async move {
            let _ = harness.engine.run_cycle().await;
            harness
        });

        // Wait until the cycle is inside its (sleeping) poll.
        for _ in 0..200 {
            if adapter.poll_calls.load(Ordering::SeqCst) > 0 {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            adapter.poll_calls.load(Ordering::SeqCst) > 0,
            "the cycle never started polling"
        );

        // This is what saving config waits on. If the cycle still held
        // the read lock, this would block until the 30 s poll finished.
        let acquired =
            tokio::time::timeout(std::time::Duration::from_millis(500), shared.write()).await;
        assert!(
            acquired.is_ok(),
            "the adapter write lock (config save) was blocked by a running cycle"
        );
        drop(acquired);

        cycle.abort();
    }

    #[tokio::test]
    async fn vendor_sync_runs_as_its_own_task_and_does_not_block_the_engine_loop() {
        // The vendor callback blocks until the test releases it — the
        // stand-in for a plugin run that may last hours. The engine used
        // to await it inline after each cycle, so Pause / Sync Now queued
        // behind the whole run. Now it is spawned: the loop must process a
        // Sync Now (and complete another cycle) while the first vendor
        // run is still blocked, and must not spawn a second overlapping
        // vendor run.
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 5)])],
            0,
        )
        .await;
        harness
            .add_product("p1", 5, &[(Platform::Ebay, "E-1")])
            .await;

        let vendor_calls = Arc::new(AtomicUsize::new(0));
        let vendor_started = Arc::new(tokio::sync::Notify::new());
        let vendor_release = Arc::new(tokio::sync::Notify::new());
        let vendor_fn: VendorSyncFn = {
            let calls = vendor_calls.clone();
            let started = vendor_started.clone();
            let release = vendor_release.clone();
            Arc::new(move || {
                let calls = calls.clone();
                let started = started.clone();
                let release = release.clone();
                Box::pin(async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    started.notify_one();
                    // Block until released (the test never releases
                    // before its assertions; runtime shutdown cleans up).
                    release.notified().await;
                })
            })
        };

        let Harness {
            engine, mut events, ..
        } = harness;
        let engine = engine.with_vendor_sync(vendor_fn);
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let run_task = tokio::spawn(engine.run(cmd_rx, 3600));

        // The startup tick runs a cycle, then triggers the vendor run.
        tokio::time::timeout(std::time::Duration::from_secs(5), vendor_started.notified())
            .await
            .expect("the vendor sync was never triggered");
        assert_eq!(vendor_calls.load(Ordering::SeqCst), 1);

        // First CycleComplete (from the startup tick) is already queued.
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
            .await
            .expect("no CycleComplete from the startup cycle");
        assert!(matches!(first, Some(SyncEngineEvent::CycleComplete { .. })));

        // While the vendor run is still blocked, Sync Now must be
        // processed and its cycle must complete.
        cmd_tx.send(SyncCommand::SyncNow).await.unwrap();
        let second = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
            .await
            .expect("Sync Now was blocked behind the running vendor sync");
        // A QuantityUpdated could precede it only if quantities moved;
        // they agree here, so the next event is the cycle's completion —
        // but skip anything else defensively until CycleComplete arrives.
        let mut event = second;
        while !matches!(event, Some(SyncEngineEvent::CycleComplete { .. })) {
            event = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
                .await
                .expect("Sync Now cycle never completed behind the vendor run");
        }

        assert_eq!(
            vendor_calls.load(Ordering::SeqCst),
            1,
            "a second vendor run must not be spawned while one is in flight"
        );

        vendor_release.notify_one();
        run_task.abort();
    }

    #[tokio::test]
    async fn reload_applies_new_interval_and_ignores_zero() {
        // Start on an interval so long it would never tick during the
        // test. A reload carrying a 1 s interval must start cycles on the
        // new cadence without a restart; a reload carrying 0 must neither
        // panic (a zero period kills the engine task) nor change the
        // cadence. (One adapter and product are registered because a cycle
        // with no adapters returns before emitting CycleComplete.)
        let harness = Harness::new(
            vec![FakeAdapter::new(Platform::Ebay).with_quantities(&[("E-1", 5)])],
            0,
        )
        .await;
        harness
            .add_product("p1", 5, &[(Platform::Ebay, "E-1")])
            .await;
        let Harness {
            engine, mut events, ..
        } = harness;
        let (cmd_tx, cmd_rx) = mpsc::channel(32);
        let run_task = tokio::spawn(engine.run(cmd_rx, 3600));

        // The startup tick fires one cycle immediately; drain it.
        let first = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
            .await
            .expect("no CycleComplete from the startup cycle");
        assert!(matches!(first, Some(SyncEngineEvent::CycleComplete { .. })));

        // Zero interval: ignored — no panic, no extra cycle, engine alive.
        cmd_tx
            .send(SyncCommand::Reload { interval_secs: 0 })
            .await
            .unwrap();
        let extra =
            tokio::time::timeout(std::time::Duration::from_millis(300), events.recv()).await;
        assert!(
            extra.is_err(),
            "a zero-interval reload must not trigger a cycle"
        );

        // New 1 s interval: a cycle completes on the new cadence.
        cmd_tx
            .send(SyncCommand::Reload { interval_secs: 1 })
            .await
            .unwrap();
        let next = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
            .await
            .expect("reload did not apply the new 1s interval");
        assert!(matches!(next, Some(SyncEngineEvent::CycleComplete { .. })));

        run_task.abort();
    }
}
