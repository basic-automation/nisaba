# [Nisaba Roadmap](docs/roadmap/README.md)

## [Priority queue (code review, 2026-10-19)](docs/roadmap/priority-queue.md)

## [1.0 release](docs/roadmap/1.0-release.md)

### [Scope decisions (settle first)](docs/roadmap/1.0-release.md#scope-decisions-settle-first)

- [x] **Is P2P in 1.0?**
- [x] **Credential model: customers bring their own API keys**
- [x] **Which adapters are "1.0"?**

### [Gate: correctness](docs/roadmap/1.0-release.md#gate-correctness)

- [ ] Priority queue items 1–5 closed
- [ ] Phase 4 *Stock correctness* closed, including the stock ledger
- [ ] Phase 2 *Writes that report success without writing* closed, with a test per write path that a rejected write surfaces as `Err`
- [ ] Live verification per stable adapter, recorded in this file with the date and what was exercised
- [ ] A 7-day soak on a real catalog with the dry-run preview (Phase 4) on first and then live, with no oversell incident and no unexplained stock change

### [Gate: data safety](docs/roadmap/1.0-release.md#gate-data-safety)

- [x] Transactional migrations, plus upgrade tests from a populated database at *every released version* (0.1.0 onward), run in CI (Phase 1)
- [x] Automatic database backup before every migration and on a schedule (keep the last N), with a *Restore from backup* action in the UI
- [x] The WAL-deletion and leave-company data-loss bugs fixed (Priority queue item 1)
- [x] Lossless export/import, round-trip tested (Cross-cutting)

### [Gate: security](docs/roadmap/1.0-release.md#gate-security)

- [x] Strict CSP and sanitised `v-html` (Cross-cutting)
- [ ] Plugin re-approval on code change, and plugin secrets in the keyring (Phase 3)
- [ ] Secret-handling audit closed
- [ ] Tauri capabilities narrowed so the main window can invoke only the commands it uses, and secret-returning commands are reachable only from the settings views that display them
- [x] `cargo audit` and `npm audit` clean in CI, on a schedule (Cross-cutting)
- [ ] GitHub private vulnerability reporting enabled (Phase 0)

### [Gate: release engineering](docs/roadmap/1.0-release.md#gate-release-engineering)

- [x] Updater end to end (Phase 6)
- [ ] **Update-path test.**
- [ ] The tag-push release path proven on a `v0.x` tag before it is trusted with `v1.0.0` (Phase 6)
- [ ] Windows code signing, with an Authenticode certificate or a cloud signing service
- [ ] macOS Developer ID signing and notarization, or a documented decision to ship unsigned with the right-click-Open workaround (Phase 6)
- [x] CI green with no `continue-on-error`
- [ ] A version-consistency check across `Cargo.toml`, `tauri.conf.json` and `frontend/package.json` (Phase 6)
- [x] Decide `crates/service` and `crates/tui`

### [Gate: product](docs/roadmap/1.0-release.md#gate-product)

- [x] **First-run setup wizard.**
- [x] Credential health in the UI
- [x] Empty and error states, confirmation of destructive actions, and the silent-startup fix (Phase 7)
- [x] **Export diagnostics**
- [ ] An accessibility pass on the core flows (Phase 7)

### [Gate: documentation](docs/roadmap/1.0-release.md#gate-documentation)

- [x] A user guide
- [ ] The plugin author reference and published types (Phase 3, *Plugin API v1*)
- [x] README screenshots (Phase 0)
- [ ] Release notes on every GitHub release, generated from PR titles

### [Gate: legal and platform terms](docs/roadmap/1.0-release.md#gate-legal-and-platform-terms)

- [x] Third-party license notices bundled in the app
- [x] Each platform's API terms reviewed against what Nisaba does
- [x] eBay **Marketplace Account Deletion** notifications
- [x] A short privacy statement

### [Stability contract at 1.0](docs/roadmap/1.0-release.md#stability-contract-at-10)

- [ ] Write down what is frozen and how it versions

## [Phase 0 — Public release readiness](docs/roadmap/phase-0-public-release-readiness.md)

- [x] MIT `LICENSE` at the repo root, matching `Cargo.toml`'s `workspace.package.license`
- [x] `README.md`
- [x] `ROADMAP.md`
- [x] Remove local scratch tooling with personal paths (`cleanup.ps1`, `parse_har.py`)
- [x] Stop redistributing licensed Adobe Fonts
- [x] Purge the licensed `.woff2` blobs and the personal-path scripts from git history
- [x] Point the Tauri updater endpoint at this repo instead of the stale `nicbudd/nisaba-releases`
- [x] CI
- [x] **CI's format gate is green**
- [x] **The workspace could not build on stable at all.**
- [ ] Take `onyums 0.5.0` once `artiqwest` catches up
- [x] Clear the clippy backlog behind CI's `-D warnings` gate
- [x] `crates/core`'s `turso 0.5` pulled in `aegis 0.9.7`, whose vendored C (`libaegis`) would not compile
- [x] Normalize line endings
- [x] Decide `rust-toolchain.toml`
- [x] `SECURITY.md` with a disclosure path
- [ ] Enable GitHub private vulnerability reporting on the repo (Settings → Code security)
- [x] CI's Rust job could not build `nisaba-tauri`
- [x] Screenshots in the README
- [x] README accuracy
- [x] `cleanup.ps1` and `parse_har.py` were purged from history but still sit untracked in at least one checkout and are not gitignored, so a `git add -A` re-adds them
- [x] **The frontend typecheck has never run.**

## [Phase 1 — Test and verification foundation](docs/roadmap/phase-1-test-and-verification-foundation.md)

- [x] Fixture-based tests for each adapter's `mapping.rs`, over recorded response *shapes*
- [ ] Capture real (redacted) API/HTML responses for the fixtures
- [x] `crates/platform-xmrbazaar/src/scraper.rs`
- [x] `SyncEngine` tests over a fake `PlatformAdapter` (17 then, 22 now)
- [x] `crates/vendor-runtime` tests (`tests/runtime.rs`, 22)
- [x] Sandbox escape tests
- [x] `crates/p2p` round-trip test (`tests/round_trip.rs`, 9)
- [x] Migration tests
- [x] Decide what the `008` gap in `migrations/` was
- [x] Renumber or rename `migrations/*.sql` so a file's number matches its schema version
- [x] **Migrations are not transactional.**
- [ ] Multi-cycle sync-engine tests

## [Phase 2 — Platform adapter completeness](docs/roadmap/phase-2-platform-adapter-completeness.md)

- [ ] Photo upload on eBay
- [ ] If eBay ever needs a true binary upload rather than `imageUrls`, it must go through the Media API (`createImageFromFile` / `createImageFromUrl`)
- [ ] Photo upload for Squarespace and Amazon
- [ ] Squarespace `set_price`
- [ ] Amazon description read/write via the SP-API listings feed
- [ ] Amazon is `enabled = false` by default in `config.example.toml` and has never been exercised end-to-end
- [x] `detect_sales()` is only meaningful for XMR Bazaar's stock-mode inventory
- [x] Amazon counts only `fulfillmentAvailability[0]`, so a listing stocked on both merchant-fulfilled and FBA reports one channel's quantity
- [x] Amazon takes `offers[0]` for price with no marketplace filtering, so a seller listed in more than one marketplace gets an arbitrary offer, and `to_listing` drops the currency
- [x] Amazon drops ERROR-severity `issues` in every mapping, so a suppressed listing reaches the UI as a normal listing with zero stock and no explanation
- [ ] eBay's two read paths key listings differently
- [x] `trading_to_listing` computes `Quantity - QuantitySold` with no floor, so eBay reporting more sold than listed yields a negative quantity in the mapping UI and makes that…
- [ ] Rate-limit handling per platform
- [x] Token refresh failure path
- [ ] A fifth adapter would prove the trait is actually general
- [x] eBay OAuth sends no `state` parameter (`platform-ebay/src/auth.rs` ~54–60), and the callback accepts a `code` from any page the auth window reaches (~279–283)
- [ ] Four `SyncError` variants are never constructed

### [Writes that report success without writing](docs/roadmap/phase-2-platform-adapter-completeness.md#writes-that-report-success-without-writing)

- [x] **eBay `set_quantity` swallows per-SKU errors.**
- [x] **eBay `set_price` probably never changes the price.**
- [x] **eBay read-modify-write erases fields it doesn't model.**
- [x] **eBay `update_listing` sends a partial body to `updateOffer`**
- [x] **Amazon PATCH bodies are double-wrapped, and failures report `Ok`.**
- [x] **Amazon pagination may read the wrong key.**
- [x] **XMR Bazaar writes cannot tell success from failure.**
- [x] **XMR `create_listing` can record the wrong listing id.**

### [Adapter correctness](docs/roadmap/phase-2-platform-adapter-completeness.md#adapter-correctness)

- [x] The in-memory order dedup in the XMR adapter consumes sales
- [ ] XMR orders always count as one unit (`scraper.rs` ~778, pinned by `sales_page.rs:71`)
- [x] XMR listing status
- [x] XMR error snippets slice `&body[..body.len().min(N)]` (`client.rs` ~183, ~407, ~672, ~776), which panics when byte N splits a UTF-8 character in the remote HTML
- [x] XMR `set_price` drops the currency (`lib.rs` ~290), and `set_description` posts raw HTML (~300) while `create_listing` strips tags (`client.rs` ~476)
- [x] eBay pagination ignores the `next` href (`types.rs` ~50, ~101)
- [x] Squarespace pagination loops forever if `hasNextPage` is true with a null cursor (`client.rs` ~81, ~135), and only `fetch_inventory` maps a 401 to `AuthError` (~57)
- [x] Squarespace `update_listing` silently drops `request.price` (`lib.rs` ~324–336)
- [ ] Hard-coded regions
- [ ] Dead code

## [Phase 3 — Vendor plugin platform](docs/roadmap/phase-3-vendor-plugin-platform.md)

- [x] Deno-based sandbox with `Nisaba.fetch`/`sleep`/`log`/`emitBatch`
- [x] Metadata read without executing `fetchListings`
- [x] Multi-file plugins with TypeScript transpile
- [x] Rothco Wholesale plugin on the GraphQL v2 API, with SKU variants, tiered pricing, UPC and weight
- [x] Document the plugin contract as a real reference, and a starter plugin so a third party can begin without reading the Rothco source
- [x] Harden the plugin importer (`extract_zip_to_files_map`)
- [x] **Plugin `secret: true` values are stored in plaintext.**
- [x] Plugin resource limits
- [x] `Nisaba.fetch` read every response body fully into Rust memory with no cap
- [x] Let the user raise a plugin's time limit, for a catalog that genuinely takes longer than 30 minutes (a rate-limited API backing off, say)
- [ ] Show the other limits (heap, output, response size) in the plugin's details, and decide whether any of them should be user-adjustable too
- [x] Per-plugin allowlist of fetchable hosts
- [x] **Updated plugin code runs without re-approval.**
- [x] Host-side fetches bypass the plugin allowlist
- [x] `AUTO_VENDOR_SYNC_RUNNING` (`vendors.rs` ~1482) is a process-wide flag, so one company's vendor sync skips every other company's, logged only at `debug`
- [x] Surface each plugin's network access at install time (first install only — in-place updates skip it; see the re-approval item above)
- [ ] Once the UI surfaces it, decide when an undeclared allowlist stops meaning "unrestricted"
- [ ] The `marketplace.vue` submit/approve/reject flow implies a plugin registry
- [ ] Plugin signing or checksum pinning before any non-local distribution happens
- [ ] Plugin version upgrade path

### [Plugin API v1 (freeze for 1.0)](docs/roadmap/phase-3-vendor-plugin-platform.md#plugin-api-v1-freeze-for-10)

- [x] An `api_version` in metadata
- [x] Fix `VendorListing` before freezing it
- [x] **Secrets from the keyring.**
- [x] **Per-plugin storage**
- [x] **Incremental sync.**
- [ ] **Host-managed OAuth for supplier APIs.**
- [ ] **Feed suppliers.**
- [x] Per-plugin schedule (an interval, plus manual runs) and a **run history** in the UI
- [ ] **Capabilities manifest.**
- [x] **Author tooling.**

### [Plugin kinds after 1.0](docs/roadmap/phase-3-vendor-plugin-platform.md#plugin-kinds-after-10)

- [ ] **Marketplace adapter plugins.**
- [ ] **Order (dropship) plugins.**
- [ ] **Notification plugins.**
- [ ] **Pricing rules**

## [Phase 4 — Sync engine depth](docs/roadmap/phase-4-sync-engine-depth.md)

- [x] ~~Quantity deltas and conflict resolution (`resolve_deltas`)~~
- [x] Cache-first listing loads and live/cached merge
- [x] Per-sync-cycle vendor data refresh
- [x] `conflict.rs` exposes one strategy, which the engine now uses as its resolution path (folded into the ledger below), rather than a dead user-selectable strategy

### [Stock correctness](docs/roadmap/phase-4-sync-engine-depth.md#stock-correctness)

- [x] **Sales on two platforms in one cycle are undercounted.**
- [x] **Stock changed in Nisaba never reaches the platforms.**
- [x] **A failed push is never retried, and a newly mapped listing is never corrected.**
- [x] **Variants are ignored.**
- [x] **Re-mapping an unmapped listing fails.**
- [ ] **An XMR order is marked processed before its sale is applied**
- [x] **Re-base the engine on a stock ledger.**
- [x] Pruning is never wired
- [x] The vendor sync blocks the platform sync loop
- [x] `run_cycle` holds the adapters' read lock for the whole cycle, network retries included (~135–629), so saving config waits for the cycle to finish
- [x] A dry-run mode that reports what a sync *would* change without writing to any platform
- [x] Surface dry-run in the UI and the headless daemon
- [x] Partial-failure semantics
- [x] Surface a per-cycle reconciliation report in the UI, not just `SyncEvent` rows…
- [ ] Oversell protection
- [x] Chart `InventoryHistoryEntry` analytics
- [x] **The sync schedule accepts anything and can kill the engine.**

## [Phase 5 — P2P and multi-company](docs/roadmap/phase-5-p2p-and-multi-company.md)

- [x] Tor onion service, peer client/server and company sync payloads
- [x] Multi-company data model and company logos
- [x] **Peers never converged, and could overwrite each other's edits.**
- [x] Dropship stock did not survive P2P
- [ ] Vendor stock is still per peer
- [ ] The sync payload carries platform auth tokens and plugin configuration protected only by Tor's transport encryption
- [ ] **`[company].enabled` is never read.**
- [x] Peer trust model
- [ ] Authenticate peers individually
- [ ] Enforce roles on the caller
- [x] Bound peer-supplied timestamps
- [ ] Consider a hybrid/logical clock for merges instead of wall time
- [x] Compare the company secret in constant time in `auth_middleware` (was `==`)
- [x] Auth tokens ping-ponged like variants did
- [ ] Key rotation for the company secret, with a migration path for existing peers
- [x] Conflict resolution for P2P merges was `merge_remote_payload`'s implicit policy
- [ ] Offline/rejoin behaviour after a peer has been away for longer than the retention window
- [ ] Bounded payload sizes

### [Today's P2P (Tor) — what the 2026-10-19 review found](docs/roadmap/phase-5-p2p-and-multi-company.md#todays-p2p-tor--what-the-2026-10-19-review-found)

- [x] **Leaving a legacy-migrated company deletes the app database.**
- [x] **The legacy company secret can be lost.**
- [ ] **Join cannot produce a syncing pair.**
- [ ] **Platform-sync round-robin is dead code.**
- [ ] **One onion identity for all companies, tied to the launch directory.**
- [ ] Hard 2 MiB request limit and no client timeout
- [x] The merge is not transactional
- [ ] No onion-service lifecycle
- [x] Product and variant deletes orphaned rows in `listing_descriptions`, `listing_photos`, `pricing_snapshots` and `cached_listings`
- [ ] Product and variant deletes are hard deletes (`db.rs` `delete_product`, `delete_variant`), so a merge re-inserts them from any peer that still has them
- [x] Peer admin commands (approve, reject, set role, remove) return `Ok` when the peer does not exist and have no admin check, unlike `get_company_secret` (`company.rs` ~438+)
- [ ] README still says "AES-GCM encrypted payloads" (`README.md:27`)

### [Sync v2: CRDT replication over iroh](docs/roadmap/phase-5-p2p-and-multi-company.md#sync-v2-crdt-replication-over-iroh)

#### [Step 0: decisions before code](docs/roadmap/phase-5-p2p-and-multi-company.md#step-0-decisions-before-code)

- [ ] **Privacy model.**
- [ ] **CRDT design doc**
- [ ] **Engine spike.**

#### [Step 1: transport — today's protocol over iroh](docs/roadmap/phase-5-p2p-and-multi-company.md#step-1-transport--todays-protocol-over-iroh)

- [ ] Persist each install's iroh secret key in the OS keyring, next to the company secret
- [ ] Use a `nisaba/sync/1` ALPN with an `iroh::protocol::Router` handler in place of the axum router
- [ ] Authorize by the connection's remote endpoint id
- [ ] Join by ticket
- [ ] Enforce roles on the caller's endpoint id
- [ ] Revocation instead of rotation
- [ ] Elect the designated peer (`is_our_turn_to_sync`) on endpoint ids instead of onion addresses
- [ ] Resolve `[company].enabled` (above) as "keep this install off P2P" rather than "off Tor"
- [ ] Transport tests on loopback endpoints with relays off

#### [Step 2: the CRDT data layer](docs/roadmap/phase-5-p2p-and-multi-company.md#step-2-the-crdt-data-layer)

- [ ] Scalar fields
- [ ] Collections
- [ ] **Stock is not a register.**
- [ ] **Marketplace writes stay outside the CRDT.**
- [ ] **Secrets stay out of the replicated log in plaintext.**
- [ ] Sign every op with its author's endpoint key, and validate it before applying

#### [Step 3: oversell protection — escrow over the stock CRDT](docs/roadmap/phase-5-p2p-and-multi-company.md#step-3-oversell-protection--escrow-over-the-stock-crdt)

- [ ] Rights state per variant
- [ ] Gate every decrement on available rights
- [ ] Allocation policy for each peer's share
- [ ] Rights transfer
- [ ] Rebalancing
- [ ] Revocation and lost keys
- [ ] Fold Phase 4's "oversell protection: a configurable reserve buffer" into this
- [ ] Tests

#### [Step 4: replication protocol](docs/roadmap/phase-5-p2p-and-multi-company.md#step-4-replication-protocol)

- [ ] A per-author op log with sequence numbers
- [ ] Compaction
- [ ] Push on change
- [ ] Schema evolution

#### [Step 5: verification and migration](docs/roadmap/phase-5-p2p-and-multi-company.md#step-5-verification-and-migration)

- [ ] Property-based convergence tests with `proptest`
- [ ] Migration

## [Phase 6 — Release and distribution](docs/roadmap/phase-6-release-and-distribution.md)

- [x] Signing key for the Tauri updater
- [x] Pin `createUpdaterArtifacts` to the v2 format rather than `"v1Compatible"` when the release workflow lands
- [ ] Track the Tauri 3 alphas before they land on a stable line
- [x] `.updater/latest.json` published from a real release workflow
- [x] Correction to the item above about pinning `createUpdaterArtifacts`
- [x] The updater checks a GitHub raw URL at launch and every 6 hours (`useUpdater.ts` ~61–65), outside Tor and with no opt-out
- [x] CI runners
- [x] CI has no macOS leg, although every release ships a universal macOS bundle, and `release.yml` runs no tests before publishing
- [x] A `release.yml` that builds **Linux, Windows and macOS** bundles and attaches them to the tag's GitHub release
- [x] Prove the release matrix green
- [ ] The tag-push path of `release.yml` has never run
- [x] `bundle` in `tauri.conf.json` had **no `active` flag, which defaults to `false`**
- [ ] macOS code signing and notarization
- [x] Decide whether `crates/service` (headless sync daemon) and `crates/tui` ship as real products
- [x] Version consistency

## [Phase 7 — Frontend and UX](docs/roadmap/phase-7-frontend-and-ux.md)

- [x] Nuxt 3 SPA with Tailwind and shadcn-vue-style components (`components/ui/`, on `radix-vue`), covering products, listings, vendors, inventory and companies
- [x] Multi-tier pricing display, dealer price, UPC and weight on vendor cards
- [x] Empty and error states
- [x] Confirm destructive actions, using one mechanism
- [x] Sync status goes stale
- [x] N+1 IPC calls
- [x] Type drift between TypeScript and Rust
- [x] Dead code
- [ ] The publish flow (`products/[id]/publish.vue`) should show, per platform, exactly which fields will be written and which are unsupported, driven by `PlatformCapabilities`
- [x] The per-platform sales breakdown is always empty
- [ ] Startup failures are silent
- [ ] Accessibility pass
- [x] `frontend/public/guide/` is an empty directory with a `.gitkeep`
- [ ] The window is `"decorations": false`
- [x] A WebDriver harness that drives the real app
- [ ] Grow the UI smoke test beyond the marketplace
- [x] **Fix the CI UI smoke job, which fails on PR #2**

## [Services and integrations](docs/roadmap/services-and-integrations.md)

### [Hosted infrastructure](docs/roadmap/services-and-integrations.md#hosted-infrastructure)

- [x] **No OAuth broker.**
- [ ] Post-1.0, revisit a **shared eBay keyset** only
- [ ] **iroh relay**
- [ ] **Update manifest**
- [ ] **Plugin registry**
- [x] **No telemetry or crash-reporting service.**

### [Marketplace and storefront integrations](docs/roadmap/services-and-integrations.md#marketplace-and-storefront-integrations)

#### [Priority: WordPress and the WooCommerce ecosystem](docs/roadmap/services-and-integrations.md#priority-wordpress-and-the-woocommerce-ecosystem)

- [ ] **WooCommerce adapter**
- [ ] **Live verification in CI.**
- [ ] **Blind stock writes race checkout.**
- [ ] **Companion WordPress plugin "Nisaba Connector"**
- [ ] **Channel extensions instead of direct adapters.**
- [ ] **Crypto checkout guide.**
- [ ] **WordPress.com-hosted WooCommerce.**
- [ ] **Move a catalog into WooCommerce.**
- [ ] *Could:* WordPress multi-vendor marketplaces such as Dokan, WCFM and WC Vendors, for sellers who list on someone else's WordPress-based marketplace

#### [Should — after WordPress, in recommended order](docs/roadmap/services-and-integrations.md#should--after-wordpress-in-recommended-order)

- [ ] **Shopify**
- [ ] **Google Merchant Center**
- [ ] **Square POS — if customers sell in person**
- [ ] **Walmart Marketplace**
- [ ] Build these as **marketplace plugins** rather than new Rust crates (Phase 3, *Plugin kinds after 1.0*)

#### [Could — build when a customer actually sells there](docs/roadmap/services-and-integrations.md#could--build-when-a-customer-actually-sells-there)

- [ ] **BigCommerce**
- [ ] **Meta (Facebook/Instagram) catalog**
- [ ] **TikTok Shop**
- [ ] **BTCPay Server store and point-of-sale apps**
- [ ] **GunBroker**
- [ ] Other crypto-native marketplaces besides XMR Bazaar

#### [Not building, and why](docs/roadmap/services-and-integrations.md#not-building-and-why)

### [Ancillary integrations](docs/roadmap/services-and-integrations.md#ancillary-integrations)

#### [Should](docs/roadmap/services-and-integrations.md#should)

- [ ] **A generic CSV/XML feed vendor plugin**
- [ ] **Off-site encrypted backups**
- [ ] Notifications
- [ ] Dropship ordering with suppliers that support it, starting with Rothco (Phase 3, order plugins)

#### [Could](docs/roadmap/services-and-integrations.md#could)

- [ ] **UPC/product-data lookup**
- [ ] **Shipping labels**

## [Cross-cutting](docs/roadmap/cross-cutting.md)

- [x] `config.example.toml` told users to manage products "via the TUI's Products tab"
- [ ] Secret handling audit
- [x] **`Db::open` deletes the WAL when turso panics**
- [x] Export/import is lossy
- [x] Config settings that do nothing, and a save that writes secrets
- [ ] Swallowed errors
- [x] **Untrusted HTML runs in a webview with full IPC access.**
- [x] Money is `f64` from end to end (`core/src/types.rs` ~258, ~313)
- [ ] Structured logging levels that are useful in the shipped app, not just `tracing` defaults
- [ ] Nuxt 4 migration
- [ ] Dependency freshness pass
- [x] A scheduled `cargo update` / `cargo audit` CI job
- [ ] `proc-macro-error2 v2.0.1` is flagged future-incompatible by cargo (`cargo report future-incompatibilities`)
