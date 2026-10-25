# Phase 1 — Test and verification foundation

[← Back to the checklist](../../ROADMAP.md#phase-1--test-and-verification-foundation)

Current state: 191 tests. `crates/core` 47 (`config` 2, `conflict` 6, `crypto` 7, `db` 10,
`sync_engine` 22), `crates/platform-xmrbazaar` 24 (`edit_form` 15, `sales_page` 9),
`crates/platform-ebay` 21 (`mapping`), `crates/platform-squarespace` 15 (`mapping`),
`crates/platform-amazon` 13 (`mapping`), `crates/vendor-runtime` 56 (`runtime` 22,
`sandbox` 9, `limits` 11, `network` 11, `template` 3), `crates/tauri-app` 5 (zip
importer), `crates/p2p` 10 (`round_trip` 9, `server` 1). Every adapter's live network path and the P2P
layer's Tor transport are still untested.

## Fixture-based tests for each adapter's `mapping.rs`, over recorded response *shapes*

Squarespace 15 (the products/inventory join, unlimited variants, the variant-name title suffix), eBay 21 (both the Inventory API's JSON and the Trading API's XML, including `quick_xml`'s `@currencyID`/`$text` handling and the `extras` round-trip), Amazon 13 (the SP-API Listings Items arrays).

## Capture real (redacted) API/HTML responses for the fixtures

Everything added so far is hand-built from documented response shapes, which catches structural regressions but not "the platform changed what it actually sends" — the failure mode that matters most for the XMR Bazaar scraper.

## `crates/platform-xmrbazaar/src/scraper.rs`

Golden-file tests for `parse_edit_form()` (15) and `parse_sales_page()` (9) against saved listing HTML. These found a real defect: `parse_edit_form` collected *every* named control on the page and the client re-posted all of them, so a routine quantity update also submitted unchecked checkboxes as checked (`international_shipping`, `private_listing`), every option of each radio group (`stock`, `delivery`, `payment_method`), file inputs as empty strings, and the fields of unrelated forms on the page. It now collects only the controls a browser would submit, from the form the CSRF token belongs to.

## `SyncEngine` tests over a fake `PlatformAdapter` (17 then, 22 now)

Quantity deltas both ways (lowest-stock-wins and restock-net-of-sales), the floor at zero, the `has_stock_mode_inventory` path with order deduplication, retry/backoff and the auth-refresh branch, and partial platform failure. These found a real defect too: `update_snapshot_version_tag` is a plain `UPDATE` and was called *before* the snapshot row existed, so the first stock-mode push never persisted its tag and every later cycle re-pushed the stock mode to the live XMR Bazaar listing. Reverting the fix reproduces it as `version_tag: None`.

## `crates/vendor-runtime` tests (`tests/runtime.rs`, 22)

Metadata reads and their defaults (without calling `fetchListings`), the TypeScript transpile path and its parse errors, config values that look like code arriving as inert strings, `emitBatch` ordering, `Nisaba.fetch` against a loopback server (method, headers, body, non-2xx, connection failure), `sleep`, `log` routing to `tracing`, and how a throwing, stalled or malformed plugin surfaces as an error.

## Sandbox escape tests

Assert a plugin cannot reach the filesystem, spawn a process, or hit the network outside `op_nisaba_fetch` (`crates/vendor-runtime/tests/sandbox.rs`, 9). Writing them found **three real escapes**, all fixed:

- *Arbitrary file write at install time.* `write_plugin_files` joined each plugin file
  name onto its temp dir unchecked, and the zip importer passes entry names through
  raw, so a zip entry named `../…` or an absolute path was written anywhere the user
  can write — during the metadata read, before the user ever ran the plugin. File
  names must now be plain relative paths, checked before anything touches the disk.
- *Arbitrary module read at run time.* The module loader resolved any `file://`
  specifier, so `await import("file:///…")` read any JS/TS module on disk and handed
  its exports to plugin code, one `Nisaba.fetch` away from exfiltration; `../` also
  reached other plugins' files in the shared temp dir. The loader is now confined to
  the plugin's own directory (canonicalized, so `..` and symlinks cannot walk out),
  and every run gets a fresh directory of its own.
- *Any plugin could abort the app.* `Deno.core.ops` exposed every deno_core built-in
  op to plugin code, including `op_panic`, which panics across the V8 boundary where
  unwinding is impossible — one call `SIGABRT`ed the whole process. The shim now
  captures the ops it needs and deletes `Deno` from the global scope, so `Nisaba` is
  the plugin's only API.

The tests pin that API (`Nisaba`'s keys and the extension's op list), so widening the
sandbox has to be a deliberate change to them.

## `crates/p2p` round-trip test (`tests/round_trip.rs`, 9)

`load_full_sync_payload` → the real wire format (`SyncRequest` JSON) → `merge_remote_payload`, without Tor — every payload section arrives, a repeated sync is a no-op, the newer edit wins both ways, a deleted mapping propagates as a tombstone, and two peers syncing both ways converge. (There is no "encrypt" step to include: payloads carry no application-layer encryption, only Tor's.) The convergence test found a real bug, fixed — see Phase 5.

## Migration tests

Apply every step to an empty DB and assert the resulting schema — all 22 expected tables present, the columns `008` restores after `007` rebuilds `platform_mappings`, and that a second `migrate()` is a no-op (it runs on every launch).

## Decide what the `008` gap in `migrations/` was

It is intentional and now documented on `Db::migrate`: steps whose body is an idempotent `ALTER TABLE` or a data repair are written inline in Rust rather than as a `.sql` file (`004`, `008`, `010`, `012`, `015`–`016` all are), so `migrations/` was never a complete list. `008` specifically repairs the columns `007` drops when it rebuilds `platform_mappings`. Correction (2026-10-19): `004_company_logo.sql` (comments only), `010_always_variants.sql` and `012_variant_vendor_source.sql` *do* exist but are never `include_str!`'d, and the `010` file differs from the inline `migrate_always_variants`. Delete them as part of the renumbering below, and fix the `Db::migrate` doc comment.

## Renumber or rename `migrations/*.sql` so a file's number matches its schema version

Done (N-3.2, 2026-10-20): the only drifted file, `014_cached_platform_listings.sql`, was renamed to `017_cached_platform_listings.sql` to match its `version < 17` gate; the orphan files `004_company_logo.sql`, `010_always_variants.sql` and `012_variant_vendor_source.sql` (never `include_str!`'d) were deleted, and the `Db::migrate` doc comment now states that file numbers equal schema versions and inline steps are the single authority for their numbers. The `if version < N` gate remains the runtime authority.

## Migrations are not transactional

`migrate()` (`db.rs` ~233–686) has no BEGIN/COMMIT and writes `user_version` only at the end. A crash between `DROP` and `RENAME` in the table rebuilds (steps 7, 15, 16) leaves a database on which every later launch fails. Wrap each step in a transaction that bumps `user_version`. Add an upgrade test from a *populated* database at each older version; today's tests only migrate an empty one.

## Multi-cycle sync-engine tests

`a_failed_push_is_recorded_and_does_not_abort_the_cycle` runs one cycle, so it cannot see that the failed push is never retried (Phase 4).
