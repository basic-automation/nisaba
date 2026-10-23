# Nisaba Roadmap

The single source of truth for work. Every item is a `[ ]`/`[x]` checkbox in phase order.
Shipped consumer-facing capability belongs in `README.md`; everything not-done, discovered
or next belongs here. There is no run log and no changelog — git history and the PRs are
the record.

Tick `[x]` only when an item genuinely shipped and was verified.

---

## Priority queue (code review, 2026-10-19)

The phases below are grouped by area, not by urgency. A four-way code review on 2026-10-19,
of `master` plus PR #2, found defects that lose data, oversell stock, or leak secrets. They
go before any new capability. Each line links to the full item in its phase.

1. **Data loss on disk.** Leaving a legacy-migrated company deletes the app's own database,
   and `Db::open` deletes the WAL after a turso panic, discarding committed transactions
   (Phase 5 *Today's P2P*, Cross-cutting).
2. **Untrusted HTML with full IPC access.** CSP is off and `v-html` renders marketplace and
   plugin HTML, so a supplier's description can read every platform secret (Cross-cutting).
3. **Stock resolution is wrong.** `resolve_deltas` is dead code. Sales on two platforms in
   the same cycle are undercounted. Stock edited in Nisaba never reaches the platforms.
   Variants are ignored. A failed push is never retried (Phase 4 *Stock correctness*).
4. **Writes that report success without writing,** on eBay, Amazon and XMR Bazaar (Phase 2).
5. **Updated plugin code runs without re-approval,** and can silently drop its host
   allowlist (Phase 3).
6. **P2P company sync does not work end to end today.** The join flow cannot produce a
   syncing pair, and the platform-sync round-robin is never called (Phase 5). Do not
   repair the Tor path beyond the data-loss fixes; Sync v2 replaces it.
7. Fix the failing UI smoke job on PR #2, and make the frontend typecheck real (Phases 7, 0).

The *1.0 release* section below turns these into release gates, alongside everything else
that has to be true before tagging `v1.0.0`.

## 1.0 release

1.0 is a promise, not a feature count. It tells users that their data survives every later
update, that `config.toml`, the export format and the plugin API will not break without a
major version, and that the build they install is signed and updates itself. The workspace
is at `0.1.1`. Everything below is the line between that and `v1.0.0`; items in the phases
are referenced rather than repeated.

### Scope decisions (settle first)

- [x] **Is P2P in 1.0?** Decision (owner, 2026-10-19): no. Ship 1.0 as a single-machine product and put
      company sync behind a "Preview" switch that is off by default. Sync v2 (Phase 5) is a
      rewrite of the transport and the data layer, and today's Tor path does not work end
      to end, so holding 1.0 for it would hold everything else. Data-loss fixes in the
      current P2P path still land now; P2P itself is not a 1.0 promise. The exception is the
      **stock ledger** (Phase 4): it changes the database's data model, so it must land
      before 1.0, or 1.0 freezes the model Sync v2 has to migrate away from.
- [x] **Credential model: customers bring their own API keys** (owner decision, 2026-10-19).
      Nisaba's own application for Amazon SP-API keys was denied, so there is no shared
      Amazon app to broker. Every customer enters their own eBay developer keyset, their
      own Amazon SP-API app (LWA client id and secret plus a refresh token), their own
      Squarespace API key and their own XMR Bazaar login. This is what the code already
      does. 1.0 runs no hosted OAuth broker; the setup wizard (*Gate: product*) carries the
      onboarding cost instead. Revisit a shared eBay keyset after 1.0 (see *Services*).
- [x] **Which adapters are "1.0"?** Decision (owner, 2026-10-19). An adapter ships as stable only after a live-account
      verification (below). Anything not verified ships labelled *Beta* in the UI, or
      disabled. **Amazon ships as Beta:** each customer must get their own SP-API developer
      registration approved (bring-your-own SP-API keys), Amazon can refuse it as it refused
      ours, and the adapter has never been verified end to end. The other adapters (eBay,
      Squarespace, XMR Bazaar) ship as stable only if the live-account verification below is
      recorded for them; otherwise they too ship labelled *Beta* or disabled. The setup
      guide says so before the customer starts.

### Gate: correctness

- [ ] Priority queue items 1–5 closed.
- [ ] Phase 4 *Stock correctness* closed, including the stock ledger.
- [ ] Phase 2 *Writes that report success without writing* closed, with a test per write
      path that a rejected write surfaces as `Err`.
- [ ] Live verification per stable adapter, recorded in this file with the date and what
      was exercised: fetch, set quantity, set price, set description and create listing
      against a real (or sandbox) account. Real captured fixtures (Phase 1) come out of the
      same session.
- [ ] A 7-day soak on a real catalog with the dry-run preview (Phase 4) on first and then
      live, with no oversell incident and no unexplained stock change.

### Gate: data safety

- [x] Transactional migrations, plus upgrade tests from a populated database at *every
      released version* (0.1.0 onward), run in CI (Phase 1).
- [x] Automatic database backup before every migration and on a schedule (keep the last N),
      with a *Restore from backup* action in the UI. Backups land next to the database and
      never leave the machine.
- [x] The WAL-deletion and leave-company data-loss bugs fixed (Priority queue item 1) — fixed in PR #5.
- [x] Lossless export/import, round-trip tested (Cross-cutting). From 1.0 the export format
      carries a version and every later release imports every earlier 1.x export.

### Gate: security

- [x] Strict CSP and sanitised `v-html` (Cross-cutting).
- [ ] Plugin re-approval on code change, and plugin secrets in the keyring (Phase 3).
- [ ] Secret-handling audit closed: nothing secret in `config.toml`, logs, exports or P2P
      payloads (Cross-cutting).
- [ ] Tauri capabilities narrowed so the main window can invoke only the commands it uses,
      and secret-returning commands are reachable only from the settings views that
      display them.
- [x] `cargo audit` and `npm audit` clean in CI, on a schedule (Cross-cutting).
- [ ] GitHub private vulnerability reporting enabled (Phase 0).

### Gate: release engineering

- [x] Updater end to end (Phase 6): signing key, `createUpdaterArtifacts`, `latest.json`
      assembled by the workflow, and an endpoint that only a published release can change.
- [ ] **Update-path test.** Install the previous release, put data in it, update to the
      release candidate through the in-app updater, and assert the data and settings
      survived. Run it on all three platforms before every release from 1.0 on.
- [ ] The tag-push release path proven on a `v0.x` tag before it is trusted with `v1.0.0`
      (Phase 6). Treat `-rc.N` tags as pre-releases.
- [ ] Windows code signing, with an Authenticode certificate or a cloud signing service.
      Unsigned installers trigger SmartScreen's "unrecognised app" wall, which most users
      will not click through.
- [ ] macOS Developer ID signing and notarization, or a documented decision to ship
      unsigned with the right-click-Open workaround (Phase 6).
- [x] CI green with no `continue-on-error`: typecheck, UI smoke, a macOS leg, and runners
      moved off deprecated Node 20 actions (Phases 0, 6, 7).
- [ ] A version-consistency check across `Cargo.toml`, `tauri.conf.json` and
      `frontend/package.json` (Phase 6).
- [x] Decide `crates/service` and `crates/tui`: shipped as products with 1.0, or deleted
      (Phase 6). **Decided 2026-10-22 (N-10.3): deleted** — both removed from the repo;
      a headless daemon can return post-1.0, designed against the current per-company
      encrypted config model.

### Gate: product

- [x] **First-run setup wizard.** It walks through each platform's credentials, with links
      to the developer portals, the exact redirect URI (eBay RuName) to register, and a
      *Test connection* button per platform. Because customers bring their own keys, this
      is the difference between a tool its author can use and one a seller can. Amazon's
      step explains the SP-API developer registration (self-authorized private app), the
      roles to request, and that approval is Amazon's decision.
- [x] Credential health in the UI: per platform, show when the token or keys expire (eBay's
      refresh token, Amazon's LWA client-secret rotation), warn ahead of time, and link
      straight back to the wizard step that renews them (Phase 2's token-expiry item).
      [N-11.2: `get_credential_health` command + credential-health panel (dashboard
      alerts, settings full view, status-bar warnings); eBay persists
      `refresh_token_expires_in` to `auth_tokens.refresh_token_expires_at` (migration
      029); Amazon stamps `client_secret_updated_at` on secret change and warns past
      the 365-day rotation window; mid-sync `refresh_auth` failures surface as a
      persistent banner via the `AuthExpired` event; warnings deep-link to
      `/setup?platform=…` (N-11.1 wizard step).]
- [x] Empty and error states, confirmation of destructive actions, and the silent-startup
      fix (Phase 7).
      [N-11.5: `main.rs` no longer `expect()`s on config load, DB open, migrations,
      the rustls provider install, or `Builder::run` — failures go through
      `startup::fatal_startup_error`, which writes a timestamped log to
      `<data dir>/logs/startup-errors/` (`%APPDATA%\nisaba\…` on Windows,
      `~/.config/nisaba/…` on Unix), shows a native OS error dialog
      (MessageBoxW / osascript / zenity-kdialog) naming the failing stage,
      and exits non-zero. Unit tests cover the report text and log-file
      writing.]
- [x] **Export diagnostics**: one button that zips recent logs, versions, the platform and
      the redacted config for a bug report. This replaces any telemetry or crash-reporting
      service (see *Services*).
      [N-11.4: `export_diagnostics` command builds `nisaba-diagnostics.zip`
      (`versions.json`, `logs.txt`, `config.redacted.json`) via a save dialog;
      every secret value is replaced with `[REDACTED]` by key *and* scrubbed
      from the log text; unit tests assert five known secret values appear
      nowhere in the bundle. Button lives on the new Settings → About view
      (`frontend/pages/about.vue`, also openable from the main nav), next to
      the app version from `get_app_version`.]
- [ ] An accessibility pass on the core flows (Phase 7).

### Gate: documentation

- [ ] A user guide: setup, each platform's credentials, mappings and variants, what sync
      does and does not do, backups, and troubleshooting. Either ship it in the empty
      `frontend/public/guide/` or host it.
- [ ] The plugin author reference and published types (Phase 3, *Plugin API v1*).
- [ ] README screenshots (Phase 0).
- [ ] Release notes on every GitHub release, generated from PR titles. (This file stays the
      plan; the release notes are the user-facing record.)

### Gate: legal and platform terms

- [ ] Third-party license notices bundled in the app: an *About → Licenses* view generated
      by `cargo-about` (Rust) and a license checker (npm). Most dependencies are MIT or
      Apache-2.0, and both require shipping their notices with binaries.
- [ ] Each platform's API terms reviewed against what Nisaba does: eBay's API License
      Agreement, Amazon SP-API's Acceptable Use and Data Protection policies (relevant as
      soon as order data with buyer details is read), Squarespace's API terms, and XMR
      Bazaar's terms on automated access, since that adapter scrapes HTML.
- [ ] eBay **Marketplace Account Deletion** notifications. Every production keyset must
      either subscribe an HTTPS endpoint or be granted an exemption for not storing eBay
      user data. With customers bringing their own keys, each customer does this for their
      own keyset, so the setup guide must walk through the exemption. (It would become a
      hosted service only if Nisaba ever ships a shared keyset.) Confirm the current rule
      with eBay before writing the guide.
- [ ] A short privacy statement: what leaves the machine (update checks, marketplace API
      calls, P2P relays if enabled) and what never does.

### Stability contract at 1.0

- [ ] Write down what is frozen and how it versions:
      - the plugin API (`api_version` 1; Phase 3);
      - `config.toml` (a `config_version` key, with migration on load);
      - the export format (versioned, imports every earlier 1.x);
      - the database (forward-only migrations, backup first);
      - the P2P protocol (versioned, if P2P is in).
      Plus a deprecation policy: a deprecated surface keeps working for at least one minor
      release, with a warning in the UI.

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
      unblock is an `artiqwest` release on arti 0.46. **On hold:** Phase 5 moves company
      sync to iroh, which removes `onyums` and `artiqwest` outright unless Tor is kept as
      an opt-in transport — do not spend an `artiqwest` release on this until that is
      decided.
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
- [x] README accuracy. It said "Node.js 18+" (`README.md:63`), but Nuxt 3.21 and Vite 7
      need `^20.19.0 || >=22.12.0`; add an `engines` field to `frontend/package.json` so npm
      enforces the requirement. It says analytics are "charted in the UI", but there are no
      charts: `inventory.vue` prints `history.length`. It also omits that `./config.toml` in
      the working directory overrides the data-dir config (`core/src/config.rs` ~230–235).
- [x] `cleanup.ps1` and `parse_har.py` were purged from history but still sit untracked in
      at least one checkout and are not gitignored, so a `git add -A` re-adds them. Ignore
      them, or delete the local copies.
- [x] **The frontend typecheck has never run.** `ci.yml` runs `npx nuxt typecheck` with
      `continue-on-error: true`, and `vue-tsc` is not a devDependency. On master
      (run 36194520101) `npx` fetched an incompatible `vue-tsc`, which crashed with
      `ERR_PACKAGE_PATH_NOT_EXPORTED './lib/tsc'`. Pin `vue-tsc` as a devDependency, fix the
      errors, and drop `continue-on-error`. There is also no frontend lint or test step.

## Phase 1 — Test and verification foundation

Current state: 191 tests. `crates/core` 47 (`config` 2, `conflict` 6, `crypto` 7, `db` 10,
`sync_engine` 22), `crates/platform-xmrbazaar` 24 (`edit_form` 15, `sales_page` 9),
`crates/platform-ebay` 21 (`mapping`), `crates/platform-squarespace` 15 (`mapping`),
`crates/platform-amazon` 13 (`mapping`), `crates/vendor-runtime` 56 (`runtime` 22,
`sandbox` 9, `limits` 11, `network` 11, `template` 3), `crates/tauri-app` 5 (zip
importer), `crates/p2p` 10 (`round_trip` 9, `server` 1). Every adapter's live network path and the P2P
layer's Tor transport are still untested.

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
- [x] `SyncEngine` tests over a fake `PlatformAdapter` (17 then, 22 now): quantity deltas
      both ways (lowest-stock-wins and restock-net-of-sales), the floor at zero, the
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
- [x] `crates/p2p` round-trip test (`tests/round_trip.rs`, 9): `load_full_sync_payload` →
      the real wire format (`SyncRequest` JSON) → `merge_remote_payload`, without Tor —
      every payload section arrives, a repeated sync is a no-op, the newer edit wins both
      ways, a deleted mapping propagates as a tombstone, and two peers syncing both ways
      converge. (There is no "encrypt" step to include: payloads carry no application-layer
      encryption, only Tor's.) The convergence test found a real bug, fixed — see Phase 5.
- [x] Migration tests: apply every step to an empty DB and assert the resulting schema —
      all 22 expected tables present, the columns `008` restores after `007` rebuilds
      `platform_mappings`, and that a second `migrate()` is a no-op (it runs on every launch).
- [x] Decide what the `008` gap in `migrations/` was — it is intentional and now documented
      on `Db::migrate`: steps whose body is an idempotent `ALTER TABLE` or a data repair are
      written inline in Rust rather than as a `.sql` file (`004`, `008`, `010`, `012`,
      `015`–`016` all are), so `migrations/` was never a complete list. `008` specifically
      repairs the columns `007` drops when it rebuilds `platform_mappings`. Correction
      (2026-10-19): `004_company_logo.sql` (comments only), `010_always_variants.sql` and
      `012_variant_vendor_source.sql` *do* exist but are never `include_str!`'d, and the
      `010` file differs from the inline `migrate_always_variants`. Delete them as part of
      the renumbering below, and fix the `Db::migrate` doc comment.
- [x] Renumber or rename `migrations/*.sql` so a file's number matches its schema version —
      done (N-3.2, 2026-10-20): the only drifted file,
      `014_cached_platform_listings.sql`, was renamed to
      `017_cached_platform_listings.sql` to match its `version < 17` gate; the
      orphan files `004_company_logo.sql`, `010_always_variants.sql` and
      `012_variant_vendor_source.sql` (never `include_str!`'d) were deleted, and
      the `Db::migrate` doc comment now states that file numbers equal schema
      versions and inline steps are the single authority for their numbers. The
      `if version < N` gate remains the runtime authority.
- [x] **Migrations are not transactional.** `migrate()` (`db.rs` ~233–686) has no
      BEGIN/COMMIT and writes `user_version` only at the end. A crash between `DROP` and
      `RENAME` in the table rebuilds (steps 7, 15, 16) leaves a database on which every later
      launch fails. Wrap each step in a transaction that bumps `user_version`. Add an
      upgrade test from a *populated* database at each older version; today's tests only
      migrate an empty one.
- [ ] Multi-cycle sync-engine tests. `a_failed_push_is_recorded_and_does_not_abort_the_cycle`
      runs one cycle, so it cannot see that the failed push is never retried (Phase 4).

## Phase 2 — Platform adapter completeness

The capability matrix is the queue. Current state per `capabilities()`. A ✅ means the
adapter *implements* the method, **not** that a write was verified against a live account.
The review on 2026-10-19 found several ✅ write paths that return `Ok` without changing
anything on the platform (see "Writes that report success without writing" below):

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
      — the `imageUrls` array on the inventory item — and then decide whether `upload_photo`
      becomes a set-photos operation or the trait grows one. Correction (2026-10-19): photos
      at *create* time already work. `create_listing` sends `photo_urls` as `imageUrls`
      (`lib.rs` ~625), and the PUT paths keep existing images. What is missing is changing
      the photos of an existing listing.
- [ ] If eBay ever needs a true binary upload rather than `imageUrls`, it must go through
      the Media API (`createImageFromFile` / `createImageFromUrl`); the Trading API's
      `UploadSiteHostedPictures` is decommissioned on 2026-09-30. Nisaba uses the Sell
      Inventory API throughout and so is not exposed to that deadline today.
      https://developer.ebay.com/updates/newsletter/q2_2025
- [ ] Photo upload for Squarespace and Amazon — no implementation at all. XMR Bazaar's
      `create_listing` already uploads up to 6 photos as multipart parts (`client.rs`
      ~505–610), so for XMR the gap is changing photos on an existing listing.
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
- [x] Amazon counts only `fulfillmentAvailability[0]`, so a listing stocked on both
      merchant-fulfilled and FBA reports one channel's quantity — and Amazon does not
      guarantee that array's order. Because the sync engine resolves the minimum across
      platforms, a listing with most of its stock in FBA drags canonical stock down and
      pushes that number to eBay and Squarespace. Decide whether to sum the channels or
      filter to a configured one. Pinned by `only_the_first_fulfilment_channel_is_counted`.
      [N-5.4: quantity now sums across all fulfilment channels deterministically;
      the pinned test is replaced by `quantities_sum_across_fulfilment_channels` (13 for the cap).]
- [x] Amazon takes `offers[0]` for price with no marketplace filtering, so a seller listed
      in more than one marketplace gets an arbitrary offer, and `to_listing` drops the
      currency. Filter on the configured `marketplace_ids`. [N-5.4: price is selected from
      the configured marketplace in configured order and `ListingPrice` keeps its currency.]
- [x] Amazon drops ERROR-severity `issues` in every mapping, so a suppressed listing reaches
      the UI as a normal listing with zero stock and no explanation. Carry them through
      `extras` and surface them. [N-5.4: ERROR issues are carried through `FullListing.extras`
      (`amazon_error_issues` / count / per-issue entries); `fetch_listings` now requests `issues`.]
- [ ] eBay's two read paths key listings differently — the Inventory API by SKU, the Trading
      API by ItemID — so a mapping created from one will not match the other. Decide which
      is canonical, or store both.
- [x] `trading_to_listing` computes `Quantity - QuantitySold` with no floor, so eBay
      reporting more sold than listed yields a negative quantity in the mapping UI and makes
      that listing the minimum in any comparison. The sync engine floors canonical stock at
      zero, so nothing writes it back today. [N-5.6: floored at 0 in both
      `trading_to_listing` and `trading_to_full_listing` (`mapping.rs`), pinned by the
      updated mapping fixture test (2 listed, 5 sold -> 0).]
- [ ] Rate-limit handling per platform. eBay, Amazon and Squarespace all throttle, and the
      adapters have no backoff distinct from `max_retries`. Squarespace also rescans the
      whole catalog for every per-item read, which multiplies its request count.
- [x] Token refresh failure path: what the UI shows when `refresh_auth()` fails mid-sync.
      Also warn *before* expiry. eBay's `refresh_token_expires_in` is parsed but unused
      (`platform-ebay/src/types.rs:11`), and Amazon LWA client secrets have a rotation
      deadline; check the current window before building this. [N-11.2: a failed
      `refresh_auth` mid-sync emits `AuthExpired`, which now surfaces as a persistent
      dashboard banner with a "Renew in setup wizard" deep-link (not a silent
      tooltip change); advance warnings come from the credential-health panel —
      eBay's refresh-token expiry is persisted from the token response, Amazon's
      client-secret rotation is tracked from `client_secret_updated_at`.]
- [ ] A fifth adapter would prove the trait is actually general. Superseded: the next
      adapter is WooCommerce, then Shopify, built as marketplace plugins (Phase 3,
      *Plugin kinds after 1.0*; *Services and integrations*).
- [x] eBay OAuth sends no `state` parameter (`platform-ebay/src/auth.rs` ~54–60), and the
      callback accepts a `code` from any page the auth window reaches (~279–283). Add and
      check `state`.
- [ ] Four `SyncError` variants are never constructed: `RateLimited`, `EbayRevisionLimit`,
      `ConflictError` and `PlatformDisabled` (`core/src/error.rs` ~12–34). In particular,
      nothing counts eBay's per-listing daily revision limit. A sync schedule that pushes
      often enough will hit it, and the failure looks like any other API error.

### Writes that report success without writing

Found by the 2026-10-19 review, on both `master` and PR #2 (which touches no adapter).
Line numbers are from that review. This is the highest-risk gap in the project: the sync
engine treats `Ok` as "the platform now holds this quantity". A silent no-op therefore
leaves a sold-out listing live on another marketplace, and nothing in the UI says so. Fix
these before adding new capabilities, and give every write path a test that a rejected
write surfaces as `Err`.

- [x] **eBay `set_quantity` swallows per-SKU errors.** `bulk_update_price_quantity`
      (`platform-ebay/src/client.rs` ~177–195) logs each item error at `warn!` and returns
      `Ok(())`, and `statusCode` is parsed but never read (`types.rs:81`). Listings created
      on eBay's website are keyed by ItemID; `set_quantity` sends that ItemID as a SKU, so
      every push to such a listing no-ops behind an `Ok`. Fails with the "two read paths"
      item above.
- [x] **eBay `set_price` probably never changes the price.** The `offers: [{price}]` entry
      carries no `offerId` (`client.rs:633`), and the 200/207 body is never parsed. When
      `availability` is absent, the same path falls back to quantity `0` (`lib.rs:454`), so
      a price edit can zero stock. Verify against the Inventory API docs and a sandbox.
- [x] **eBay read-modify-write erases fields it doesn't model.** `EbayInventoryItem`
      (`types.rs` ~16–31) carries only title, description, imageUrls, aspects, condition and
      quantity. `createOrReplaceInventoryItem` is a full replace, so every description edit
      (`client.rs` ~406, ~756) drops brand, MPN, UPC, `packageWeightAndSize` and
      `conditionDescriptors`, and on the `set_description` path `conditionDescription`
      too. It also writes back a quantity read earlier, which may be stale by then. Model
      the item losslessly by carrying unknown fields through, or switch to a partial
      update. [N-5.2: `EbayInventoryItem`/`EbayProductInfo`/`EbayAvailability`
      now model brand/MPN/UPC/EAN/ISBN/ePID, `conditionDescription`,
      `conditionDescriptors` and `packageWeightAndSize` explicitly and carry
      every other field through `#[serde(flatten)]` extra maps;
      `build_inventory_item_put_body` rebuilds the full-replace body from the
      just-fetched item, applies only the caller's overrides, and omits
      quantity entirely so a stale read can never resurrect sold stock
      (fixture tests in `tests/read_modify_write.rs`).]
- [x] **eBay `update_listing` sends a partial body to `updateOffer`**, which is also a full
      replace (`lib.rs` ~543–568): it drops price and quantity, or is rejected.
      [N-5.2: `update_listing` now goes through
      `update_offer_fields` — fetch the complete offer, rebuild a complete
      body losslessly via `build_offer_put_body`, PUT that — and
      `update_offer` itself validates completeness and refuses a partial
      body before sending; a missing offer or a fetched offer lacking a
      required field is an explicit error, as is a tags edit eBay cannot
      represent.]
- [x] **Amazon PATCH bodies are double-wrapped, and failures report `Ok`.**
      `AmazonPatch::value` is a `Vec<Value>`, and the client pushes `json!([{…}])` into it
      (`platform-amazon/src/client.rs` ~214, ~299), which serializes as `"value": [[{…}]]`.
      Even when Amazon marks the submission INVALID, `update_quantity` only warns
      (~252), `update_price` never reads the body, and `create_listing` discards the
      submission (`lib.rs` ~360).
      [N-5.3: PATCH bodies are now built by `build_quantity_patch_request` /
      `build_price_patch_request` with the attribute object directly in `value`
      (`"value": [{…}]`, pinned by serialisation tests); all three write paths
      parse the submission and pass it through `check_submission`, which
      returns `Err` on an `INVALID` status or any `ERROR`-severity issue —
      `update_price` now reads its response body and the adapter's
      `create_listing` checks the submission instead of discarding it.
      Fake-server tests prove a rejected write surfaces as `Err` for each of
      `update_quantity`, `update_price` and `create_listing`.]
- [x] **Amazon pagination may read the wrong key.** `types.rs:17` expects a top-level
      `nextToken`. If `searchListingsItems` nests it under `pagination`, only the first 20
      SKUs per marketplace are ever read. The hand-built fixture encodes the same
      assumption, which is the case for real captured fixtures (Phase 1). [N-5.4: the token
      is now read from the documented nested `pagination.nextToken` (with a legacy
      top-level fallback), pinned by the fixture's `pagination` block.]
- [x] **XMR Bazaar writes cannot tell success from failure.** `client.rs` ~174, ~392 and
      ~766 accept any final 2xx/3xx on a client that follows redirects, so a POST bounced
      to `/login/` or a re-rendered form with errors counts as success. `set_stock_mode`
      also returns `Ok` when no stock field is found (~343). Detect a successful save
      positively: the redirect target, or the saved value read back.
      [N-5.5: all edit writes (`update_quantity`, `set_stock_mode`,
      `update_listing_field`) now POST with a no-redirect client via
      `submit_edit_and_verify` — a redirect to `/login/` is an `AuthError`, a
      200 carrying form errors or the login page is an `Err`, and the saved
      values must read back from a fresh GET of the edit form before `Ok`.
      `set_stock_mode` returns `Err` when no stock field exists. Fake-server
      tests cover the login-bounce, error-form and redirect-without-save
      cases for each path.]
- [x] **XMR `create_listing` can record the wrong listing id.** It takes `listings.first()`
      as the newest listing (~650), and on a 200 body it accepts any `/listing/<id>/` link
      (~662). A wrong mapping points later stock toggles at an unrelated listing.
      [N-5.5: `create_listing` snapshots my-listings before creating and
      identifies the created listing deterministically — the id in the
      redirect `Location`, or the pre/post diff of my-listings (exactly one
      new listing, or exactly one new listing carrying the requested title).
      Body links are never trusted and `listings.first()` is gone; a login
      bounce, an error form, no new listing, or an ambiguous diff is an
      `Err`. Fake-server tests pin each case, including a decoy body link.]

### Adapter correctness

- [x] The in-memory order dedup in the XMR adapter consumes sales: orders are marked seen
      before the engine processes them (`platform-xmrbazaar/src/lib.rs` ~121), and
      `run_cycle` calls `detect_sales` in dry-run too (`sync_engine.rs` ~189). A dry run can
      then make the next real cycle skip a sale, which breaks Phase 4's dry-run guarantee.
      A cycle that errors mid-batch also loses its sales. The DB dedup table already exists,
      so remove the in-memory set. [N-4.4: removed `seen_orders`; the engine now defers
      order marks and applies them in the same transaction as the stock write that
      consumes the sale (`Db::apply_stock_and_consume_xmr_sales`); a dry run consumes
      nothing and a failed cycle can no longer lose a sale.]
- [ ] XMR orders always count as one unit (`scraper.rs` ~778, pinned by `sales_page.rs:71`).
      Confirm whether XMR Bazaar allows multi-unit orders in unlimited-stock mode; if it
      does, read the quantity.
- [x] XMR listing status: `status.contains("active")` also matches `"inactive"`
      (`scraper.rs:146`), so an inactive listing reads as in stock. `parse_listings` has no
      test. [N-5.6: exact `status == "active"` match; `parse_listings` now has tests
      (`tests/listings.rs`) covering active/inactive/other statuses.]
- [x] XMR error snippets slice `&body[..body.len().min(N)]` (`client.rs` ~183, ~407, ~672,
      ~776), which panics when byte N splits a UTF-8 character in the remote HTML. Truncate
      on a char boundary. [N-5.6: all four sites go through `client::body_snippet`, which
      backs off to a char boundary; pinned by unit tests with multibyte bodies.]
- [x] XMR `set_price` drops the currency (`lib.rs` ~290), and `set_description` posts raw
      HTML (~300) while `create_listing` strips tags (`client.rs` ~476). Pick one behaviour.
      [N-5.6: `set_price` submits price + currency in one form POST
      (`client::update_listing_fields`); `set_description` strips tags via
      `strip_html_tags`, matching `create_listing`. Both pinned by unit tests.]
- [x] eBay pagination ignores the `next` href (`types.rs` ~50, ~101). If `getOffers`
      requires `sku`, as the docs suggest, `fetch_all_offers` (`client.rs` ~314) always
      fails. That failure is only warned, so Inventory-API listings would have no prices.
      [N-5.6: `fetch_inventory_items`, `fetch_offers` and `fetch_all_offers` follow the
      response's `next` href via `client::next_page_url` (absent/empty/self-referential
      hrefs end pagination; offset arithmetic remains the fallback), pinned by a
      fake-server test asserting the second request goes to the href's offset.]
- [x] Squarespace pagination loops forever if `hasNextPage` is true with a null cursor
      (`client.rs` ~81, ~135), and only `fetch_inventory` maps a 401 to `AuthError` (~57).
      [N-5.6: both loops advance only via `client::next_cursor`, which terminates on a
      null/empty/repeated cursor even when `hasNextPage` is true; `fetch_products` now
      also maps 401 to `AuthError`. Pinned by unit tests.]
- [x] Squarespace `update_listing` silently drops `request.price` (`lib.rs` ~324–336).
      Return an error, as the capability matrix already says it cannot set a price.
      [N-5.6: returns `Err` before any other field is written when `request.price` is
      present; pinned by a unit test.]
- [ ] Hard-coded regions: eBay always uses `EBAY_US` (`lib.rs` ~653) and `EbayConfig` has
      no marketplace field; Amazon's consent URL is North America only (`auth.rs:9`)
      despite the configured `region`.
- [ ] Dead code: the XMR `stock_mode_field` cache is never written (`lib.rs` ~33, ~91), and
      `client.update_quantity` (~100–190) is unused.

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
- [x] **Plugin `secret: true` values are stored in plaintext.** `set_vendor_plugin_config`
      writes the whole config map — API tokens included — to
      `vendor_plugin_registry.config_json` in the unencrypted database, and that column is
      part of the P2P sync payload. The README claimed they were encrypted; it no longer
      does. Move secret fields to the OS keyring like the company secret, and decide how a
      peer that needs the token to run the plugin obtains it.
      → Done (N-2.3): `secret: true` values are stored in the OS keyring
      (`nisaba-plugin-config` service, one entry per plugin field) via
      `Db::set_plugin_config_split`; `config_json` and the P2P payload never carry
      them (stripped on payload build and on merge). Legacy plaintext values migrate
      to the keyring on first load and are scrubbed from the DB
      (`migrate_plugin_secrets_to_keyring`, also run at company-context init).
      Secrets are per-machine: a peer that needs the token sets it locally.
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
- [x] **Updated plugin code runs without re-approval.** Re-importing or re-submitting a
      plugin with the same `metadata.name` keeps the old `status`, `approved_by` and
      `config_json`, API secrets included (`commands/vendors.rs` ~576–586, ~694–704). The
      import then starts `run_vendor_sync_single`, which never checks status (~1485), even
      for a plugin that is not installed or approved. A P2P merge overwrites `files_json`
      and `status` the same way (`p2p/src/sync.rs` ~521, ~525), and the next auto-sync runs
      the new code. A new version that drops `allowed_hosts` therefore becomes
      Unrestricted without anyone seeing the badge. A changed code hash or allowlist must
      reset the plugin to pending and show the diff, and the run paths must check status.
- [x] Host-side fetches bypass the plugin allowlist. `cache_images` (`commands/images.rs`
      ~190–265) fetches any URL the webview hands it, including plugin `image_url` values
      and variants received from peers, with no host filter and no size cap
      (`resp.bytes()`), and writes the result to disk. Separately, `FetchPolicy`
      (`vendor-runtime/src/ops.rs` ~29–52) checks host *names* only, so a plugin whose
      allowed host resolves to loopback or a LAN address reaches it. Refuse private,
      loopback and link-local addresses after DNS resolution.
- [x] `AUTO_VENDOR_SYNC_RUNNING` (`vendors.rs` ~1482) is a process-wide flag, so one
      company's vendor sync skips every other company's, logged only at `debug`.
      → Done (N-7.5): the process-wide flag is gone; the auto-sync guard is
      keyed per company (by database path), so one company's running sync
      blocks only that company's next run. Covered by a unit test claiming
      two companies' slots independently.
- [x] Surface each plugin's network access at install time (first install only — in-place
      updates skip it; see the re-approval item above). The marketplace cards show
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

### Plugin API v1 (freeze for 1.0)

Today a plugin can do one thing: import a supplier catalog through `fetchListings`. It has
six operations (`fetch`, `sleep`, `log`, `emitBatch`, `store`, `setResult`) and metadata of
`name`, `version`, `description`, `config_fields`, `category`, `icon` and `allowed_hosts`.
1.0 freezes whatever shape exists at that point, so fix the shape first.

- [x] An `api_version` in metadata. The host refuses a major version it does not know and
      says so at install. The surface frozen at `api_version: 1` is written down in
      `plugins/template` and `SECURITY.md`.
- [x] Fix `VendorListing` before freezing it:
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
- [x] **Secrets from the keyring.** A `secret: true` config value is stored in the OS
      keyring, injected into the run, and never written to `config_json` or the P2P
      payload. This closes the plaintext item above.
      → Done (N-2.3): `Db::get_plugin_config_merged` injects keyring secrets into
      every plugin run path (`fetch_vendor_listings`, `sync_vendor_listings`,
      `run_vendor_sync_single`); `get_vendor_plugin_config` returns the merged map
      for the config form. Tests serialise both `config_json` and the P2P payload
      and assert the secret is absent.
- [x] **Per-plugin storage**: `Nisaba.store.get/set/delete`, a small key–value store with a
      size cap that persists between runs. It holds cursors, ETags and last-seen
      timestamps.
      → Done (N-7.3): `plugin_kv_store` table (migration 026) in the company DB,
      scoped to plugin + company and cleared on uninstall; 1 MiB per-plugin cap
      and 256-byte key limit enforced by the runtime before the write; string
      values only. Sandbox tests prove cross-plugin isolation and cap
      enforcement; the API is documented in `plugins/template/index.ts` and
      `SECURITY.md`.
- [x] **Incremental sync.** `fetchListings({ since, cursor })` returns only what changed,
      and the host merges it into the cached catalog. Full catalog pulls every cycle are
      what hit supplier rate limits and stretch runs toward the 12 h limit.
      → Done (N-7.4): the runtime always passes `{ since, cursor }` as the second
      `fetchListings` argument (`{ since: null, cursor: null }` on the first run);
      a plugin may return a `{ changes, deleted, cursor }` delta instead of the
      array, which the host merges transactionally (`merge_vendor_listings_cache`:
      changes upserted by `vendor_item_id`, deleted ids removed, untouched rows
      keep `fetched_at`). The host's `since`/cursor live in the new
      `vendor_sync_state` table (migration 027), cleared on uninstall; plugins
      that ignore the arguments keep full-pull replace semantics. Runtime tests
      cover an incremental second run changing one listing (plus cursor
      echo-back), and DB tests cover the merge and the sync-state round-trip.
      Documented in `plugins/template/index.ts` and `SECURITY.md`.
- [ ] **Host-managed OAuth for supplier APIs.** Metadata declares `oauth` (authorize URL,
      token URL, scopes). The host runs the flow in its own window and keeps the tokens in
      the keyring; the plugin receives only a current access token, never the refresh
      token.
- [ ] **Feed suppliers.** Many wholesalers publish CSV or XML feeds, over HTTPS, SFTP or
      email, rather than an API. Add host-side, size-bounded `Nisaba.parseCsv` and
      `Nisaba.parseXml`. Decide whether to support SFTP (an `sftp://` entry in
      `allowed_hosts`), a `file` config field the host reads with the user's consent, or
      both.
- [x] Per-plugin schedule (an interval, plus manual runs) and a **run history** in the UI:
      start and end time, listings emitted, errors, and the plugin's own log lines (which
      are already routed into the app log).
      → Done (N-7.5): migration 028 adds `plugin_run_history` and
      `vendor_plugin_installs.sync_interval_minutes`. Every run through the
      shared sync cycle (auto, manual Sync, direct Fetch) records start/end,
      status, listings emitted, the error on failure, and the plugin's
      `Nisaba.log` lines (captured per run by the runtime's log collector,
      which also returns them for failed runs). The marketplace installed
      card shows the history (`PluginRunHistory.vue`), a per-plugin
      interval picker plus Run now (`PluginSyncSchedule.vue`); the auto-sync
      path honours the interval via `plugin_sync_due` against the last
      recorded run, and manual runs are never gated. DB round-trip, due-ness
      and runtime log-capture tests cover the pieces.
- [ ] **Capabilities manifest.** Next to `allowed_hosts`, a plugin declares what it does:
      `catalog` today, and `orders`, `marketplace` and `notify` later. The install card
      shows the capabilities, the host refuses calls outside them, and a capability change
      triggers re-approval like a code change.
- [ ] **Author tooling.**
      - Publish the `Nisaba` API and the `VendorListing` schema as TypeScript types,
        generated from the Rust types so they cannot drift, in an npm package or as a
        `nisaba.d.ts` in the template.
      - A `nisaba-plugin` dev command that runs a plugin against a config, prints its
        batches, validates them against the schema, and packages the zip with its hash.
      - The fake-supplier harness from `tests/template.rs`, offered to authors as their own
        test kit.

### Plugin kinds after 1.0

- [ ] **Marketplace adapter plugins.** A TypeScript implementation of the `PlatformAdapter`
      contract (fetch listings, set quantity, set price, set description, create listing,
      detect sales), running in the same sandbox behind a Rust bridge that implements the
      trait. The engine then treats it exactly like a built-in adapter. Phase 2's lesson
      becomes a host rule: every write returns a structured result (`applied`, `rejected`,
      `rate_limited`, `auth_failed`), never a bare success. The host also enforces dry-run,
      so a plugin cannot write during a preview, and supplies idempotency keys and a
      rate-limit helper. WooCommerce is the first one, then Shopify (*Services and
      integrations*, WordPress priority).
- [ ] **Order (dropship) plugins.** When a sale of a vendor-sourced variant is detected,
      call the source plugin's `placeOrder` to submit it to the supplier. An approval queue
      is on by default, dry-run is supported, and order status is polled afterwards.
      Rothco is the first candidate. The order carries the buyer's name and address, so
      decide the retention and redaction rules first, and record them in the privacy
      statement.
- [ ] **Notification plugins.** Events go out to ntfy, email, Matrix, Slack or Discord: low
      stock, an oversell incident (Phase 5, Step 3), credentials about to expire, a sync
      failing, or a plugin needing re-approval.
- [ ] **Pricing rules** — decide whether these are built in or a plugin kind: markup over
      dealer price (Rothco's tiered pricing), minimum advertised price, and rounding. The
      lean is built in, because prices push to every platform and need the same
      write-safety as stock.

## Phase 4 — Sync engine depth

- [x] ~~Quantity deltas and conflict resolution (`resolve_deltas`)~~ — `resolve_deltas`
      is now the engine's resolution path: each cycle builds per-platform `QuantityDelta`s
      against the snapshots and sums them onto the canonical quantity.
- [x] Cache-first listing loads and live/cached merge
- [x] Per-sync-cycle vendor data refresh. It updates `products.quantity`, which the ledger
      now carries to the platforms on the next cycle (see *Stock correctness*).
- [x] `conflict.rs` exposes one strategy, which the engine now uses as its resolution
      path (folded into the ledger below), rather than a dead user-selectable strategy.

### Stock correctness

Found by the 2026-10-19 review and still present on PR #2. The engine derives new stock
**only from platform readings** (`sync_engine.rs` ~334–351). It takes the minimum across
platforms, or the maximum plus total decreases when any platform went up. `product.quantity`
is used only when no platform reports.

- [x] **Sales on two platforms in one cycle are undercounted.** eBay 10 → 8 and Squarespace
      10 → 9 now resolve to the summed deltas, 7: each platform's reading becomes a delta
      against its snapshot and `resolve_deltas` sums them onto canonical.
- [x] **Stock changed in Nisaba never reaches the platforms.** A restock typed into the app,
      or vendor stock added by `recalc_product_quantity`, now survives the cycle (deltas are
      computed against snapshots, not re-derived from readings) and is pushed to every
      platform whose reading differs from canonical.
- [x] **A failed push is never retried, and a newly mapped listing is never corrected.** The
      engine now pushes whenever a platform's *reading* differs from canonical, not only when
      canonical changed — so a failed push is retried on the next cycle and a new mapping is
      corrected to canonical. A platform with no reading this cycle is never pushed to.
- [x] **Variants are ignored.** Mappings carry `variant_id`, but the engine pushes the
      product's total to every mapping (~432, ~545) and takes the minimum across different
      variants' listings, so each variant listing advertises the whole product's stock.
      Fixed in N-4.3: the engine now groups mappings by (product, variant); canonical
      stock, platform readings/snapshots and pushes are all keyed per variant, with
      mappings that name no existing variant row falling back to product-level sync.
- [x] **Re-mapping an unmapped listing fails.** `delete_mapping` soft-deletes (`db.rs`
      ~1142), but the unique indexes `idx_mappings_product_platform_variant` and
      `idx_mappings_platform_item` include deleted rows, so `insert_mapping` hits UNIQUE.
      `create_listing_on_platform` creates the live listing *before* inserting the mapping
      (`commands/listings.rs` ~351–366), so this leaves an unmapped live listing. Use
      partial indexes (`WHERE deleted_at IS NULL`), and create the mapping in the same
      transaction as the record of the listing.
      Fixed in N-4.5: migration 023 rebuilds `platform_mappings` with both unique
      indexes partial (`WHERE deleted_at IS NULL`), preserving tombstones; listing
      creation pre-flights the mapping slot before creating anything remotely and
      records the mapping with the cached listing record in one transaction
      (`Db::record_created_listing`).
- [ ] **An XMR order is marked processed before its sale is applied** (`sync_engine.rs`
      ~245). Any `?` exit in between, or an untracked product (~272), loses the sale for
      good. Mark it in the same transaction as the stock write.
- [x] **Re-base the engine on a stock ledger.** Record every change as a uniquely keyed
      event: a sale by platform + order id, a restock, a manual adjustment, a vendor
      update. Canonical stock is the sum, and a push is the difference between canonical
      and each platform's reading, per variant. This fixes every item above. It is also the
      exact data model Sync v2 replicates (Phase 5, Step 2), so build it here first, on one
      machine, and replicate it later.
- [x] Pruning is never wired. `prune_old_sync_events` and `downsample_old_history` have no
      callers. `record_inventory_history` writes a row per tracked product every cycle even
      when nothing changed (~354–363), which is 288 rows per product per day at the default
      5-minute schedule. Also, `execute_write` (`db.rs` ~134) returns the row count of
      `COMMIT` rather than of the statement, and commits even when the statement failed.
      Fixed in N-4.7: the engine's `run_housekeeping` (startup + daily in
      `SyncEngine::run`) is now the caller — sync events are pruned after 90 days and
      history is downsampled to one row per product per day after 30 days.
      `record_inventory_history` inserts only when the quantity differs from the last
      recorded row, and the engine no longer calls it on unchanged cycles.
      `execute_write` runs the statement first, commits only on success (a failure
      rolls back), and returns the statement's affected-row count.
- [x] The vendor sync blocks the platform sync loop: the engine awaits the vendor run inline

      (`sync_engine.rs` ~736, ~750). Plugin limits now go up to 12 h, and Pause and Sync Now
      wait for the whole run. Run it as its own task.
- [x] `run_cycle` holds the adapters' read lock for the whole cycle, network retries
      included (~135–629), so saving config waits for the cycle to finish. Several
      commands (`listings.rs` ~66–76, `photos.rs`) hold it across fetches too. Clone the
      adapter handles out, then release the lock.
- [x] A dry-run mode that reports what a sync *would* change without writing to any platform
      — `SyncEngine::dry_run()`. It suppresses local writes too (snapshots, canonical
      quantity, history, sync events, the XMR order-dedup table and the vendor-sync
      callback), so a preview cannot make the following real cycle skip the change it
      previewed, and reports each planned change as `SyncEngineEvent::DryRunChange`.
- [x] Surface dry-run in the UI and the headless daemon — the engine supports it but nothing
      offers it to a user yet. A "preview this sync" button is the safest possible way to
      exercise the adapters against a real account. (N-11.6, 2026-10-23: the dashboard has a
      "Preview sync" button that runs a one-shot dry-run cycle via `SyncCommand::PreviewSync`
      and shows each planned `DryRunChange`; the headless-daemon half was retired by N-10.3
      when `crates/service` was deleted.)
- [x] Partial-failure semantics — one platform down must not abort the cycle or corrupt
      `PlatformSnapshot` state. Policy chosen: **never push to a platform that could not
      be read this cycle** — numeric pushes require a successful poll returning that
      listing's quantity, and stock-mode pushes require a successful poll plus a
      successful sale-detection pass. The unread platform is reconciled on the next
      cycle it can be read. `one_platform_down_does_not_abort_the_cycle` asserts the
      no-push behaviour, with stock-mode and per-listing successors alongside it; the
      policy is documented on `SyncEngine::run_cycle` and in README's Sync cycle
      section.
- [x] Surface a per-cycle reconciliation report in the UI, not just `SyncEvent` rows
      (N-11.6, 2026-10-23: the dashboard shows a "Last sync cycle" card built from the live
      event stream — products synced, changes pushed, per-platform change counts, platform
      errors and unmapped-listing detections.)
- [ ] Oversell protection: a configurable reserve buffer so concurrent sales across platforms
      cannot drive true stock negative. Moved to Phase 5, Sync v2 Step 3 (escrow), where a
      reserve is a share of stock rights held back from every peer.
- [ ] Chart `InventoryHistoryEntry` analytics. There are no charts today (`chart.js` and
      `vue-chartjs` are installed but never imported); `inventory.vue` shows a count.
- [x] **The sync schedule accepts anything and can kill the engine.** `config.vue` offers
      free-form cron. `parse_interval` (`tauri-app/src/main.rs` ~492) understands only a
      minute field of `*/N` and silently uses 300 s for anything else. `*/0` yields a 0 s
      period, and `tokio::time::interval` panics on zero, which ends the sync engine task.
      `Reload` never re-reads the interval (`sync_engine.rs` ~764), so a schedule change
      needs a restart. Replace the field with an interval picker, validate on save, reject
      zero, and apply changes on reload.

## Phase 5 — P2P and multi-company

- [x] Tor onion service, peer client/server and company sync payloads. (An earlier wording
      here said "encrypted" payloads: they carry no application-layer encryption, only
      Tor's transport encryption — see the sealing item below.)
- [x] Multi-company data model and company logos
- [x] **Peers never converged, and could overwrite each other's edits.** Merges are
      last-writer-wins on `updated_at`, but the merge wrote variants with `insert_variant`/
      `update_variant`, which stamp `now`, and then ran `recalc_product_quantity`, which
      re-stamped every variant and the product even when nothing changed. So each receiving
      peer's copy looked newer than the sender's, flowed back on the next sync, and the two
      rewrote each other every cycle, forever — and a stale copy carrying a fresh stamp could
      beat a genuine edit (an on-hand stock change) made on the other peer between payload
      load and merge. Fixed by `Db::upsert_variant_from_peer`, which keeps the edit's
      timestamp, and by `recalc_product_quantity` writing only rows whose quantity actually
      changes. `two_peers_converge_after_syncing_both_ways` reproduced it (every round
      rewrote the product and its variant) and now converges after the first sync.
- [x] Dropship stock did not survive P2P. A variant's effective quantity is on-hand plus
      vendor stock when its source plugin has "include vendor stock" on, but the variant's
      source-plugin link was not in the payload (and the merge wrote `None`), so a
      receiving peer could never attach vendor stock to it and counted on-hand only. The
      link now travels (`SyncableVariant::source_plugin_id`/`source_vendor_item_id`,
      defaulted so older peers still parse, and a peer that sends none does not erase a
      local link); each peer's own vendor sync supplies the vendor quantity, and peers
      holding the same plugin data agree and converge.
- [ ] Vendor stock is still per peer: a peer that has not installed and synced the source
      plugin counts that variant's on-hand stock only, and the peers disagree until it
      does. Decide whether to sync `product_vendor_data` itself, or require every peer that
      runs platform sync to have the company's vendor plugins installed.
- [ ] The sync payload carries platform auth tokens and plugin configuration protected only
      by Tor's transport encryption. Decide whether that is enough, or whether sensitive
      sections should be sealed with a key derived from the company secret as the at-rest
      company config already is. The iroh move below changes the question: QUIC to an
      endpoint id proven in the handshake is a stronger channel than "anyone who holds the
      secret", so revisit this after the transport lands rather than before.
- [ ] **`[company].enabled` is never read.** `CompanyConfig::enabled` is parsed, but
      `init_company_context` starts the Tor onion service and periodic P2P sync for every
      company unconditionally, so `enabled = false` (the default in
      `config.example.toml`) does not keep an install off Tor. Honouring it would stop P2P
      for anyone who relies on today's behaviour without setting the flag, so decide
      first: gate on it (and set it where it matters), or drop the setting. The README no
      longer claims it works.
- [x] Peer trust model — documented in the README's P2P section, from `server.rs` and
      `sync.rs`: the company secret is the only credential; a secret holder receives
      everything (tokens included) and anything it sends is merged. The items below are
      what that reading found.
- [ ] Authenticate peers individually. `/api/sync` authorizes by the `sender_peer_id` /
      `sender_onion` in the request body — self-asserted, so any secret holder can speak as
      any authorized peer, and a sync under another peer's id rewrites that peer's onion
      address to the sender's (`update_peer_onion_address`). `/api/address-update` repoints
      any peer with no check that the caller is that peer. Needs a per-peer credential:
      Tor onion-service client authorization, or requests signed with a per-peer key.
      **Resolved by the iroh transport below** — each peer's Ed25519 endpoint id is proven
      in the QUIC handshake, so authorization keys off the connection, not the body.
- [ ] Enforce roles on the caller. `check_admin` checks the receiving node's own role, so on
      an admin's node every secret holder can add, approve and remove peers and change
      roles. Depends on the item above — a caller's role means nothing until the caller is
      authenticated.
- [x] Bound peer-supplied timestamps. Last-writer-wins compared the sender's `updated_at`
      verbatim, so a row stamped `9999-12-31` — or anything sorting after a digit, like
      `"Z"` — won every later merge and could never be corrected. Every merged section now
      refuses a row whose timestamp does not parse (SQLite or RFC 3339) or is more than 10
      minutes ahead of the receiver's clock, and counts it in
      `MergeSummary::rejected_timestamps`.
- [ ] Consider a hybrid/logical clock for merges instead of wall time: even bounded,
      last-writer-wins on wall clocks lets a peer with a fast clock win ties it should lose.
      Planned as part of Sync v2, Step 2 (per-field registers on a hybrid logical clock).
- [x] Compare the company secret in constant time in `auth_middleware` (was `==`).
- [x] Auth tokens ping-ponged like variants did: `upsert_auth_token` stamped `now` on
      receipt, so a received token flowed straight back re-stamped, and a stale copy could
      overwrite a token the other peer had just refreshed — an auth failure on the next
      sync. Merges now use `upsert_auth_token_from_peer`, which keeps the edit's timestamp.
- [ ] Key rotation for the company secret, with a migration path for existing peers.
      `/api/rotate-secret` exists but is a stub that only acknowledges the request. Under
      iroh this becomes per-peer revocation (drop an endpoint id from the allow list), and
      the shared secret only seals company config at rest — see below.
- [x] Conflict resolution for P2P merges was `merge_remote_payload`'s implicit policy. It is
      now written down per section in the README and pinned by `tests/round_trip.rs`
      (newer-wins both ways, tombstones, idempotence, two-way convergence) — which is how
      the convergence bug above was found.
- [ ] Offline/rejoin behaviour after a peer has been away for longer than the retention window
- [ ] Bounded payload sizes — a full sync payload currently grows with the whole catalog.
      Answered by Sync v2's op log and compaction (Step 4 below), which also covers the
      offline/rejoin item above.

### Today's P2P (Tor) — what the 2026-10-19 review found

On the PR #2 branch, P2P company sync is **not functional end to end**: the join flow
cannot produce a syncing pair, and platform-sync coordination is never invoked. Sync v2
replaces this transport. Fix the data-loss and secret-loss items now, have the README say
P2P is experimental, and do not repair the Tor join path.

- [x] **Leaving a legacy-migrated company deletes the app database.** At startup the legacy
      company is registered with `config.database.path` (`main.rs` ~150–151), which is the
      app registry database. Leaving the company then runs `remove_file` on that path
      (`commands/company.rs` ~327–330). Fixed in PR #5.
- [x] **The legacy company secret can be lost.** Startup replaces the stored secret with
      its hash even when saving it to the keyring failed (`main.rs` ~143–148). Fixed in PR #5.
- [ ] **Join cannot produce a syncing pair.** `join_company` appends `.onion` to a name that
      onyums already suffixes (`company.rs` ~247), so the admin stores `x.onion.onion`. The
      member stores the admin only in `company_peers_v2`, under a random peer id, while
      `sync_all_peers` reads the legacy `company_peers` table (`p2p/src/lib.rs` ~126). The
      member therefore never starts a sync, and `handle_sync`'s lookups miss for the admin.
- [ ] **Platform-sync round-robin is dead code.** `is_our_turn_to_sync` and
      `record_platform_sync_and_push` have no callers. Every peer pushes to the marketplaces
      on its own timer, with credentials shared over P2P, and XMR order dedup converges only
      at the P2P interval, so a sale can be processed twice. Sync v2 needs a single writer
      per platform regardless (Step 2).
- [ ] **One onion identity for all companies, tied to the launch directory.** Every company
      calls `onyums::serve(router, "nisaba")`. onyums keeps one global `ONION_NAME` and
      state under `./tor/onyums/state` (artiqwest under `./tor/arti`), so a second company
      saves the first company's address as its own. A packaged build launched from `/` or
      Program Files may be unable to write the state, and a different launch directory
      means a new address.
- [ ] Hard 2 MiB request limit and no client timeout. `handle_sync` takes
      `Json<SyncRequest>` under axum's default 2 MiB body limit, so once a catalog's
      payload outgrows it, every sync fails with 413. artiqwest sets no timeout, so one
      hung peer stalls the sequential peer loop.
- [x] The merge is not transactional. Each row was applied with `?`, so the first error
      left earlier sections applied and skipped the rest (config, tokens, plugins,
      variants), and the same payload failed the same way every cycle.
      `merge_remote_payload` now plans every write from the local state first, then
      applies the batch in a single transaction (`Db::execute_write_batch`): any
      error rolls the whole merge back and leaves the database unchanged. Pinned by
      `a_failing_merge_leaves_the_database_unchanged` in `tests/round_trip.rs`.
- [ ] No onion-service lifecycle. `start_onion_service` returns before Tor has
      bootstrapped, with no retry. `Stop` ends only the sync loop, so a company that was
      left keeps serving its database. Companies created or joined at runtime drop sync
      and P2P events and get no vendor-sync hook until restart.
- [x] Product and variant deletes orphaned rows in `listing_descriptions`,
      `listing_photos`, `pricing_snapshots` and `cached_listings`. `delete_product`
      now removes those tables too (with the dependents it already removed) in its
      transaction, and `delete_variant`/`delete_variants_for_product` also remove
      `product_vendor_data`, so a delete leaves no rows pointing at a product or
      variant that no longer exists.
- [ ] Product and variant deletes are hard deletes (`db.rs` `delete_product`,
      `delete_variant`), so a merge re-inserts them from any peer that still has
      them. Sync v2's observed-remove sets cover this half.
- [x] Peer admin commands (approve, reject, set role, remove) return `Ok` when the peer does
      not exist and have no admin check, unlike `get_company_secret` (`company.rs` ~438+).
- [ ] README still says "AES-GCM encrypted payloads" (`README.md:27`); payloads have no
      application-layer encryption.

### Sync v2: CRDT replication over iroh

Owner direction 2026-10-19: replace today's sync with a CRDT replication layer carried over
[iroh](https://github.com/n0-computer/iroh). Today's sync sends the full payload every time,
merges whole rows by last-writer-wins on wall-clock `updated_at`, and travels over a Tor
onion service.

**Why the current model has to go, from the code.** `merge_remote_payload` (`sync.rs`)
replaces a whole product or variant row whenever the remote `updated_at` is newer.
Two edits made concurrently on different peers therefore lose one of them, even when they
touch different fields: a rename on one peer and a threshold change on another, or two
on-hand stock changes to the same variant (each peer moves 10 → 9; the merged result is 9,
not 8). The other open items above follow from the same design: wall-clock ties, payloads
that grow with the catalog, and rejoin after a long absence.

**What iroh brings.** iroh 1.0 shipped 2026-06-15 (1.3.0 on 2026-09-28). Each install gets
an Ed25519 *endpoint id*, and QUIC connections are authenticated against that key. iroh
connects directly through NAT and falls back to a relay when it cannot. Two endpoints can
also run on loopback with relays off, which makes the transport testable in CI. Today's
transport is `onyums` (an axum router served as an onion service) plus `artiqwest` (HTTP
over Tor), and only `crates/p2p/src/{lib,server,client}.rs` use them.

**What the move retires:** the self-asserted `sender_peer_id`/`sender_onion` (above),
`/api/address-update` and `update_peer_onion_address` (endpoint ids are stable), the 120 s
onion-publication poll, the on-hold `onyums 0.5` item (Phase 0), and the arti dependency
tree that carries the `proc-macro-error2` warning (Cross-cutting).

#### Step 0: decisions before code

- [ ] **Privacy model.** Tor hides each peer's IP address from the other peers and from
      network observers; iroh does not. Direct connections show IPs to peers. Relays and
      discovery see endpoint ids, IPs and connection timing, though not content, which is
      end-to-end encrypted. If you keep the defaults, those relays and discovery are n0's
      public infrastructure. For a tool whose users sell on XMR Bazaar, that is a real
      downgrade. Options:
      (a) iroh only, with a self-hosted `iroh-relay` and peers found through invite tickets
      instead of public discovery;
      (b) iroh by default, with Tor kept as a per-company opt-in transport (keeps
      `onyums`/`artiqwest` and their maintenance);
      (c) iroh on n0's public relays.
      Record the choice in the README's P2P section and in `SECURITY.md`.
- [ ] **CRDT design doc** (`docs/sync-v2.md`) covering every synced section of
      `SyncPayload`: which CRDT each field uses, how stock works, and what stays outside
      the CRDT (below). Code starts after it is agreed.
- [ ] **Engine spike.** Build one product with variants, mappings and stock events in each
      candidate. Judge them on: delta export since a version vector; compaction (garbage
      collection of history); schema evolution; queryability from the Tauri commands, which
      all read SQLite today; and memory at catalog scale.
      - Hand-rolled per-table CRDTs over the existing turso/SQLite schema, the way cr-sqlite
        does it: per-column clocks, an op log and version vectors. SQLite stays the source
        of truth and the change is confined to write paths. cr-sqlite itself is a C
        loadable extension for SQLite and will not load into turso.
      - [Loro](https://loro.dev) 1.16: Rust-native; Map, List, MovableList, Tree and Counter
        containers; version vectors; export of updates since a version; shallow snapshots
        for garbage collection. It makes the CRDT document the source of truth, with SQLite
        as a projection of it — an inversion of today's architecture.
      - Automerge 0.12: Rust core, JSON document model, a mature sync protocol, and the same
        inversion as Loro.
      Lean: per-table over SQLite, unless the spike shows a document engine's projection
      is cheap enough to justify the inversion.

#### Step 1: transport — today's protocol over iroh

Do this first. It gives per-peer authentication on its own, independent of the CRDT design.

- [ ] Persist each install's iroh secret key in the OS keyring, next to the company secret.
      Losing it means a new identity, so export/import and moving to a new machine must
      carry it, or document that the peer re-joins.
- [ ] Use a `nisaba/sync/1` ALPN with an `iroh::protocol::Router` handler in place of the
      axum router: one bidirectional stream per request, with length-prefixed JSON for
      `SyncRequest`, join, ping, company info and peer administration. Leave `sync.rs`
      untouched so `tests/round_trip.rs` keeps pinning the merge while only the wire changes.
- [ ] Authorize by the connection's remote endpoint id. Add an `endpoint_id` column to the
      peers table (a migration) and drop `sender_peer_id`/`sender_onion` from the request
      types.
- [ ] Join by ticket. The admin issues an invite with its endpoint address and a one-time
      join token, shared by copy-paste or QR; approval binds the joiner's endpoint id. The
      company secret then never crosses the wire and stops being a bearer credential.
- [ ] Enforce roles on the caller's endpoint id. This replaces `check_admin`'s check of the
      node's own role.
- [ ] Revocation instead of rotation. Removing a peer drops its endpoint id and takes effect
      on its next connection; retire the `/api/rotate-secret` stub.
- [ ] Elect the designated peer (`is_our_turn_to_sync`) on endpoint ids instead of onion
      addresses.
- [ ] Resolve `[company].enabled` (above) as "keep this install off P2P" rather than "off
      Tor".
- [ ] Transport tests on loopback endpoints with relays off: join by ticket, sync both
      ways, and refuse an unknown endpoint id, a revoked peer, and a non-admin caller on
      admin routes. These replace "the Tor transport is untested" in Phase 1.

#### Step 2: the CRDT data layer

- [ ] Scalar fields — name, SKU, threshold, image, sort order, attributes, company config,
      logo — become **per-field last-writer-wins registers** stamped with a **hybrid
      logical clock** plus the endpoint id as tie-breaker. This replaces whole-row,
      wall-clock comparison and closes the hybrid-clock item above. Keep a bound on how far
      ahead a peer's physical clock may be, as `plausible_timestamp` does today.
- [ ] Collections — a product's variants, platform mappings, vendor plugins and the peer
      list — become **add-wins observed-remove sets** keyed by stable UUIDs. A delete that
      races an edit then resolves the same way on every peer, and tombstones can be
      compacted once every peer has seen them.
- [ ] **Stock is not a register.** Model it as a grow-only set of uniquely identified stock
      events, with quantity = the sum of the events:
      - a sale, keyed by platform + order id, so two peers that detect the same sale count
        it once (this generalizes today's `xmr_processed_orders` dedup table);
      - a restock;
      - a manual adjustment, keyed by UUID.
      A manual "set on-hand to N" is recorded as an adjustment computed against the
      writer's current view. Two concurrent sets therefore both apply, so surface the pair
      for review instead of summing them silently.
      This is the same ledger Phase 4 builds on a single machine (*Stock correctness*);
      Sync v2 replicates it rather than inventing a second model.
- [ ] **Marketplace writes stay outside the CRDT.** Replicas converge on local state, not
      on what eBay or Squarespace hold. Pushes stay with the designated peer and must be
      idempotent. `platform_snapshots` and the version tags become per-peer local state.
      If peers need to see them, replicate them as observations, never as commands.
- [ ] **Secrets stay out of the replicated log in plaintext.** That covers auth tokens,
      plugin `secret: true` fields and the iroh key. Seal them with a company key, or
      distribute them out of band. This closes the sealing item above and the
      plugin-secrets item in Phase 3.
- [ ] Sign every op with its author's endpoint key, and validate it before applying: the
      signature, the author's role at that point, the schema, and value bounds. Because ops
      are attributed, revoking a compromised peer can also undo its ops since a chosen
      point.

#### Step 3: oversell protection — escrow over the stock CRDT

Convergence and correctness are different guarantees. The stock CRDT ensures every peer
ends up agreeing on the same quantity. It cannot keep that quantity at or above zero:
two peers that each sell the last unit while partitioned will both apply the sale, and
the merged stock is −1. Holding a numeric invariant without coordinating every write is
what **escrow**, or a **bounded counter**, does (the Bounded Counter CRDT, Balegas et al.,
SRDS 2015). Each peer holds a share of the stock as *rights*. A peer spends only its own
rights, so no interleaving of sales can drive the total below zero, and coordination is
needed only when a peer's rights run out. Today's sync engine never goes negative only
because platform sync runs on a single designated peer. Escrow keeps that guarantee once
any peer can record a sale.

- [ ] Rights state per variant: a matrix of `rights[from][to]` transfers plus each peer's
      spent count. Both only ever grow, so the matrix is itself a CRDT. A peer's available
      rights are what it was given, minus what it gave away, minus what it spent. Stock
      remains the sum of stock events (Step 2); rights only limit who may *decrement* it.
- [ ] Gate every decrement on available rights: a detected sale, a manual reduction, a
      reservation. A restock adds rights to the peer that records it. A sale detected on a
      marketplace has already happened, so it is recorded regardless. If it overdraws the
      detecting peer's rights, it is an **oversell incident**: logged, surfaced in the UI,
      and triggering the zero-stock push to the other platforms immediately rather than
      at the next cycle.
- [ ] Allocation policy for each peer's share. The starting point is to give the
      designated peer, which runs platform sync and so sees every marketplace sale, all
      the rights, and other peers request what they need for manual edits. A later step
      can give each platform's poller a share proportional to that platform's recent sales
      velocity.
- [ ] Rights transfer: a peer short of rights asks peers with spare rights over the iroh
      connection. A transfer is just another op, so it replicates and stays idempotent.
      Define what the UI shows while a request is outstanding, and the timeout after
      which an edit is refused as "not enough stock rights on this machine, others
      offline".
- [ ] Rebalancing: periodically move rights toward the peers that use them, and pull
      rights back from a peer that has been offline longer than a threshold. Rights held
      by an offline peer are stock that nobody else can sell.
- [ ] Revocation and lost keys: when a peer is revoked or its iroh key is lost, its unspent
      rights must return to the pool. Only an admin can reclaim them, recorded as a signed
      op. Without this, revoking a peer quietly shrinks sellable stock.
- [ ] Fold Phase 4's "oversell protection: a configurable reserve buffer" into this: a
      reserve is a share of rights held back from every peer and released by an admin.
- [ ] Tests: property tests in which concurrent decrements across N partitioned peers never
      take stock below zero under any delivery order; rights stay conserved, meaning the
      sum of available rights plus the sum of spent equals the sum of stock events; and an
      overdraw from a marketplace sale is reported as an incident and never absorbed
      silently.

#### Step 4: replication protocol

- [ ] A per-author op log with sequence numbers. A sync exchanges version vectors and sends
      only the ops the other side lacks, so payloads are bounded by the difference instead
      of the catalog. For state-based reconciliation, range-based set reconciliation
      (`negentropy` 0.5, the approach `iroh-docs` uses) is the reference design.
- [ ] Compaction. Ops older than the causal-stability frontier, meaning seen by every
      active peer, fold into a snapshot. A peer away longer than retention receives the
      snapshot plus newer ops. This closes the offline/rejoin and bounded-payload items
      above.
- [ ] Push on change: `iroh-gossip` announces new ops to online peers, who then pull, and a
      periodic anti-entropy sync covers missed announcements.
- [ ] Schema evolution: version the op format, have peers advertise the versions they
      support, and make an unknown field round-trip instead of dropping it.

#### Step 5: verification and migration

- [ ] Property-based convergence tests with `proptest`: N replicas, random ops, and random
      delivery order, duplication, loss and redelivery. Every replica must end in the same
      state, stock must equal the sum of its events, and no delete may resurface. Add a
      deterministic simulation of partitions and rejoins. Port the scenarios in
      `tests/round_trip.rs`.
- [ ] Migration. One peer turns the existing database into a genesis snapshot under
      protocol v2; the other peers re-join from it by ticket. Once no supported release
      speaks v1, remove the full-payload last-writer-wins path (`load_full_sync_payload`
      and `merge_remote_payload`). Unless option (b) was chosen, also remove `onyums`,
      `artiqwest` and the Tor startup path, re-run `cargo report
      future-incompatibilities`, and close the `proc-macro-error2` item.

## Phase 6 — Release and distribution

- [x] Signing key for the Tauri updater — `tauri.conf.json` ships an empty `pubkey`, so
      auto-update cannot work at all until this exists. Concretely: `tauri signer generate`
      produces the pair, the *contents* of the public key (not a path) go in `pubkey`, and
      the release workflow needs `TAURI_SIGNING_PRIVATE_KEY` (plus
      `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` if set) exported as real environment variables.
      https://v2.tauri.app/plugin/updater/
- [x] Pin `createUpdaterArtifacts` to the v2 format rather than `"v1Compatible"` when the
      release workflow lands — Tauri documents the setting as removed in v3.
      https://v2.tauri.app/plugin/updater/
- [ ] Track the Tauri 3 alphas before they land on a stable line: `v3.0.0-alpha.2`
      (2026-09-21) renames plugin APIs (`js_init_script` → `initialization_script`,
      `Plugin::extend_api` → `Plugin::run_invoke_handler`) and removes
      `Invoke::state`/`state_ref`. Nisaba stays on Tauri 2 until 3 is stable; this is the
      migration checklist's first entry. https://github.com/tauri-apps/tauri/releases
- [x] `.updater/latest.json` published from a real release workflow. The Tauri CLI does not
      write `latest.json`, so the workflow must assemble it from the `.sig` files.
      Alternatively, point the endpoint at a release asset instead of a file on `master`.
      Today the endpoint (`tauri.conf.json` ~47–49) is a raw file on `master`, so any commit
      to that file is an update push.
- [x] Correction to the item above about pinning `createUpdaterArtifacts`: the key is not in
      `tauri.conf.json` at all, so nothing produces `.sig` or `.app.tar.gz` files, and the
      globs in `release.yml` for them can never match. Add the key (v2 format) together
      with the signing key.
- [x] The updater checks a GitHub raw URL at launch and every 6 hours (`useUpdater.ts`
      ~61–65), outside Tor and with no opt-out. Add a setting, and record the choice in the
      Sync v2 privacy decision.
- [x] CI runners: every run carries Node 20 deprecation annotations for the actions, and
      `ubuntu-latest` moves to Ubuntu 26 from 2026-10-19. Bump the action versions, and
      check that the apt list (`libappindicator3-dev`, the WebKitGTK packages) still
      resolves on the new image, or pin `ubuntu-24.04`.
- [x] CI has no macOS leg, although every release ships a universal macOS bundle, and
      `release.yml` runs no tests before publishing. Add `macos-latest` to `ci.yml`, or a
      test job that `publish` depends on.
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
- [x] Decide whether `crates/service` (headless sync daemon) and `crates/tui` ship as real
      products; they were tracked in git but excluded from `[workspace] members`, so they
      were not built, not tested and not covered by CI. **Decided 2026-10-22 (N-10.3):
      deleted** — restoring them was close to a rewrite (neither had changed since
      `1f3cdc7`; both read platform credentials from `config.toml` and use the app
      database where the app now uses per-company encrypted config; neither supports
      Amazon, plugins or P2P). A headless daemon can return post-1.0 designed against
      the current config model. `crates/service` and `crates/tui` were removed from the
      repo, including all stale references. (This also retires Phase 4's "dry run in the
      headless daemon" as a 1.0 item; dry-run remains engine-supported for the UI.)
- [x] Version consistency — `Cargo.toml`, `tauri.conf.json` and any updater manifest must be
      bumped together; add a check that fails CI when they disagree (N-10.2, 2026-10-21:
      `scripts/check-version-consistency.py` runs as its own `version-consistency` CI job,
      comparing `Cargo.toml [workspace.package]`, `crates/tauri-app/tauri.conf.json` and
      `frontend/package.json`; the missing `version` field was added to `frontend/package.json`)

## Phase 7 — Frontend and UX

- [x] Nuxt 3 SPA with Tailwind and shadcn-vue-style components (`components/ui/`, on
      `radix-vue`), covering products, listings, vendors, inventory and companies. The
      `shadcn-nuxt` module is a dependency but is not registered in `nuxt.config.ts`.
- [x] Multi-tier pricing display, dealer price, UPC and weight on vendor cards
- [x] Empty and error states — the pages assume data exists. Named cases:
      - Creating a product (`products/index.vue` ~254–264) has no validation and no
        `catch`; a duplicate SKU hits the unique index and the user sees nothing.
      - The dashboard's `Promise.all` (`pages/index.vue` ~14–22) has no `catch`.
      - "Sync now" (`StatusBar.vue` ~40) gives no progress or error feedback.
      - `inventory.vue` shows a load error as "No analytics data yet".
      [N-11.3: create-product dialog validates (name/SKU required) and shows
      backend errors inline incl. duplicate SKU; dashboard wraps the `Promise.all`
      in try/catch with an error banner + retry; Sync Now shows a spinner and
      notifies on failure; inventory distinguishes load errors (with retry)
      from the genuine empty state.]
- [x] Confirm destructive actions, using one mechanism. Unlinking a mapping, removing a peer
      and uninstalling a plugin act immediately. Import (`products/index.vue` ~309) does
      `INSERT OR REPLACE` over live rows with no preview. The confirms that do exist use
      three different mechanisms: `window.confirm`, the dialog plugin, and an in-app modal.
      [N-11.3: one shared `<ConfirmDialog>` component (+ `useConfirmDialog`) now
      covers unlinking a mapping, removing a peer, uninstalling/removing a plugin,
      deleting a product/variant, leaving a company and restoring a backup;
      import is preview-then-apply (`preview_import_data`/`apply_import_data`
      commands) showing new/replaced/skipped counts and the affected SKUs before
      anything is written.]
- [x] Sync status goes stale: each `useSync()` call creates its own `isRunning`, starting
      at `true` (`useSync.ts:7`). Nothing refetches it on a company switch, so the dashboard
      and sidebar disagree after a pause. Make it one shared store, updated from engine
      events.
- [x] N+1 IPC calls. The dashboard, listings and products pages call `get_mappings`, and
      sometimes `list_variants`, once per product, sequentially, on every change. Add batch
      commands.
- [x] Type drift between TypeScript and Rust:
      - `SyncEngineEvent` lacks `DryRunChange`;
      - `MergeSummary` lacks `vendor_plugins_updated`, `variants_updated` and
        `rejected_timestamps`;
      - `ProductVariant` lacks `source_plugin_id` and `source_vendor_item_id`.
      `company.vue` also ignores the merge summary, so rejected peer rows are never shown.
      Generate the types from Rust (`ts-rs` or `specta`) instead of maintaining them by hand.
- [x] Dead code:
      - `usePricing.ts` and `useTauriCommand.ts` are never imported, so `update_price` and
        `get_pricing_history` cannot be reached from the UI.
      - Five registered commands have no caller: `get_cached_images`, `get_ebay_auth_url`,
        `get_product_cache_freshness`, `list_company_peers`, `update_variant_quantity`.
      - Several npm dependencies are never imported: `chart.js`, `vue-chartjs`, `diff`,
        `lucide-vue-next`, `class-variance-authority`, `clsx`, `tailwind-merge`,
        `@tauri-apps/plugin-shell`.
      Wire up or remove each one.
- [ ] The publish flow (`products/[id]/publish.vue`) should show, per platform, exactly which
      fields will be written and which are unsupported, driven by `PlatformCapabilities`
- [ ] The per-platform sales breakdown is always empty. `analytics.rs` (~76–91) groups by
      `source_platform`, but every sync event is written with `"sync_engine"` there
      (`sync_engine.rs` ~465, ~497, ~565, ~597), and `from_str_loose` drops those rows, so
      `inventory.vue` always shows "—".
- [ ] Startup failures are silent. `main.rs` (~72, ~84–86) calls `expect()` on loading the
      config and opening the database, and a Windows release build has no console, so the
      app just exits. Show a native error dialog and write a log file.
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
      pages, and the empty states above. (Whether it can run in CI is settled: c99c566
      added the job.)
- [x] **Fix the CI UI smoke job, which fails on PR #2** (run 36223531255).
      `scripts/ui-smoke.py` builds the app path from a relative `--target-dir target`
      without `abspath`. Its existence check passes from the repo root, but tauri-driver is
      then started with `cwd` set to the temp fixture dir, so WebKitWebDriver cannot find
      `target/debug/nisaba-tauri`. Locally it passes because the README passes an absolute
      path. Fix: `os.path.abspath(args.target_dir)`.

## Services and integrations

Two kinds of "service" matter here: hosted infrastructure that Nisaba itself would have to
run, and external platforms that Nisaba syncs with. Nisaba is a desktop app that never
holds a customer's keys anywhere but their own machine, so the bar for the first kind is
high: every hosted service is an availability dependency, a privacy promise and a running
cost.

### Hosted infrastructure

- [x] **No OAuth broker.** Customers bring their own API keys (see *1.0 release*, owner
      decision 2026-10-19, after Nisaba's own Amazon SP-API application was denied).
- [ ] Post-1.0, revisit a **shared eBay keyset** only. eBay is the platform where
      bring-your-own costs the most: developer program signup, a production keyset, a
      RuName, and the account-deletion exemption. Sharing a keyset would need a small
      stateless broker that does the code-for-token exchange and every refresh with
      Nisaba's client secret, plus an account-deletion endpoint. Weigh that against the
      setup wizard's real-world drop-off before building it. Amazon stays bring-your-own
      unless a new Nisaba application is approved.
- [ ] **iroh relay** — only if Sync v2 picks option (a) in Step 0. Host one relay for
      customers who want it, and document running your own; `iroh-relay` is a single
      binary.
- [ ] **Update manifest** — GitHub Releases, with no new service (Phase 6). Checks can be
      turned off (Phase 6).
- [ ] **Plugin registry** — no hosted registry for 1.0. Plugins are imported from a file
      or URL, and shared within a company. Revisit with plugin signing (Phase 3).
- [x] **No telemetry or crash-reporting service.** A local *Export diagnostics* bundle
      that the user chooses to attach to a bug report covers the need without data
      leaving the machine unasked (*1.0 release*, Gate: product).

### Marketplace and storefront integrations

Shipping today: eBay, Squarespace, XMR Bazaar, and Amazon (Beta for 1.0). Every candidate
below uses keys or tokens the customer creates for their own account, so none needs a
broker. Which ones are worth building depends on two facts this list does not have: where
customers' sales actually come from, and whether they sell in person. Ask before building
past the *Should* group.

#### Priority: WordPress and the WooCommerce ecosystem

Owner direction 2026-10-19: WordPress and its ecosystem come first. WooCommerce is the
next adapter, ahead of Shopify. It has four advantages over the rest of the list:

- it is self-hosted, so customers own their store and their data, which matches Nisaba's
  stance;
- it is common with sellers who take crypto, which matches the XMR Bazaar audience;
- it is the only candidate that CI can verify against a real store, using the official
  WordPress and WooCommerce Docker images;
- its official channel extensions can carry stock onward to Google, Meta and TikTok.

- [ ] **WooCommerce adapter** (as a marketplace plugin if the Phase 3 bridge is ready,
      otherwise as a Rust crate that later moves behind it):
      - **Auth.** REST API v3 with a consumer key and secret that the customer generates in
        WooCommerce → Settings → Advanced → REST API (Read/Write), over HTTPS only.
        Bring-your-own keys, no app review.
      - **Stock.** Simple products and every variation carry `manage_stock`,
        `stock_quantity`, `stock_status` and `backorders`; map variations onto Nisaba
        variants by SKU. Push through the `products/batch` and `variations/batch`
        endpoints rather than one request per item.
      - **Sales.** Poll orders with `modified_after` on each cycle; a desktop app cannot
        receive WooCommerce webhooks. Key each sale by store and order id so it lands in
        the stock ledger once (Phase 4). Decide which order statuses count as a sale, and
        how WooCommerce's held stock for pending orders is treated.
      - **Price and description.** `regular_price` and `sale_price` are strings, which
        suits the decimal-money item; descriptions are HTML and go through the same
        sanitiser as every other `v-html` sink.
      - **Create listing**, including variable products with attributes and images by URL.
- [ ] **Live verification in CI.** Start `wordpress` with WooCommerce in a service
      container, seed products and variations, and run the adapter's full write path
      against it, including a rejected write. No other platform allows a real-API test
      in CI, so this adapter becomes the template for "writes report what really
      happened" (Phase 2).
- [ ] **Blind stock writes race checkout.** A REST stock update is an absolute set, so an
      order placed between Nisaba's read and write is overwritten, losing that decrement.
      Every platform has this race; WooCommerce is the one where Nisaba can ship
      server-side code to close it. If polling cannot keep the window small enough,
      build the companion plugin below.
- [ ] **Companion WordPress plugin "Nisaba Connector"** (optional; build only if the race
      above or the REST API's limits demand it). It would provide an atomic adjust
      endpoint ("decrement by N") or a compare-and-set ("set to X if current is Y"), a
      bulk stock read keyed by SKU, and an order-change cursor. Authentication would use
      WordPress Application Passwords or the WooCommerce keys. Publish it on
      wordpress.org so customers install it from their dashboard.
- [ ] **Channel extensions instead of direct adapters.** Google's, Meta's and TikTok's
      official WooCommerce extensions sync the store's catalog and availability to
      Merchant Center, Meta catalogs and TikTok Shop. For a WooCommerce seller, a correct
      WooCommerce stock count therefore reaches all three without a Nisaba adapter.
      Document the recommended setup, and verify that each extension pushes stock changes
      promptly and does not wait for a daily feed. Direct Merchant Center, Meta and TikTok
      adapters (below) are then for sellers without WooCommerce.
- [ ] **Crypto checkout guide.** BTCPay Server for WooCommerce, and the Monero WooCommerce
      gateway, are the recommended way for XMR Bazaar sellers to take crypto on their own
      store. No integration is needed, because Nisaba sees WooCommerce orders whatever the
      payment method; this is a setup guide.
- [ ] **WordPress.com-hosted WooCommerce.** Confirm that the REST API and its keys work on
      WordPress.com's commerce plans, so customers who don't want to self-host are covered.
- [ ] **Move a catalog into WooCommerce.** Use Nisaba's cross-platform create-listing to
      copy a Squarespace or eBay catalog into a new WooCommerce store, with mappings
      created as it goes. This turns "set up a WordPress store" from a migration project
      into a button.
- [ ] *Could:* WordPress multi-vendor marketplaces such as Dokan, WCFM and WC Vendors, for
      sellers who list on someone else's WordPress-based marketplace. Dokan exposes a
      REST API for vendors. Build one when a customer sells on one.
- *Not building:* Easy Digital Downloads and other WordPress digital-goods plugins. They
  sell downloads, so there is no physical stock to sync.

**Should** — after WordPress, in recommended order:

- [ ] **Shopify** — the largest small-business storefront. The GraphQL Admin API has
      first-class inventory levels per location. A custom app created in the store's admin
      gives an access token with no app review.
- [ ] **Google Merchant Center** — a direct adapter for sellers whose store is *not*
      WooCommerce (Squarespace today); WooCommerce sellers are covered by Google's
      extension (above). Merchant Center disapproves items whose availability does not
      match the store page, so accurate stock is exactly what it needs. Build on the new
      Merchant API, not the Content API for Shopping it replaces; check the Content API's
      sunset date first.
- [ ] **Square POS — if customers sell in person** (shows, a shop). A sale at a table is an
      oversell Nisaba cannot see today. Square issues a personal access token for the
      seller's own account, which fits bring-your-own keys. Square is unrelated to
      Squarespace.
- [ ] **Walmart Marketplace** — a large marketplace, but sellers must be approved by
      Walmart first. Its API keys are per seller.
- [ ] Build these as **marketplace plugins** rather than new Rust crates (Phase 3,
      *Plugin kinds after 1.0*). A new marketplace then ships without an app release.
      WooCommerce, followed by Shopify, proves that the plugin bridge is general. This
      supersedes the "fifth adapter" item in Phase 2.

**Could** — build when a customer actually sells there:

- [ ] **BigCommerce** — a storefront like Shopify, with store-level API tokens. It also
      has a WordPress plugin (BigCommerce for WordPress), so it overlaps with the WordPress
      priority above.
- [ ] **Meta (Facebook/Instagram) catalog** — for sellers without WooCommerce, since
      WooCommerce sellers are covered by Meta's extension. Catalog and availability sync
      only: Meta moved US Shops checkout to sellers' own websites, so this channel sends
      traffic rather than taking sales. Check the current state of Shops checkout before
      building.
- [ ] **TikTok Shop** — for sellers without WooCommerce, since WooCommerce sellers are
      covered by TikTok's extension. A growing channel, but it needs partner app approval,
      the same risk that sank Nisaba's own Amazon application (*1.0 release*).
- [ ] **BTCPay Server store and point-of-sale apps** — a crypto-native storefront that
      tracks item inventory; the audience matches XMR Bazaar's. Confirm that the Greenfield
      API exposes item inventory for read and write.
- [ ] **GunBroker** — only if customers' catalogs lean toward tactical accessories. It has
      an API.
- [ ] Other crypto-native marketplaces besides XMR Bazaar — ask current customers which
      ones they sell on.

**Not building, and why:**

- **Mercari and Poshmark** — neither offers a public API for sellers, and automating
  their apps would break their terms.
- **Etsy** — its seller policy is built around handmade, vintage and craft supplies, so
  reselling a wholesale catalog such as Rothco's is a policy problem, not a technical one.
  Revisit only for a customer whose catalog qualifies.
- **Faire** — a marketplace for selling *to* retailers at wholesale; Nisaba's customers
  buy at wholesale and sell at retail.

### Ancillary integrations

**Should:**

- [ ] **A generic CSV/XML feed vendor plugin**, built in, with a screen that maps a
      supplier's columns to `VendorListing` fields and reads the feed over a URL or SFTP.
      Most wholesalers publish a feed rather than an API, so this covers suppliers without
      anyone writing a plugin. It depends on Phase 3's feed-supplier item (`parseCsv`,
      `parseXml`, the SFTP decision).
- [ ] **Off-site encrypted backups**, to a destination the customer chooses: an
      S3-compatible bucket (e.g. Backblaze B2) or a folder synced elsewhere (a NAS,
      Syncthing). Encrypt with a key the customer holds before anything leaves the machine.
      Local backups are a 1.0 gate (*Gate: data safety*); this is the step after.
- [ ] Notifications — low stock, an oversell incident, credentials about to expire, sync
      failing — to ntfy, email, Matrix, Slack or Discord (Phase 3, notification plugins).
- [ ] Dropship ordering with suppliers that support it, starting with Rothco (Phase 3,
      order plugins).

**Could:**

- [ ] **UPC/product-data lookup** — fill in brand, MPN, images and dimensions when creating
      a listing from a bare UPC. These are paid APIs with per-customer keys.
- [ ] **Shipping labels** (Shippo, EasyPost, ShipStation) and **accounting** (QuickBooks,
      Xero) — these would turn Nisaba into an order-management tool, which is a product
      decision more than an integration. If accounting is wanted, start with a CSV export
      of sales and stock valuation rather than an API integration; QuickBooks and Xero
      both require app review for production access, the hurdle the Amazon application
      failed.

## Cross-cutting

- [x] `config.example.toml` told users to manage products "via the TUI's Products tab";
      it now points at the app's Products pages
- [ ] Secret handling audit — `keyring` is a dependency, but confirm nothing (tokens, the
      company secret, plugin `secret: true` fields) is ever written to `config.toml`, logged,
      or included in an export. Plugin secrets already fail this — see Phase 3.
- [x] **`Db::open` deletes the WAL when turso panics** (`db.rs` ~50–64), and retries. The
      comment protects only the main file, but a WAL can hold committed transactions not
      yet checkpointed into it, so this can throw away recent writes. Back up the files and
      surface the error instead of deleting. Fixed in PR #5.
- [x] Export/import is lossy. Export contains no secrets (verified 2026-10-19). But:
      - import drops `on_hand_quantity`, `source_plugin_id` and `source_vendor_item_id`
        (`db.rs` ~3414), and its `INSERT OR REPLACE` wipes them on variants that already
        exist;
      - export (`list_all_mappings`) includes soft-deleted mappings without `deleted_at`,
        so import revives them as active and the engine pushes to them.
      `ExportData` lives in `core/src/types.rs` (~397) and is built in
      `tauri-app/src/commands/export_import.rs`. It has no tests.
- [x] Config settings that do nothing, and a save that writes secrets:
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
- [ ] Swallowed errors: `cache_listing_detail` ignores four database writes
      (`listings.rs` ~538–569), and the pricing snapshot hard-codes `"USD"` (~98).
- [x] **Untrusted HTML runs in a webview with full IPC access.** CSP is disabled
      (`tauri.conf.json:31`, `"csp": null`). Marketplace listing descriptions
      (`products/[id]/index.vue` ~1093) and vendor-plugin descriptions
      (`DescriptionEditor.vue:143`, reached from `publish.vue`) are rendered with `v-html`.
      Script in a supplier's, a plugin's or a marketplace listing's HTML can therefore call
      any Tauri command, including `get_company_secret` and `get_config`, which returns
      platform secrets. That is a way around the Deno sandbox from the other side. Set a
      strict CSP with no inline script, sanitize every `v-html` sink (DOMPurify), and narrow
      the Tauri capability set so the main window cannot read secrets it never displays.
      - N-2.7 (2026-10-20) landed the capability-narrowing third: `crates/tauri-app/permissions/`
        allow-lists + `capabilities/settings.json` deny `get_company_secret`, `get_config` and
        `get_company_platform_config` on the main window; those commands are invocable only
        from the dedicated `nisaba-settings` window hosting `/config`, `/company` and the
        existing-company `/setup` flow (a narrow `get_xmrbazaar_monero_address` keeps the
        publish page's payout-address prefill working). REMAINDER: strict CSP (still
        `"csp": null`) and DOMPurify sanitization of the `v-html` sinks — injected script in
        the main window can still invoke every non-secret command.
- [x] Money is `f64` from end to end (`core/src/types.rs` ~258, ~313). The XMR and
      Squarespace adapters serialize prices with `to_string()`, so a computed price can be
      sent as `0.30000000000000004`. Use a decimal type, or integer minor units plus a
      currency. N-6.1 (2026-10-20) landed this: `ListingPrice.amount`,
      `PricingSnapshot.amount`, `PlatformListing.price` and `VendorListingCache.price` are
      now `rust_decimal::Decimal` (exact JSON-number serde, so the Tauri ↔ frontend
      contract keeps `amount: number`); all four adapters parse platform price strings
      into `Decimal` and serialize with `Decimal::to_string` (tests assert a computed
      `0.1 + 0.2` goes out as `0.3`, never `0.30000000000000004`); the three price
      columns (`pricing_snapshots.amount`, `vendor_listings_cache.price`,
      `product_vendor_data.price`) are TEXT holding the exact decimal string via
      migration 024, which converts legacy REAL values with `CAST`. Plugin
      `VendorListing.price` is now an exact decimal at the plugin boundary
      (N-7.2 froze the shape): plugins send the decimal text, a JSON number
      from a pre-freeze plugin is still accepted via `Decimal::from_f64`
      (shortest round-trip), and no `f64` remains on the path.
- [ ] Structured logging levels that are useful in the shipped app, not just `tracing` defaults
- [ ] Nuxt 4 migration. The lockfile pins Nuxt 3.21.1, `radix-vue` 1.9 and Tailwind 3.4.
      Nuxt 3's maintenance window is closing, and `radix-vue` has been succeeded by
      `reka-ui`. Plan the move to Nuxt 4 + `reka-ui` (and decide on Tailwind 4) as one
      migration, behind the typecheck gate from Phase 0.
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
- [x] A scheduled `cargo update` / `cargo audit` CI job. This run found two dependency
      breakages by accident — a transitive C library that stopped compiling on current
      compilers, and a direct dependency that required nightly — and both had been sitting
      in the tree unnoticed because CI never got past the format check.
- [ ] `proc-macro-error2 v2.0.1` is flagged future-incompatible by cargo (`cargo report
      future-incompatibilities`). It arrives via `getset` → `tor-dircommon` → the arti
      stack, so it is fixed by an arti bump rather than anything in this tree — worth
      re-checking whenever `onyums`/`artiqwest` move.
