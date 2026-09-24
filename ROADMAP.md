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
- [x] **CI's format gate is green** — the workspace was run through rustfmt in one
      mechanical pass; `cargo fmt --all -- --check` now exits clean, so the build, test
      and clippy steps behind it finally run.
- [x] **The workspace could not build on stable at all.** `onyums 0.2.5` — the Tor
      onion-service crate behind `crates/p2p` — carries `#![feature(addr_parse_ascii)]`, so
      `cargo build` on the toolchain CI installs died with `E0554` before anything else ran.
      The format check was failing first, so CI had never reached the build step to report
      it. Fixed by moving to `onyums 0.3.1` (same `serve`/`get_onion_name` API, no nightly
      feature) together with `artiqwest 0.3.0` → `0.4.1`; the pair has to move together
      because both embed arti and their versions must agree.
- [ ] Take `onyums 0.5.0` once `artiqwest` catches up. 0.5 drops the polling hack in
      `start_onion_service` — its builder returns a handle with `onion_address()`,
      `ready()`, `wait_until_settled()` and `shutdown()` instead of a 120s poll on a global
      `get_onion_name()` — and its crypto is pure Rust (no vendored C at all). Blocked
      today: `onyums 0.5.0` needs `arti-client 0.46`, `artiqwest 0.4.1` (the newest) pins
      `0.43`, and cargo cannot resolve both. Both crates are `basic-automation`'s, so the
      unblock is an `artiqwest` release on arti 0.46.
- [x] Clear the clippy backlog behind CI's `-D warnings` gate. With fmt and the build green
      it was finally measurable, and there is no backlog: `cargo clippy --workspace
      --all-targets -- -D warnings` exits 0 across all eight member crates including
      `nisaba-tauri`. The gate should hold from here.
- [x] `crates/core`'s `turso 0.5` pulled in `aegis 0.9.7`, whose vendored C (`libaegis`)
      would not compile. The cause was not `-mtune=native`: libaegis guards its AVX-512
      sources with `target("aes,vaes,avx512f,evex512")`, and both Clang 22 and GCC 16
      dropped the `evex512` token. Clang only *warns* and then discards the whole `target`
      attribute, which disables `avx512f` and turns every intrinsic in the file into a hard
      error. Fixed by `cargo update -p aegis` (0.9.7 → 0.9.19), which vendors a libaegis
      that no longer uses the token.
      Clang 22 removal: https://releases.llvm.org/22.1.0/tools/clang/docs/ReleaseNotes.html
      GCC 16 removal: https://github.com/google/highway/issues/2577
- [x] Normalize line endings — the tree is now LF in both the index and the worktree
      (`git ls-files --eol` reports no CRLF), and `.gitattributes` (`* text=auto eol=lf`,
      plus binary markers for images/fonts) keeps a Windows checkout from reintroducing it.
- [x] Decide `rust-toolchain.toml`: pinned to `stable` with `rustfmt` + `clippy`, matching
      CI, so a dev host defaulting to nightly cannot drift from what CI verifies
- [x] `SECURITY.md` with a disclosure path — GitHub private vulnerability reporting, plus
      what counts as a vulnerability (credential handling, the plugin sandbox, the P2P
      layer, adapter handling of untrusted marketplace responses) and what does not
- [ ] Enable GitHub private vulnerability reporting on the repo (Settings → Code security);
      `SECURITY.md` points contributors at it and it is off by default
- [x] CI's Rust job could not build `nisaba-tauri`: `cargo build` does not run Tauri's
      `beforeBuildCommand` (only `cargo tauri build` does), so `tauri::generate_context!()`
      panicked on the missing `frontendDist` (`frontend/.output/public`, which is
      gitignored). The Rust job now runs `npm ci && npm run generate` before cargo.
- [ ] Screenshots in the README — the UI is the product and there is currently nothing to look at

## Phase 1 — Test and verification foundation

Current state: 79 tests. `crates/core` 40 (`config` 2, `conflict` 6, `crypto` 7, `db` 8,
`sync_engine` 17), `crates/platform-xmrbazaar` 24 (`edit_form` 15, `sales_page` 9),
`crates/platform-squarespace` 15 (`mapping`). The eBay and Amazon adapters, every adapter's
live network path, the P2P layer and the plugin runtime are still untested.

- [ ] Fixture-based tests for each adapter's `mapping.rs`. Squarespace is done
      (`crates/platform-squarespace/tests/mapping.rs`, 15 tests over recorded response
      *shapes*): the products/inventory join, unlimited variants, the variant-name title
      suffix, and `to_full_listing`. eBay and Amazon still have none.
- [ ] Capture real (redacted) API/HTML responses for the fixtures. Everything added so far
      is hand-built from documented response shapes, which catches structural regressions
      but not "the platform changed what it actually sends" — the failure mode that matters
      most for the XMR Bazaar scraper.
- [x] `crates/platform-xmrbazaar/src/scraper.rs`: golden-file tests for `parse_edit_form()`
      (15) and `parse_sales_page()` (9) against saved listing HTML. These found a real
      defect: `parse_edit_form` collected *every* named control on the page and the client
      re-posted all of them, so a routine quantity update also submitted unchecked
      checkboxes as checked (`international_shipping`, `private_listing`), every option of
      each radio group (`stock`, `delivery`, `payment_method`), file inputs as empty
      strings, and the fields of unrelated forms on the page. It now collects only the
      controls a browser would submit, from the form the CSRF token belongs to.
- [x] `SyncEngine` tests over a fake `PlatformAdapter` (17): quantity deltas both ways
      (lowest-stock-wins and restock-net-of-sales), the floor at zero, the
      `has_stock_mode_inventory` path with order deduplication, retry/backoff and the
      auth-refresh branch, and partial platform failure. These found a real defect too:
      `update_snapshot_version_tag` is a plain `UPDATE` and was called *before* the snapshot
      row existed, so the first stock-mode push never persisted its tag and every later
      cycle re-pushed the stock mode to the live XMR Bazaar listing. Reverting the fix
      reproduces it as `version_tag: None`.
- [ ] `crates/vendor-runtime` tests: a fixture plugin exercising `Nisaba.fetch`, `emitBatch`,
      `log`, metadata-only reads, and the transpile path
- [ ] Sandbox escape tests — assert a plugin cannot reach the filesystem, spawn a process, or
      hit the network outside `op_nisaba_fetch`
- [ ] `crates/p2p` round-trip test: `load_full_sync_payload` → encrypt → `merge_remote_payload`
      over loopback, without Tor
- [x] Migration tests: apply every step to an empty DB and assert the resulting schema —
      all 22 expected tables present, the columns `008` restores after `007` rebuilds
      `platform_mappings`, and that a second `migrate()` is a no-op (it runs on every launch).
- [x] Decide what the `008` gap in `migrations/` was — it is intentional and now documented
      on `Db::migrate`: steps whose body is an idempotent `ALTER TABLE` or a data repair are
      written inline in Rust rather than as a `.sql` file (`004`, `008`, `010`, `012`,
      `015`–`016` all are), so `migrations/` was never a complete list. `008` specifically
      repairs the columns `007` drops when it rebuilds `platform_mappings`.
- [ ] Renumber or rename `migrations/*.sql` so a file's number matches its schema version —
      they have drifted (`014_cached_platform_listings.sql` runs at `version < 17`), and the
      `if version < N` gate being the only authority is a trap for a new contributor

## Phase 2 — Platform adapter completeness

The capability matrix is the queue. Current state per `capabilities()`:

| | eBay | Squarespace | XMR Bazaar | Amazon |
|---|---|---|---|---|
| fetch full listing | ✅ | ✅ | ✅ | ✅ |
| fetch/set description | ✅ | ✅ | ✅ | ❌ |
| set price | ✅ | ❌ | ✅ | ✅ |
| upload photos | ❌ | ❌ | ❌ | ❌ |
| create listing | ✅ | ✅ | ✅ | ✅ |

- [ ] Photo upload on eBay. Correcting an earlier reading of this: there is no dead
      implementation to verify or delete — `upload_photo` in
      `crates/platform-ebay/src/lib.rs` is a stub that returns an error explaining that the
      Sell Inventory API has no per-photo upload call, and `can_upload_photos = false` is
      therefore *accurate*. The real work is to set photos the way the Inventory API expects
      — the `imageUrls` array on the inventory item (`client.rs` already threads an
      `image_urls` field through and sends `[]`) — and then decide whether `upload_photo`
      becomes a set-photos operation or the trait grows one.
- [ ] If eBay ever needs a true binary upload rather than `imageUrls`, it must go through
      the Media API (`createImageFromFile` / `createImageFromUrl`); the Trading API's
      `UploadSiteHostedPictures` is decommissioned on 2026-09-30. Nisaba uses the Sell
      Inventory API throughout and so is not exposed to that deadline today.
      https://developer.ebay.com/updates/newsletter/q2_2025
- [ ] Photo upload for Squarespace, XMR Bazaar and Amazon — no implementation at all
- [ ] Squarespace `set_price` — the only platform that cannot be repriced from Nisaba
- [ ] Amazon description read/write via the SP-API listings feed
- [ ] Amazon is `enabled = false` by default in `config.example.toml` and has never been
      exercised end-to-end — run a real sandbox seller account through fetch → set quantity →
      set price, and mark the adapter verified or list what broke
- [x] `detect_sales()` is only meaningful for XMR Bazaar's stock-mode inventory. Confirmed
      correct: `run_cycle` gates the call on `has_stock_mode_inventory`, so eBay, Squarespace
      and Amazon never reach the default impl at all — they are not silently reporting "no
      sales" each cycle. `detect_sales_is_only_called_on_stock_mode_platforms` holds the gate
      in place.
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
      `PlatformSnapshot` state. The cycle does survive a failed poll (there is now a test
      for it), but it then **writes to the platform it could not read**: with no entry in
      `platform_quantities`, `current_on_platform` is `None`, which never equals the
      resolved quantity, so the push always fires — using a canonical value computed
      without any reading from that platform. On a platform that was simply slow to answer
      this overwrites live stock with a number derived from its peers. Decide the policy
      (skip pushes to unpolled platforms, or push only when the last snapshot disagrees)
      and make `one_platform_down_does_not_abort_the_cycle` assert the chosen one.
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
      auto-update cannot work at all until this exists. Concretely: `tauri signer generate`
      produces the pair, the *contents* of the public key (not a path) go in `pubkey`, and
      the release workflow needs `TAURI_SIGNING_PRIVATE_KEY` (plus
      `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` if set) exported as real environment variables.
      https://v2.tauri.app/plugin/updater/
- [ ] Pin `createUpdaterArtifacts` to the v2 format rather than `"v1Compatible"` when the
      release workflow lands — Tauri documents the setting as removed in v3.
      https://v2.tauri.app/plugin/updater/
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

- [x] `config.example.toml` told users to manage products "via the TUI's Products tab";
      it now points at the app's Products pages
- [ ] Secret handling audit — `keyring` is a dependency, but confirm nothing (tokens, the
      company secret, plugin `secret: true` fields) is ever written to `config.toml`, logged,
      or included in an export
- [ ] `export_import.rs` produces `ExportData`; document exactly what it contains and make
      sure secrets are excluded
- [ ] Structured logging levels that are useful in the shipped app, not just `tracing` defaults
- [ ] Dependency freshness pass — `reqwest 0.13`, `deno_core 0.389`, `zip 8` and Nuxt 3.16+
      all move fast; keep them current and green. `turso` is the exception: 0.5.0 is still
      the newest release, and everything published since is `0.8.0-pre.*`, so staying on 0.5
      is correct until a stable 0.8 exists. https://github.com/tursodatabase/turso/releases
- [ ] A scheduled `cargo update` / `cargo audit` CI job. This run found two dependency
      breakages by accident — a transitive C library that stopped compiling on current
      compilers, and a direct dependency that required nightly — and both had been sitting
      in the tree unnoticed because CI never got past the format check.
- [ ] `proc-macro-error2 v2.0.1` is flagged future-incompatible by cargo (`cargo report
      future-incompatibilities`). It arrives via `getset` → `tor-dircommon` → the arti
      stack, so it is fixed by an arti bump rather than anything in this tree — worth
      re-checking whenever `onyums`/`artiqwest` move.
