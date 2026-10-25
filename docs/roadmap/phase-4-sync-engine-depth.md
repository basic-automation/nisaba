# Phase 4 — Sync engine depth

[← Back to the checklist](../../ROADMAP.md#phase-4--sync-engine-depth)

## Quantity deltas and conflict resolution (`resolve_deltas`)

`resolve_deltas` is now the engine's resolution path: each cycle builds per-platform `QuantityDelta`s against the snapshots and sums them onto the canonical quantity.

## Per-sync-cycle vendor data refresh

It updates `products.quantity`, which the ledger now carries to the platforms on the next cycle (see *Stock correctness*).

## Stock correctness

Found by the 2026-10-19 review and still present on PR #2. The engine derives new stock
**only from platform readings** (`sync_engine.rs` ~334–351). It takes the minimum across
platforms, or the maximum plus total decreases when any platform went up. `product.quantity`
is used only when no platform reports.

### Sales on two platforms in one cycle are undercounted

eBay 10 → 8 and Squarespace 10 → 9 now resolve to the summed deltas, 7: each platform's reading becomes a delta against its snapshot and `resolve_deltas` sums them onto canonical.

### Stock changed in Nisaba never reaches the platforms

A restock typed into the app, or vendor stock added by `recalc_product_quantity`, now survives the cycle (deltas are computed against snapshots, not re-derived from readings) and is pushed to every platform whose reading differs from canonical.

### A failed push is never retried, and a newly mapped listing is never corrected

The engine now pushes whenever a platform's *reading* differs from canonical, not only when canonical changed — so a failed push is retried on the next cycle and a new mapping is corrected to canonical. A platform with no reading this cycle is never pushed to.

### Variants are ignored

Mappings carry `variant_id`, but the engine pushes the product's total to every mapping (~432, ~545) and takes the minimum across different variants' listings, so each variant listing advertises the whole product's stock. Fixed in N-4.3: the engine now groups mappings by (product, variant); canonical stock, platform readings/snapshots and pushes are all keyed per variant, with mappings that name no existing variant row falling back to product-level sync.

### Re-mapping an unmapped listing fails

`delete_mapping` soft-deletes (`db.rs` ~1142), but the unique indexes `idx_mappings_product_platform_variant` and `idx_mappings_platform_item` include deleted rows, so `insert_mapping` hits UNIQUE. `create_listing_on_platform` creates the live listing *before* inserting the mapping (`commands/listings.rs` ~351–366), so this leaves an unmapped live listing. Use partial indexes (`WHERE deleted_at IS NULL`), and create the mapping in the same transaction as the record of the listing. Fixed in N-4.5: migration 023 rebuilds `platform_mappings` with both unique indexes partial (`WHERE deleted_at IS NULL`), preserving tombstones; listing creation pre-flights the mapping slot before creating anything remotely and records the mapping with the cached listing record in one transaction (`Db::record_created_listing`).

### An XMR order is marked processed before its sale is applied

(`sync_engine.rs` ~245). Any `?` exit in between, or an untracked product (~272), loses the sale for good. Mark it in the same transaction as the stock write.

### Re-base the engine on a stock ledger

Record every change as a uniquely keyed event: a sale by platform + order id, a restock, a manual adjustment, a vendor update. Canonical stock is the sum, and a push is the difference between canonical and each platform's reading, per variant. This fixes every item above. It is also the exact data model Sync v2 replicates (Phase 5, Step 2), so build it here first, on one machine, and replicate it later.

### Pruning is never wired

`prune_old_sync_events` and `downsample_old_history` have no callers. `record_inventory_history` writes a row per tracked product every cycle even when nothing changed (~354–363), which is 288 rows per product per day at the default 5-minute schedule. Also, `execute_write` (`db.rs` ~134) returns the row count of `COMMIT` rather than of the statement, and commits even when the statement failed. Fixed in N-4.7: the engine's `run_housekeeping` (startup + daily in `SyncEngine::run`) is now the caller — sync events are pruned after 90 days and history is downsampled to one row per product per day after 30 days. `record_inventory_history` inserts only when the quantity differs from the last recorded row, and the engine no longer calls it on unchanged cycles. `execute_write` runs the statement first, commits only on success (a failure rolls back), and returns the statement's affected-row count.

### The vendor sync blocks the platform sync loop

The engine awaits the vendor run inline (`sync_engine.rs` ~736, ~750). Plugin limits now go up to 12 h, and Pause and Sync Now wait for the whole run. Run it as its own task.

### `run_cycle` holds the adapters' read lock for the whole cycle, network retries included (~135–629), so saving config waits for the cycle to finish

Several commands (`listings.rs` ~66–76, `photos.rs`) hold it across fetches too. Clone the adapter handles out, then release the lock.

### A dry-run mode that reports what a sync *would* change without writing to any platform

`SyncEngine::dry_run()`. It suppresses local writes too (snapshots, canonical quantity, history, sync events, the XMR order-dedup table and the vendor-sync callback), so a preview cannot make the following real cycle skip the change it previewed, and reports each planned change as `SyncEngineEvent::DryRunChange`.

### Surface dry-run in the UI and the headless daemon

The engine supports it but nothing offers it to a user yet. A "preview this sync" button is the safest possible way to exercise the adapters against a real account. (N-11.6, 2026-10-23: the dashboard has a "Preview sync" button that runs a one-shot dry-run cycle via `SyncCommand::PreviewSync` and shows each planned `DryRunChange`; the headless-daemon half was retired by N-10.3 when `crates/service` was deleted.)

### Partial-failure semantics

One platform down must not abort the cycle or corrupt `PlatformSnapshot` state. Policy chosen: **never push to a platform that could not be read this cycle** — numeric pushes require a successful poll returning that listing's quantity, and stock-mode pushes require a successful poll plus a successful sale-detection pass. The unread platform is reconciled on the next cycle it can be read. `one_platform_down_does_not_abort_the_cycle` asserts the no-push behaviour, with stock-mode and per-listing successors alongside it; the policy is documented on `SyncEngine::run_cycle` and in README's Sync cycle section.

### Surface a per-cycle reconciliation report in the UI, not just `SyncEvent` rows…

…(N-11.6, 2026-10-23: the dashboard shows a "Last sync cycle" card built from the live event stream — products synced, changes pushed, per-platform change counts, platform errors and unmapped-listing detections.)

### Oversell protection

A configurable reserve buffer so concurrent sales across platforms cannot drive true stock negative. Moved to Phase 5, Sync v2 Step 3 (escrow), where a reserve is a share of stock rights held back from every peer.

### Chart `InventoryHistoryEntry` analytics

There are no charts today (`chart.js` and `vue-chartjs` are installed but never imported); `inventory.vue` shows a count. N-11.8: `inventory.vue` now renders the history as a chart via the new `StockHistoryChart.vue` component (vue-chartjs Line on chart.js).

### The sync schedule accepts anything and can kill the engine

`config.vue` offers free-form cron. `parse_interval` (`tauri-app/src/main.rs` ~492) understands only a minute field of `*/N` and silently uses 300 s for anything else. `*/0` yields a 0 s period, and `tokio::time::interval` panics on zero, which ends the sync engine task. `Reload` never re-reads the interval (`sync_engine.rs` ~764), so a schedule change needs a restart. Replace the field with an interval picker, validate on save, reject zero, and apply changes on reload.
