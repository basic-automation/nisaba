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

Current state: 181 tests. `crates/core` 47 (`config` 2, `conflict` 6, `crypto` 7, `db` 10,
`sync_engine` 22), `crates/platform-xmrbazaar` 24 (`edit_form` 15, `sales_page` 9),
`crates/platform-ebay` 21 (`mapping`), `crates/platform-squarespace` 15 (`mapping`),
`crates/platform-amazon` 13 (`mapping`), `crates/vendor-runtime` 56 (`runtime` 22,
`sandbox` 9, `limits` 11, `network` 11, `template` 3), `crates/tauri-app` 5 (zip importer).
Every adapter's live network path and the P2P layer are still untested.

- [x] Fixture-based tests for each adapter's `mapping.rs`, over recorded response *shapes*:
      Squarespace 15 (the products/inventory join, unlimited variants, the variant-name
      title suffix), eBay 21 (both the Inventory API's JSON and the Trading API's XML,
      including `quick_xml`'s `@currencyID`/`$text` handling and the `extras` round-trip),
      Amazon 13 (the SP-API Listings Items arrays).
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
- [x] `crates/vendor-runtime` tests (`tests/runtime.rs`, 22): metadata reads and their
      defaults (without calling `fetchListings`), the TypeScript transpile path and its
      parse errors, config values that look like code arriving as inert strings, `emitBatch`
      ordering, `Nisaba.fetch` against a loopback server (method, headers, body, non-2xx,
      connection failure), `sleep`, `log` routing to `tracing`, and how a throwing, stalled
      or malformed plugin surfaces as an error.
- [x] Sandbox escape tests — assert a plugin cannot reach the filesystem, spawn a process, or
      hit the network outside `op_nisaba_fetch` (`crates/vendor-runtime/tests/sandbox.rs`,
      9). Writing them found **three real escapes**, all fixed:
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
- [ ] Amazon counts only `fulfillmentAvailability[0]`, so a listing stocked on both
      merchant-fulfilled and FBA reports one channel's quantity — and Amazon does not
      guarantee that array's order. Because the sync engine resolves the minimum across
      platforms, a listing with most of its stock in FBA drags canonical stock down and
      pushes that number to eBay and Squarespace. Decide whether to sum the channels or
      filter to a configured one. Pinned by `only_the_first_fulfilment_channel_is_counted`.
- [ ] Amazon takes `offers[0]` for price with no marketplace filtering, so a seller listed
      in more than one marketplace gets an arbitrary offer, and `to_listing` drops the
      currency. Filter on the configured `marketplace_ids`.
- [ ] Amazon drops ERROR-severity `issues` in every mapping, so a suppressed listing reaches
      the UI as a normal listing with zero stock and no explanation. Carry them through
      `extras` and surface them.
- [ ] eBay's two read paths key listings differently — the Inventory API by SKU, the Trading
      API by ItemID — so a mapping created from one will not match the other. Decide which
      is canonical, or store both.
- [ ] `trading_to_listing` computes `Quantity - QuantitySold` with no floor, so eBay
      reporting more sold than listed yields a negative quantity in the mapping UI and makes
      that listing the minimum in any comparison. The sync engine floors canonical stock at
      zero, so nothing writes it back today.
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
- [x] Document the plugin contract as a real reference, and a starter plugin so a third
      party can begin without reading the Rothco source — both are `plugins/template`: a
      working plugin whose comments document the `Nisaba` API, every `metadata` and
      `config_fields` option, the `VendorListing` schema (variants, recognised `extras`
      keys), paging, batching, 429/5xx retries, and throw-versus-skip. It is kept honest by
      `tests/template.rs` (3), which runs it against a fake supplier API on loopback.
- [x] Harden the plugin importer (`extract_zip_to_files_map`). It unpacked every entry
      into memory with no cap, so a few kilobytes of zip bomb could exhaust memory at import;
      it now stops at 1,000 entries and 64 MiB of *decompressed* data (counted while
      reading, since header sizes are attacker-supplied), and plugins submitted by URL are
      capped at a 64 MiB download. Entry names with a root, `..` or a backslash (a separator
      on a Windows peer) are refused with a reason — `ZipFile::enclosed_name` alone would
      not do, since it quietly turns `/abs` into `abs`. The root-folder strip also no longer
      needs an explicit directory entry, which many zip tools omit; such a plugin used to
      fail with "must contain an index.ts". Importer tests: 5, the first in `nisaba-tauri`.
      Nisaba never calls `ZipArchive::extract`, so the symlink zip-slip in the `zip` crate
      (CVE-2025-29787) does not reach it. https://github.com/advisories/GHSA-94vh-gphv-8pm8
- [ ] **Plugin `secret: true` values are stored in plaintext.** `set_vendor_plugin_config`
      writes the whole config map — API tokens included — to
      `vendor_plugin_registry.config_json` in the unencrypted database, and that column is
      part of the P2P sync payload. The README claimed they were encrypted; it no longer
      does. Move secret fields to the OS keyring like the company secret, and decide how a
      peer that needs the token to run the plugin obtains it.
- [x] Plugin resource limits — `PluginLimits` bounds every run: a wall-clock timeout
      (a watchdog thread terminates the isolate, since a spinning plugin never yields to
      tokio's timer), a V8 heap ceiling with a near-limit callback, and an output budget
      across `emitBatch` and the final result. Defaults are 30 min / 1 GiB / 256 MiB for
      `fetchListings` and 10 s / 128 MiB / 1 MiB for a metadata read, which runs the
      module's top-level code at install time. Before this a plugin allocating in a loop
      hit V8's fatal out-of-memory handler and killed the whole app. `tests/limits.rs` (9).
- [x] `Nisaba.fetch` read every response body fully into Rust memory with no cap — outside
      the V8 heap, so the heap limit never saw it. The body is now streamed and refused
      past `PluginLimits::max_response_bytes` (64 MiB; 1 MiB during a metadata read), up
      front when `Content-Length` declares it and while counting when it does not.
- [x] Let the user raise a plugin's time limit, for a catalog that genuinely takes longer
      than 30 minutes (a rate-limited API backing off, say). It is per machine — a column on
      the local `vendor_plugin_installs` table (migration 19), never synced — chosen on the
      installed plugin's card (15 min – 12 h), and applied by all three execution paths
      (manual fetch, background sync, auto sync). Verified in the running app by the UI
      smoke test, which changes it and reads it back after leaving the page, and by a
      direct read of the fixture database.
- [ ] Show the other limits (heap, output, response size) in the plugin's details, and
      decide whether any of them should be user-adjustable too
- [x] Per-plugin allowlist of fetchable hosts — enforcement. A plugin declares
      `allowed_hosts` in its metadata (exact names, or `*.example.com` for subdomains) and
      `Nisaba.fetch` reaches those hosts only, with every redirect hop held to the same
      list and at most 10 hops. The list is read in an isolate of its own and applied from
      Rust before the run's plugin code starts, so top-level code cannot widen it; module
      top-level code gets no network at all (it runs at install time). A plugin that
      declares nothing stays unrestricted so existing installs keep working. The Rothco
      plugin now declares `www.rothco.com`. `tests/network.rs` (11).
- [x] Surface each plugin's network access at install time. The marketplace cards show
      the declared hosts, "No network access", or an amber "Unrestricted" warning when a
      plugin declares none (`PluginNetworkAccess.vue`). The value is a cache on the registry
      row (migration 18, `vendor_plugin_registry.allowed_hosts`) computed from the plugin's
      own files at import and recomputed at install — never taken from a P2P peer, whose
      rows arrive as "checked when installed" — and a plugin whose metadata no longer loads
      is refused at install. Verified by `npm run generate` and a DB round-trip test; not
      yet exercised in the running app.
- [ ] Once the UI surfaces it, decide when an undeclared allowlist stops meaning
      "unrestricted" — e.g. refuse new installs without one, keep existing ones working.
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
- [x] A dry-run mode that reports what a sync *would* change without writing to any platform
      — `SyncEngine::dry_run()`. It suppresses local writes too (snapshots, canonical
      quantity, history, sync events, the XMR order-dedup table and the vendor-sync
      callback), so a preview cannot make the following real cycle skip the change it
      previewed, and reports each planned change as `SyncEngineEvent::DryRunChange`.
- [ ] Surface dry-run in the UI and the headless daemon — the engine supports it but nothing
      offers it to a user yet. A "preview this sync" button is the safest possible way to
      exercise the adapters against a real account.
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
- [ ] **`[company].enabled` is never read.** `CompanyConfig::enabled` is parsed, but
      `init_company_context` starts the Tor onion service and periodic P2P sync for every
      company unconditionally, so `enabled = false` (the default in
      `config.example.toml`) does not keep an install off Tor. Honouring it would stop P2P
      for anyone who relies on today's behaviour without setting the flag, so decide
      first: gate on it (and set it where it matters), or drop the setting. The README no
      longer claims it works.
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
- [ ] Track the Tauri 3 alphas before they land on a stable line: `v3.0.0-alpha.2`
      (2026-09-21) renames plugin APIs (`js_init_script` → `initialization_script`,
      `Plugin::extend_api` → `Plugin::run_invoke_handler`) and removes
      `Invoke::state`/`state_ref`. Nisaba stays on Tauri 2 until 3 is stable; this is the
      migration checklist's first entry. https://github.com/tauri-apps/tauri/releases
- [ ] `.updater/latest.json` published from a real release workflow
- [x] A `release.yml` that builds **Linux, Windows and macOS** bundles and attaches them to
      the tag's GitHub release — one matrix, `fail-fast: false`, Linux on `ubuntu-22.04` so
      bundles link against an older glibc, macOS as a `universal-apple-darwin` binary
      covering Apple Silicon and Intel. `workflow_dispatch` with `publish` off builds all
      three without publishing, so the matrix can be checked without cutting a release.
      Owner directive 2026-09-25: every release covers all three platforms, and a release
      missing one is a failed release rather than a partial one.
- [x] Prove the release matrix green. Build-only dispatch run 36218681088 (2026-09-26, on
      `master` at `5fd136d`) passed on all three legs and uploaded every bundle: Linux
      `.deb`/`.rpm`/`.AppImage`, Windows `.msi` and NSIS `.exe`, macOS universal `.dmg` —
      the first bundles ever built for this project. They are unsigned (no updater key, no
      Apple identity), so there are no `.sig` files.
      https://github.com/basic-automation/nisaba/actions/runs/36218681088
- [ ] The tag-push path of `release.yml` has never run. As first written it could not have
      worked — each leg ran `gh release upload` against a release nothing created — and it
      uploaded per leg, so one failed leg would have left a published two-platform
      release. Now each leg uploads into a *draft* (the first creates it, pre-release for
      `v0.*`) and a final `publish` job, which only runs when every leg succeeded, checks
      the draft holds an AppImage, a `.deb`, an `.msi` and a `.dmg` before publishing it.
      The shell was syntax-checked and the asset check dry-run against the real bundle
      names; the path itself is unverified until the first `v*` tag.
- [x] `bundle` in `tauri.conf.json` had **no `active` flag, which defaults to `false`** —
      `tauri-utils`' `BundleConfig::active` is `#[serde(default)]` on a `bool`, so
      `cargo tauri build` produced only the executable and never a single bundle, on any
      platform. Found by the first release-matrix run: the build step passed and the upload
      found nothing. Now `"active": true` with an explicit target list
      (`deb`, `rpm`, `appimage`, `app`, `dmg`, `msi`, `nsis`) instead of bundler defaults.
- [ ] macOS code signing and notarization — without an Apple Developer identity the `.dmg`
      is unsigned and Gatekeeper blocks it on first open for most users. Decide whether to
      sign, or document the right-click-Open workaround in the README.
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
- [x] A WebDriver harness that drives the real app: `scripts/ui-smoke.py` plus the
      `ui_fixture` example that seeds a throwaway install. It runs the app from the
      fixture directory (the app reads `./config.toml` first) inside `unshare -rn`, so it
      has no network at all and cannot touch live data, and drives it through
      tauri-driver + WebKitWebDriver. First green run 2026-09-26 on Hyprland: 6 checks on
      the marketplace's network-access badges and the per-plugin time limit. WebKitGTK needs
      `WEBKIT_DISABLE_DMABUF_RENDERER=1` there ("Error 71 (Protocol error) dispatching to
      Wayland display" otherwise).
- [ ] Grow the UI smoke test beyond the marketplace — the product, listings and vendor
      pages, and the empty states above — and decide whether it can run in CI (it needs a
      display; `xvfb-run` on the Linux runner is the obvious route).

## Cross-cutting

- [x] `config.example.toml` told users to manage products "via the TUI's Products tab";
      it now points at the app's Products pages
- [ ] Secret handling audit — `keyring` is a dependency, but confirm nothing (tokens, the
      company secret, plugin `secret: true` fields) is ever written to `config.toml`, logged,
      or included in an export. Plugin secrets already fail this — see Phase 3.
- [ ] `export_import.rs` produces `ExportData`; document exactly what it contains and make
      sure secrets are excluded
- [ ] Structured logging levels that are useful in the shipped app, not just `tracing` defaults
- [ ] Dependency freshness pass — `reqwest 0.13`, `deno_core 0.389`, `zip 8` and Nuxt 3.16+
      all move fast; keep them current and green. `deno_core` is now 0.412 (2026-09-16), 23
      releases ahead of the tree, and its repo was archived in April 2026 and merged into
      `denoland/deno` — watch that repo's changelog for breaking changes, not the old one.
      The upgrade must keep `tests/sandbox.rs` and `tests/limits.rs` green: the op surface,
      `op_import_sync` and the heap-limit callback are exactly what moves between releases.
      https://docs.rs/deno_core/latest/deno_core/struct.RuntimeOptions.html
      https://github.com/denoland/deno_core `turso` is the exception: 0.5.0 is still
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
