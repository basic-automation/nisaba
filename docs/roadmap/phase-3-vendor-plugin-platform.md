# Phase 3 — Vendor plugin platform

[← Back to the checklist](../../ROADMAP.md#phase-3--vendor-plugin-platform)

## Document the plugin contract as a real reference, and a starter plugin so a third party can begin without reading the Rothco source

Both are `plugins/template`: a working plugin whose comments document the `Nisaba` API, every `metadata` and `config_fields` option, the `VendorListing` schema (variants, recognised `extras` keys), paging, batching, 429/5xx retries, and throw-versus-skip. It is kept honest by `tests/template.rs` (3), which runs it against a fake supplier API on loopback.

## Harden the plugin importer (`extract_zip_to_files_map`)

It unpacked every entry into memory with no cap, so a few kilobytes of zip bomb could exhaust memory at import; it now stops at 1,000 entries and 64 MiB of *decompressed* data (counted while reading, since header sizes are attacker-supplied), and plugins submitted by URL are capped at a 64 MiB download. Entry names with a root, `..` or a backslash (a separator on a Windows peer) are refused with a reason — `ZipFile::enclosed_name` alone would not do, since it quietly turns `/abs` into `abs`. The root-folder strip also no longer needs an explicit directory entry, which many zip tools omit; such a plugin used to fail with "must contain an index.ts". Importer tests: 5, the first in `nisaba-tauri`. Nisaba never calls `ZipArchive::extract`, so the symlink zip-slip in the `zip` crate (CVE-2025-29787) does not reach it. https://github.com/advisories/GHSA-94vh-gphv-8pm8

## Plugin `secret: true` values are stored in plaintext

`set_vendor_plugin_config` writes the whole config map — API tokens included — to `vendor_plugin_registry.config_json` in the unencrypted database, and that column is part of the P2P sync payload. The README claimed they were encrypted; it no longer does. Move secret fields to the OS keyring like the company secret, and decide how a peer that needs the token to run the plugin obtains it. → Done (N-2.3): `secret: true` values are stored in the OS keyring (`nisaba-plugin-config` service, one entry per plugin field) via `Db::set_plugin_config_split`; `config_json` and the P2P payload never carry them (stripped on payload build and on merge). Legacy plaintext values migrate to the keyring on first load and are scrubbed from the DB (`migrate_plugin_secrets_to_keyring`, also run at company-context init). Secrets are per-machine: a peer that needs the token sets it locally.

## Plugin resource limits

`PluginLimits` bounds every run: a wall-clock timeout (a watchdog thread terminates the isolate, since a spinning plugin never yields to tokio's timer), a V8 heap ceiling with a near-limit callback, and an output budget across `emitBatch` and the final result. Defaults are 30 min / 1 GiB / 256 MiB for `fetchListings` and 10 s / 128 MiB / 1 MiB for a metadata read, which runs the module's top-level code at install time. Before this a plugin allocating in a loop hit V8's fatal out-of-memory handler and killed the whole app. `tests/limits.rs` (9).

## `Nisaba.fetch` read every response body fully into Rust memory with no cap

Outside the V8 heap, so the heap limit never saw it. The body is now streamed and refused past `PluginLimits::max_response_bytes` (64 MiB; 1 MiB during a metadata read), up front when `Content-Length` declares it and while counting when it does not.

## Let the user raise a plugin's time limit, for a catalog that genuinely takes longer than 30 minutes (a rate-limited API backing off, say)

It is per machine — a column on the local `vendor_plugin_installs` table (migration 19), never synced — chosen on the installed plugin's card (15 min – 12 h), and applied by all three execution paths (manual fetch, background sync, auto sync). Verified in the running app by the UI smoke test, which changes it and reads it back after leaving the page, and by a direct read of the fixture database.

## Per-plugin allowlist of fetchable hosts

Enforcement. A plugin declares `allowed_hosts` in its metadata (exact names, or `*.example.com` for subdomains) and `Nisaba.fetch` reaches those hosts only, with every redirect hop held to the same list and at most 10 hops. The list is read in an isolate of its own and applied from Rust before the run's plugin code starts, so top-level code cannot widen it; module top-level code gets no network at all (it runs at install time). A plugin that declares nothing stays unrestricted so existing installs keep working. The Rothco plugin now declares `www.rothco.com`. `tests/network.rs` (11).

## Updated plugin code runs without re-approval

Re-importing or re-submitting a plugin with the same `metadata.name` keeps the old `status`, `approved_by` and `config_json`, API secrets included (`commands/vendors.rs` ~576–586, ~694–704). The import then starts `run_vendor_sync_single`, which never checks status (~1485), even for a plugin that is not installed or approved. A P2P merge overwrites `files_json` and `status` the same way (`p2p/src/sync.rs` ~521, ~525), and the next auto-sync runs the new code. A new version that drops `allowed_hosts` therefore becomes Unrestricted without anyone seeing the badge. A changed code hash or allowlist must reset the plugin to pending and show the diff, and the run paths must check status.

## Host-side fetches bypass the plugin allowlist

`cache_images` (`commands/images.rs` ~190–265) fetches any URL the webview hands it, including plugin `image_url` values and variants received from peers, with no host filter and no size cap (`resp.bytes()`), and writes the result to disk. Separately, `FetchPolicy` (`vendor-runtime/src/ops.rs` ~29–52) checks host *names* only, so a plugin whose allowed host resolves to loopback or a LAN address reaches it. Refuse private, loopback and link-local addresses after DNS resolution.

## `AUTO_VENDOR_SYNC_RUNNING` (`vendors.rs` ~1482) is a process-wide flag, so one company's vendor sync skips every other company's, logged only at `debug`

→ Done (N-7.5): the process-wide flag is gone; the auto-sync guard is keyed per company (by database path), so one company's running sync blocks only that company's next run. Covered by a unit test claiming two companies' slots independently.

## Surface each plugin's network access at install time (first install only — in-place updates skip it; see the re-approval item above)

The marketplace cards show the declared hosts, "No network access", or an amber "Unrestricted" warning when a plugin declares none (`PluginNetworkAccess.vue`). The value is a cache on the registry row (migration 18, `vendor_plugin_registry.allowed_hosts`) computed from the plugin's own files at import and recomputed at install — never taken from a P2P peer, whose rows arrive as "checked when installed" — and a plugin whose metadata no longer loads is refused at install. Verified by `npm run generate` and a DB round-trip test; not yet exercised in the running app.

## Once the UI surfaces it, decide when an undeclared allowlist stops meaning "unrestricted"

E.g. refuse new installs without one, keep existing ones working.

## The `marketplace.vue` submit/approve/reject flow implies a plugin registry

Decide whether that registry is a real hosted service, a P2P-shared list, or local-only, and document the answer

## Plugin version upgrade path

What happens to cached vendor listings when a plugin's schema changes under them

## Plugin API v1 (freeze for 1.0)

Today a plugin can do one thing: import a supplier catalog through `fetchListings`. It has
six operations (`fetch`, `sleep`, `log`, `emitBatch`, `store`, `setResult`) and metadata of
`name`, `version`, `description`, `config_fields`, `category`, `icon` and `allowed_hosts`.
1.0 freezes whatever shape exists at that point, so fix the shape first.

### An `api_version` in metadata

The host refuses a major version it does not know and says so at install. The surface frozen at `api_version: 1` is written down in `plugins/template` and `SECURITY.md`.

### Fix `VendorListing` before freezing it

- `image_url` (a single URL) becomes `images: string[]`, with a compat
  fold: a legacy `image_url` from an old plugin becomes `images[0]`;
- `price` is an exact decimal (decimal string from plugins; a JSON number
  is still accepted via the shortest round-trip), with `currency`
  required whenever a price is present — enforced at the core boundary
  (the listing is skipped with a warning) and at import;
- the meaning of `quantity: null` (unknown) versus `0` (out of stock) is
  written down in the template contract and preserved through sync,
  cache, and import — unknown is never coerced to 0;
- UPC/GTIN, brand, MPN, weight (`{ value, unit }`) and dimensions
  (`{ length, width, height, unit }`) become typed fields instead of
  strings in `extras`, because platform create-listing needs them;
- `extras` keeps its free-form role.

→ Done (N-7.2): `VendorListing` (vendor-runtime) and `VendorListingCache`
(core, migration 025 adds `images_json`/`upc`/`brand`/`mpn`/`weight_json`/
`dimensions_json` to both vendor tables; legacy `image_url` rows fold on
read) carry the frozen shape; the template and Rothco plugins emit it;
vendor-runtime tests cover the schema and the `image_url`/numeric-price
compat path; the frozen surface is written down in `plugins/template`
and `SECURITY.md`.

### Secrets from the keyring

A `secret: true` config value is stored in the OS keyring, injected into the run, and never written to `config_json` or the P2P payload. This closes the plaintext item above. → Done (N-2.3): `Db::get_plugin_config_merged` injects keyring secrets into every plugin run path (`fetch_vendor_listings`, `sync_vendor_listings`, `run_vendor_sync_single`); `get_vendor_plugin_config` returns the merged map for the config form. Tests serialise both `config_json` and the P2P payload and assert the secret is absent.

### Per-plugin storage

`Nisaba.store.get/set/delete`, a small key–value store with a size cap that persists between runs. It holds cursors, ETags and last-seen timestamps. → Done (N-7.3): `plugin_kv_store` table (migration 026) in the company DB, scoped to plugin + company and cleared on uninstall; 1 MiB per-plugin cap and 256-byte key limit enforced by the runtime before the write; string values only. Sandbox tests prove cross-plugin isolation and cap enforcement; the API is documented in `plugins/template/index.ts` and `SECURITY.md`.

### Incremental sync

`fetchListings({ since, cursor })` returns only what changed, and the host merges it into the cached catalog. Full catalog pulls every cycle are what hit supplier rate limits and stretch runs toward the 12 h limit. → Done (N-7.4): the runtime always passes `{ since, cursor }` as the second `fetchListings` argument (`{ since: null, cursor: null }` on the first run); a plugin may return a `{ changes, deleted, cursor }` delta instead of the array, which the host merges transactionally (`merge_vendor_listings_cache`: changes upserted by `vendor_item_id`, deleted ids removed, untouched rows keep `fetched_at`). The host's `since`/cursor live in the new `vendor_sync_state` table (migration 027), cleared on uninstall; plugins that ignore the arguments keep full-pull replace semantics. Runtime tests cover an incremental second run changing one listing (plus cursor echo-back), and DB tests cover the merge and the sync-state round-trip. Documented in `plugins/template/index.ts` and `SECURITY.md`.

### Host-managed OAuth for supplier APIs

Metadata declares `oauth` (authorize URL, token URL, scopes). The host runs the flow in its own window and keeps the tokens in the keyring; the plugin receives only a current access token, never the refresh token.

### Feed suppliers

Many wholesalers publish CSV or XML feeds, over HTTPS, SFTP or email, rather than an API. Add host-side, size-bounded `Nisaba.parseCsv` and `Nisaba.parseXml`. Decide whether to support SFTP (an `sftp://` entry in `allowed_hosts`), a `file` config field the host reads with the user's consent, or both.

### Per-plugin schedule (an interval, plus manual runs) and a **run history** in the UI

Start and end time, listings emitted, errors, and the plugin's own log lines (which are already routed into the app log). → Done (N-7.5): migration 028 adds `plugin_run_history` and `vendor_plugin_installs.sync_interval_minutes`. Every run through the shared sync cycle (auto, manual Sync, direct Fetch) records start/end, status, listings emitted, the error on failure, and the plugin's `Nisaba.log` lines (captured per run by the runtime's log collector, which also returns them for failed runs). The marketplace installed card shows the history (`PluginRunHistory.vue`), a per-plugin interval picker plus Run now (`PluginSyncSchedule.vue`); the auto-sync path honours the interval via `plugin_sync_due` against the last recorded run, and manual runs are never gated. DB round-trip, due-ness and runtime log-capture tests cover the pieces.

### Capabilities manifest

Next to `allowed_hosts`, a plugin declares what it does: `catalog` today, and `orders`, `marketplace` and `notify` later. The install card shows the capabilities, the host refuses calls outside them, and a capability change triggers re-approval like a code change.

### Author tooling

→ Done (N-12.2): the `Nisaba` API and the `VendorListing` schema ship as `nisaba.d.ts` in `plugins/template`, generated from the Rust types via ts-rs (`crates/vendor-runtime/src/dts.rs`), so they cannot drift — a sync test fails if the checked-in file differs from the generator's output. The template plugin typechecks against it (`plugins/template/tsconfig.json`). The remaining two sub-bullets (the `nisaba-plugin` dev command, the fake-supplier author test kit) are not 1.0 queue items.

- Publish the `Nisaba` API and the `VendorListing` schema as TypeScript types,
  generated from the Rust types so they cannot drift, in an npm package or as a
  `nisaba.d.ts` in the template.
- A `nisaba-plugin` dev command that runs a plugin against a config, prints its
  batches, validates them against the schema, and packages the zip with its hash.
- The fake-supplier harness from `tests/template.rs`, offered to authors as their own
  test kit.

## Plugin kinds after 1.0

### Marketplace adapter plugins

A TypeScript implementation of the `PlatformAdapter` contract (fetch listings, set quantity, set price, set description, create listing, detect sales), running in the same sandbox behind a Rust bridge that implements the trait. The engine then treats it exactly like a built-in adapter. Phase 2's lesson becomes a host rule: every write returns a structured result (`applied`, `rejected`, `rate_limited`, `auth_failed`), never a bare success. The host also enforces dry-run, so a plugin cannot write during a preview, and supplies idempotency keys and a rate-limit helper. WooCommerce is the first one, then Shopify (*Services and integrations*, WordPress priority).

### Order (dropship) plugins

When a sale of a vendor-sourced variant is detected, call the source plugin's `placeOrder` to submit it to the supplier. An approval queue is on by default, dry-run is supported, and order status is polled afterwards. Rothco is the first candidate. The order carries the buyer's name and address, so decide the retention and redaction rules first, and record them in the privacy statement.

### Notification plugins

Events go out to ntfy, email, Matrix, Slack or Discord: low stock, an oversell incident (Phase 5, Step 3), credentials about to expire, a sync failing, or a plugin needing re-approval.

### Pricing rules

Decide whether these are built in or a plugin kind: markup over dealer price (Rothco's tiered pricing), minimum advertised price, and rounding. The lean is built in, because prices push to every platform and need the same write-safety as stock.
