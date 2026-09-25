# Nisaba Roadmap

The single source of truth for work. Every item is a `[ ]`/`[x]` checkbox in phase order.
Shipped consumer-facing capability belongs in `README.md`; everything not-done, discovered
or next belongs here. There is no run log and no changelog — git history and the PRs are
the record.

Tick `[x]` only when an item genuinely shipped and was verified.

---

## Phase 0 — Public release readiness

- [x] MIT `LICENSE` at the repo root, matching `Cargo.toml`'s `workspace.package.license`
- [x] `README.md` — what Nisaba is, architecture, dev/build steps, config, plugin contract
- [x] `ROADMAP.md` — this file
- [x] Remove local scratch tooling with personal paths (`cleanup.ps1`, `parse_har.py`)
- [x] Stop redistributing licensed Adobe Fonts; gitignore `frontend/public/fonts/*.woff2`
      and fall back to system stacks in `tailwind.config.ts` + `app.vue`
- [x] Purge the licensed `.woff2` blobs and the personal-path scripts from git history
- [x] Point the Tauri updater endpoint at this repo instead of the stale
      `nicbudd/nisaba-releases`
- [x] CI: `cargo fmt --check`, `build`, `test`, `clippy -D warnings` on Linux + Windows,
      plus a `npm run generate` frontend build
- [x] Run `cargo fmt --all` across the workspace — 280 hunks, never formatted; the check
      now passes and no longer blocks the rest of CI
- [x] Return the workspace to stable Rust — `onyums 0.2.5` declared
      `#![feature(addr_parse_ascii)]` and failed E0554 on the stable channel. Bumped to
      `onyums 0.3.1` (first stable-clean release) with `artiqwest 0.4.1`; both sit on
      `arti-client 0.43`, and `crates/p2p` needed no code change.
- [ ] Upgrade to `onyums 0.5.x`. **Blocked on artiqwest**: onyums 0.4+ requires
      `arti-client 0.46`, artiqwest is still on `0.43` as of 0.4.1, and the two resolve to
      a single `derive-deftly` whose `~1.6.0` and `~1.11.4` ranges cannot unify. The two
      crates must move to the same arti generation together, so this unblocks only after
      artiqwest adopts `arti-client 0.46`. Worth taking then: 0.5's builder returns an
      `OnionServiceHandle` with the address already known, replacing the 120s polling loop
      in `start_onion_service` with `handle.onion_address()` + `handle.ready()`.
- [x] CI green end to end on Linux and Windows — fmt, build, `cargo test --workspace`
      (20 passed, 0 failed; all in `crates/core`) and `clippy -D warnings`, which turned
      out to have no backlog at all. The Rust jobs generate the frontend first because
      `tauri::generate_context!` embeds it at compile time.
- [ ] `aegis 0.9.7` (reached via `turso 0.5`) fails to compile **on the dev workstation
      only** — its vendored `libaegis` C hits AVX-512 intrinsics under `-mtune=native`
      (`_mm512_xor_si512 requires target feature 'avx512f'`), and its build script pins
      clang, so `CC=gcc` does not help. Confirmed to build fine on the CI runners, so this
      is a local toolchain problem, not a contributor-facing blocker. Worth fixing anyway
      so the workspace builds on this machine.
- [ ] Normalize line endings — the tree carries CRLF from its Windows origin, so a clean
      checkout on Linux shows 56 files modified with whole-file diffs. Add a
      `.gitattributes` (`* text=auto eol=lf`) and normalize in one commit while the tree is
      otherwise clean, or every contributor's PR is a whole-file rewrite.
- [ ] Decide `rust-toolchain.toml`: the dev host is nightly, CI pins stable, and nothing
      in-tree uses `#![feature(...)]` — pin stable explicitly so the two cannot drift
- [ ] `SECURITY.md` with a disclosure contact — the app holds marketplace OAuth tokens and
      a P2P company secret, so a public repo needs a reporting path
- [ ] Screenshots in the README — the UI is the product and there is currently nothing to look at

## Phase 1 — Test and verification foundation

Current state: 20 tests, all in `crates/core` (`config` 2, `conflict` 6, `crypto` 7, `db` 5).
Every adapter, the sync engine, the P2P layer and the plugin runtime are untested.

- [ ] Fixture-based tests for each adapter's `mapping.rs` — record real API/HTML responses
      once, assert the mapping into `PlatformListing`/`PlatformInventoryItem`
- [ ] `crates/platform-xmrbazaar/src/scraper.rs`: golden-file tests for `parse_edit_form()`
      against saved listing HTML — this is the most brittle code in the tree and has zero coverage
- [ ] `SyncEngine` tests over a fake `PlatformAdapter`: quantity deltas, the
      `has_stock_mode_inventory` path, retries, partial platform failure
- [ ] `crates/vendor-runtime` tests: a fixture plugin exercising `Nisaba.fetch`, `emitBatch`,
      `log`, metadata-only reads, and the transpile path
- [ ] Sandbox escape tests — assert a plugin cannot reach the filesystem, spawn a process, or
      hit the network outside `op_nisaba_fetch`
- [ ] `crates/p2p` round-trip test: `load_full_sync_payload` → encrypt → `merge_remote_payload`
      over loopback, without Tor
- [ ] Migration tests: apply `001`–`014` to an empty DB and assert the resulting schema
- [ ] Decide what the `008` gap in `migrations/` was — either document it as intentional or
      renumber, before external contributors trip on it

## Phase 2 — Platform adapter completeness

The capability matrix is the queue. Current state per `capabilities()`:

| | eBay | Squarespace | XMR Bazaar | Amazon |
|---|---|---|---|---|
| fetch full listing | ✅ | ✅ | ✅ | ✅ |
| fetch/set description | ✅ | ✅ | ✅ | ❌ |
| set price | ✅ | ❌ | ✅ | ✅ |
| upload photos | ❌ | ❌ | ❌ | ❌ |
| create listing | ✅ | ✅ | ✅ | ✅ |

- [ ] Photo upload on eBay — `upload_photo` has an implementation in
      `crates/platform-ebay/src/lib.rs:459` but `can_upload_photos` still reports `false`;
      verify it against the real API and flip the flag, or delete the dead path
- [ ] Photo upload for Squarespace, XMR Bazaar and Amazon — no implementation at all
- [ ] Squarespace `set_price` — the only platform that cannot be repriced from Nisaba
- [ ] Amazon description read/write via the SP-API listings feed
- [ ] Amazon is `enabled = false` by default in `config.example.toml` and has never been
      exercised end-to-end — run a real sandbox seller account through fetch → set quantity →
      set price, and mark the adapter verified or list what broke
- [ ] `detect_sales()` is only meaningful for XMR Bazaar's stock-mode inventory; confirm the
      default trait impl is correct for the other three rather than silently returning empty
- [ ] Rate-limit handling per platform — eBay and Amazon both throttle and the adapters
      currently have no backoff distinct from `max_retries`
- [ ] Token refresh failure path: what the UI shows when `refresh_auth()` fails mid-sync
- [ ] A fifth adapter would prove the trait is actually general — Etsy or Shopify is the
      obvious candidate

## Phase 3 — Vendor plugin platform

- [x] Deno-based sandbox with `Nisaba.fetch`/`sleep`/`log`/`emitBatch`
- [x] Metadata read without executing `fetchListings`
- [x] Multi-file plugins with TypeScript transpile
- [x] Rothco Wholesale plugin on the GraphQL v2 API, with SKU variants, tiered pricing,
      UPC and weight
- [ ] Document the plugin contract as a real reference — the `metadata` shape, every
      `config_field` option, the `VendorListing` schema `emitBatch` expects, and the error
      convention. Right now `plugins/rothco-wholesale` is the only specification.
- [ ] A `nisaba-plugin-template` starter plugin so a third party can begin without reading
      the Rothco source
- [ ] Plugin resource limits — a plugin can currently loop forever or emit unbounded batches;
      add a wall-clock timeout, a memory ceiling and a batch cap
- [ ] Per-plugin allowlist of fetchable hosts, surfaced at install time
- [ ] The `marketplace.vue` submit/approve/reject flow implies a plugin registry; decide
      whether that registry is a real hosted service, a P2P-shared list, or local-only, and
      document the answer
- [ ] Plugin signing or checksum pinning before any non-local distribution happens
- [ ] Plugin version upgrade path — what happens to cached vendor listings when a plugin's
      schema changes under them

## Phase 4 — Sync engine depth

- [x] Quantity deltas and conflict resolution (`resolve_deltas`)
- [x] Cache-first listing loads and live/cached merge
- [x] Per-sync-cycle vendor data refresh
- [ ] `conflict.rs` exposes one strategy; make the resolution policy explicit and
      user-selectable (last-writer-wins vs. platform-authoritative vs. manual review)
- [ ] A dry-run mode that reports what a sync *would* change without writing to any platform
- [ ] Partial-failure semantics — one platform down must not abort the cycle or corrupt
      `PlatformSnapshot` state
- [ ] Surface a per-cycle reconciliation report in the UI, not just `SyncEvent` rows
- [ ] Oversell protection: a configurable reserve buffer so concurrent sales across platforms
      cannot drive true stock negative
- [ ] Backfill `InventoryHistoryEntry` analytics into charts beyond the current time windows

## Phase 5 — P2P and multi-company

- [x] Tor onion service, peer client/server, encrypted company sync payloads
- [x] Multi-company data model and company logos
- [ ] Peer trust model — document how a peer is authorized today and what an attacker who
      learns the company secret can do
- [ ] Key rotation for the company secret, with a migration path for existing peers
- [ ] Conflict resolution for P2P merges is `merge_remote_payload`'s implicit policy; make it
      explicit and testable
- [ ] Offline/rejoin behaviour after a peer has been away for longer than the retention window
- [ ] Bounded payload sizes — a full sync payload currently grows with the whole catalog

## Phase 6 — Release and distribution

- [ ] Signing key for the Tauri updater — `tauri.conf.json` ships an empty `pubkey`, so
      auto-update cannot work at all until this exists
- [ ] `.updater/latest.json` published from a real release workflow
- [ ] A `release.yml` that builds Windows and Linux bundles on tag and attaches them to the
      GitHub release
- [ ] macOS build — the icon set includes `icon.icns` and iOS assets, but no macOS build has
      ever been run
- [ ] Decide whether `crates/service` (headless sync daemon) and `crates/tui` ship as real
      products; they are tracked in git but excluded from `[workspace] members`, so they are
      not built, not tested and not covered by CI. Either restore them to the workspace and
      fix whatever broke, or delete them.
- [ ] Version consistency — `Cargo.toml`, `tauri.conf.json` and any updater manifest must be
      bumped together; add a check that fails CI when they disagree

## Phase 7 — Frontend and UX

- [x] Nuxt 3 SPA with Tailwind + shadcn-vue, products/listings/vendors/inventory/companies
- [x] Multi-tier pricing display, dealer price, UPC and weight on vendor cards
- [ ] Empty and error states — the pages assume data exists
- [ ] The publish flow (`products/[id]/publish.vue`) should show, per platform, exactly which
      fields will be written and which are unsupported, driven by `PlatformCapabilities`
- [ ] Accessibility pass: keyboard navigation, focus rings, and contrast against the
      Palenight surfaces
- [ ] `frontend/public/guide/` is an empty directory with a `.gitkeep` — either ship the
      in-app guide it was made for or remove it
- [ ] The window is `"decorations": false`; confirm the custom chrome behaves on Linux/Wayland
      and macOS, not just Windows

## Cross-cutting

- [ ] `config.example.toml` still tells users to manage products "via the TUI's Products tab";
      the app is a Tauri GUI now — fix the stale guidance
- [ ] Secret handling audit — `keyring` is a dependency, but confirm nothing (tokens, the
      company secret, plugin `secret: true` fields) is ever written to `config.toml`, logged,
      or included in an export
- [ ] `export_import.rs` produces `ExportData`; document exactly what it contains and make
      sure secrets are excluded
- [ ] Structured logging levels that are useful in the shipped app, not just `tracing` defaults
- [ ] Dependency freshness pass — `turso 0.5`, `reqwest 0.13`, `deno_core 0.389`, `zip 8`,
      Nuxt 3.16+ all move fast; keep them current and green
