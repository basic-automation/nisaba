# Cross-cutting

[← Back to the checklist](../../ROADMAP.md#cross-cutting)

## `config.example.toml` told users to manage products "via the TUI's Products tab"

It now points at the app's Products pages

## Secret handling audit

`keyring` is a dependency, but confirm nothing (tokens, the company secret, plugin `secret: true` fields) is ever written to `config.toml`, logged, or included in an export. Plugin secrets already fail this — see Phase 3.

## `Db::open` deletes the WAL when turso panics

(`db.rs` ~50–64), and retries. The comment protects only the main file, but a WAL can hold committed transactions not yet checkpointed into it, so this can throw away recent writes. Back up the files and surface the error instead of deleting. Fixed in PR #5.

## Export/import is lossy

Export contains no secrets (verified 2026-10-19). But:

- import drops `on_hand_quantity`, `source_plugin_id` and `source_vendor_item_id`
  (`db.rs` ~3414), and its `INSERT OR REPLACE` wipes them on variants that already
  exist;
- export (`list_all_mappings`) includes soft-deleted mappings without `deleted_at`,
  so import revives them as active and the engine pushes to them.

`ExportData` lives in `core/src/types.rs` (~397) and is built in
`tauri-app/src/commands/export_import.rs`. It has no tests.

## Config settings that do nothing, and a save that writes secrets

- `build_adapter_config` (`main.rs` ~450–457) resets every platform section, so
  `[ebay]`, `[squarespace]`, `[xmrbazaar]` (including `endpoints`) and `[amazon]` in
  `config.toml` are ignored by the app;
- `general.log_level` and `alerts.default_low_stock_threshold` are never read, though
  the second is editable in `config.vue`;
- `get_config`/`update_config` still round-trip those sections, and `AppConfig::save`
  (`config.rs` ~266) writes them back, along with any `NISABA_*` environment-variable
  secrets merged in at ~259, non-atomically and with default file permissions.

Remove the dead sections, never persist environment-sourced secrets, and write
atomically with `0600`.

## Swallowed errors

`cache_listing_detail` ignores four database writes (`listings.rs` ~538–569), and the pricing snapshot hard-codes `"USD"` (~98).

## Untrusted HTML runs in a webview with full IPC access

CSP is disabled (`tauri.conf.json:31`, `"csp": null`). Marketplace listing descriptions (`products/[id]/index.vue` ~1093) and vendor-plugin descriptions (`DescriptionEditor.vue:143`, reached from `publish.vue`) are rendered with `v-html`. Script in a supplier's, a plugin's or a marketplace listing's HTML can therefore call any Tauri command, including `get_company_secret` and `get_config`, which returns platform secrets. That is a way around the Deno sandbox from the other side. Set a strict CSP with no inline script, sanitize every `v-html` sink (DOMPurify), and narrow the Tauri capability set so the main window cannot read secrets it never displays.

- N-2.7 (2026-10-20) landed the capability-narrowing third: `crates/tauri-app/permissions/`
  allow-lists + `capabilities/settings.json` deny `get_company_secret`, `get_config` and
  `get_company_platform_config` on the main window; those commands are invocable only
  from the dedicated `nisaba-settings` window hosting `/config`, `/company` and the
  existing-company `/setup` flow (a narrow `get_xmrbazaar_monero_address` keeps the
  publish page's payout-address prefill working). REMAINDER: strict CSP (still
  `"csp": null`) and DOMPurify sanitization of the `v-html` sinks — injected script in
  the main window can still invoke every non-secret command.
- 2026-10-28: secrets became write-only instead. No command returns one to any webview:
  `get_config`, `get_company_platform_config` and `get_vendor_plugin_config` return them
  masked (`crates/core/src/masked.rs`), saves keep a masked value as it is, and the company
  secret is shown in a native dialog or copied from Rust (`reveal_company_secret`,
  `copy_company_secret`; `get_company_secret` is retired and `acl_check.rs` keeps it so).
  The `nisaba-settings` window and its capability are gone; Settings is a tabbed page in
  the main window.

## Money is `f64` from end to end (`core/src/types.rs` ~258, ~313)

The XMR and Squarespace adapters serialize prices with `to_string()`, so a computed price can be sent as `0.30000000000000004`. Use a decimal type, or integer minor units plus a currency. N-6.1 (2026-10-20) landed this: `ListingPrice.amount`, `PricingSnapshot.amount`, `PlatformListing.price` and `VendorListingCache.price` are now `rust_decimal::Decimal` (exact JSON-number serde, so the Tauri ↔ frontend contract keeps `amount: number`); all four adapters parse platform price strings into `Decimal` and serialize with `Decimal::to_string` (tests assert a computed `0.1 + 0.2` goes out as `0.3`, never `0.30000000000000004`); the three price columns (`pricing_snapshots.amount`, `vendor_listings_cache.price`, `product_vendor_data.price`) are TEXT holding the exact decimal string via migration 024, which converts legacy REAL values with `CAST`. Plugin `VendorListing.price` is now an exact decimal at the plugin boundary (N-7.2 froze the shape): plugins send the decimal text, a JSON number from a pre-freeze plugin is still accepted via `Decimal::from_f64` (shortest round-trip), and no `f64` remains on the path.

## Nuxt 4 migration

The lockfile pins Nuxt 3.21.1, `radix-vue` 1.9 and Tailwind 3.4. Nuxt 3's maintenance window is closing, and `radix-vue` has been succeeded by `reka-ui`. Plan the move to Nuxt 4 + `reka-ui` (and decide on Tailwind 4) as one migration, behind the typecheck gate from Phase 0.

## Dependency freshness pass

`reqwest 0.13`, `deno_core 0.389`, `zip 8` and Nuxt 3.16+ all move fast; keep them current and green. `deno_core` is now 0.412 (2026-09-16), 23 releases ahead of the tree, and its repo was archived in April 2026 and merged into `denoland/deno` — watch that repo's changelog for breaking changes, not the old one. The upgrade must keep `tests/sandbox.rs` and `tests/limits.rs` green: the op surface, `op_import_sync` and the heap-limit callback are exactly what moves between releases. https://docs.rs/deno_core/latest/deno_core/struct.RuntimeOptions.html https://github.com/denoland/deno_core `turso` is the exception: 0.5.0 is still the newest release, and everything published since is `0.8.0-pre.*`, so staying on 0.5 is correct until a stable 0.8 exists. https://github.com/tursodatabase/turso/releases

## A scheduled `cargo update` / `cargo audit` CI job

This run found two dependency breakages by accident — a transitive C library that stopped compiling on current compilers, and a direct dependency that required nightly — and both had been sitting in the tree unnoticed because CI never got past the format check.

## `proc-macro-error2 v2.0.1` is flagged future-incompatible by cargo (`cargo report future-incompatibilities`)

It arrives via `getset` → `tor-dircommon` → the arti stack, so it is fixed by an arti bump rather than anything in this tree — worth re-checking whenever `onyums`/`artiqwest` move.
