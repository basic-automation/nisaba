# [Nisaba Roadmap](docs/roadmap/README.md)

## [Priority queue (code review, 2026-10-19)](docs/roadmap/priority-queue.md)

## [1.0 release](docs/roadmap/1.0-release.md)

### [Scope decisions (settle first)](docs/roadmap/1.0-release.md#scope-decisions-settle-first)

Settled by the owner on 2026-10-19. P2P is not in 1.0: it ships as a single-machine
product, with company sync behind a "Preview" switch that is off by default. Data-loss
fixes to today's P2P path still land. The stock ledger (Phase 4) must also land before
1.0, because it changes the data model. Customers bring their own API keys for Amazon,
Squarespace and XMR Bazaar, since Nisaba's own Amazon SP-API application was denied. eBay
is the exception (owner decision, 2026-10-28): 1.0 ships one-click eBay sign-in through a
small hosted OAuth broker holding Nisaba's own eBay keyset, with bring-your-own keys kept
as an advanced option. An adapter ships as stable only after a recorded live-account
verification, and otherwise as *Beta* or disabled. Amazon ships as Beta.

### [Gate: correctness](docs/roadmap/1.0-release.md#gate-correctness)

- [ ] Priority queue items 1–5 closed
- [ ] Phase 4 *Stock correctness* closed, including the stock ledger
- [ ] Phase 2 *Writes that report success without writing* closed, with a test per write path that a rejected write surfaces as `Err`
- [ ] Live verification per stable adapter, recorded in this file with the date and what was exercised
- [ ] A 7-day soak on a real catalog with the dry-run preview (Phase 4) on first and then live, with no oversell incident and no unexplained stock change

### [Gate: data safety](docs/roadmap/1.0-release.md#gate-data-safety)

Met. Migrations are transactional, and CI tests upgrades from a populated database at
every released version from 0.1.0 on. The database is backed up automatically before every
migration and on a schedule, keeping the last N, with a *Restore from backup* action;
backups never leave the machine. PR #5 fixed the WAL-deletion and leave-company data-loss
bugs. Export/import is lossless and round-trip tested. From 1.0 the export format carries
a version, and every later release imports every earlier 1.x export.

### [Gate: security](docs/roadmap/1.0-release.md#gate-security)

Done: a strict CSP with sanitised `v-html` (Cross-cutting), and `cargo audit` plus
`npm audit` running clean in CI on a schedule. Plugin re-approval on code change and
plugin secrets in the keyring are done too (Phase 3): a changed code hash or allowlist
resets a plugin to pending, and `secret: true` values live in the OS keyring, never in
`config_json` or the P2P payload.

- [ ] Secret-handling audit closed
- [ ] Tauri capabilities narrowed so the main window can invoke only the commands it uses, and secret-returning commands are reachable only from the settings views that display them
- [ ] GitHub private vulnerability reporting enabled (Phase 0)

### [Gate: release engineering](docs/roadmap/1.0-release.md#gate-release-engineering)

Done. The updater works end to end: a signing key, `createUpdaterArtifacts`, a
`latest.json` assembled by the workflow, and an endpoint that only a published release can
change. CI is green with no `continue-on-error`. It covers the typecheck, the UI smoke
test and a macOS leg, on runners moved off the deprecated Node 20 actions.
`crates/service` and `crates/tui` were deleted rather than shipped (N-10.3, 2026-10-22). A
headless daemon can return after 1.0, designed against the per-company encrypted config
model. The version-consistency check across `Cargo.toml`, `tauri.conf.json` and
`frontend/package.json` runs as its own CI job (N-10.2).

- [ ] **Update-path test.**
- [ ] The tag-push release path proven on a `v0.x` tag before it is trusted with `v1.0.0` (Phase 6)
- [ ] Windows code signing, with an Authenticode certificate or a cloud signing service
- [ ] macOS Developer ID signing and notarization, or a documented decision to ship unsigned with the right-click-Open workaround (Phase 6)

### [Gate: product](docs/roadmap/1.0-release.md#gate-product)

Shipped: a first-run setup wizard (N-11.1). Each platform's credentials step links to the
developer portal, gives the exact redirect URI to register (the eBay RuName) and has a
*Test connection* button; the Amazon step explains SP-API developer registration and that
approval is Amazon's decision. Credential health (N-11.2) warns ahead of eBay
refresh-token expiry and of Amazon's 365-day client-secret rotation, turns a
`refresh_auth` failure mid-sync into a persistent banner, and links each warning to the
wizard step that renews the credential. Empty and error states, confirmation of
destructive actions and the silent-startup fix also shipped: a startup failure now shows a
native error dialog naming the failing stage and writes a log (N-11.5).
*Export diagnostics* (N-11.4) zips versions, logs and redacted config from Settings →
About, and replaces any telemetry or crash-reporting service.

- [ ] An accessibility pass on the core flows (Phase 7)
- [ ] One-click eBay sign-in through a hosted OAuth broker (shared eBay keyset)

### [Gate: documentation](docs/roadmap/1.0-release.md#gate-documentation)

Shipped: the user guide (N-12.1) and the README screenshots (N-12.5). The guide is static
HTML in `frontend/public/guide/`, covering setup, credentials, mappings, sync, backups,
troubleshooting and eBay account deletion. It opens in a help window from the sidebar and
from Settings → About. The UI smoke harness captures the screenshots from the real app on
a seeded sample catalog, and CI commits them to `docs/images/`.

- [ ] The plugin author reference and published types (Phase 3, *Plugin API v1*)
- [ ] Release notes on every GitHub release, generated from PR titles

### [Gate: legal and platform terms](docs/roadmap/1.0-release.md#gate-legal-and-platform-terms)

Met. An *About → Licenses* view bundles third-party notices, generated by `cargo-about`
and an npm license checker. eBay's, Amazon SP-API's, Squarespace's and XMR Bazaar's terms
were reviewed against what Nisaba does on 2026-10-23, in the
[platform terms review](docs/roadmap/platform-terms-review.md) (N-12.4). Its follow-ups
are an Amazon 429/backoff audit, a Squarespace User-Agent check, an XMR Bazaar
residual-risk note in the guide, and a re-review whenever order or buyer data is first
read. The eBay Marketplace Account Deletion rule was confirmed: subscribe or opt out
before the first production call. Each customer using their own keyset does this
themselves, following a walkthrough in the in-app guide (N-12.1). A privacy statement ships as `PRIVACY.md` and
as a page in the in-app guide (N-12.4).

### [Stability contract at 1.0](docs/roadmap/1.0-release.md#stability-contract-at-10)

- [ ] Write down what is frozen and how it versions

## [Phase 0 — Public release readiness](docs/roadmap/phase-0-public-release-readiness.md)

The repository was made publishable. It has an MIT `LICENSE`, a `README.md` and this
roadmap. `SECURITY.md` points reporters at GitHub private vulnerability reporting and says
what counts as a vulnerability. The licensed Adobe Fonts are no longer redistributed: the
`.woff2` files are gitignored and the app falls back to system font stacks. The font blobs
and the personal-path scripts (`cleanup.ps1`, `parse_har.py`) were purged from git
history, and stray untracked copies are kept out of `git add -A`. The updater endpoint
points at this repo instead of the stale `nicbudd/nisaba-releases`. The tree is LF
everywhere, with `.gitattributes` to keep it so. `rust-toolchain.toml` pins `stable` with
rustfmt and clippy. The README gained screenshots (N-12.5). Its accuracy was also fixed:
the Node.js requirement (`^20.19.0 || >=22.12.0`, enforced by an `engines` field), a claim
that analytics were charted when they were not, and the undocumented `./config.toml`
override.

CI runs `cargo fmt --check`, build, test and `clippy -D warnings` on Linux and Windows,
plus a frontend build. It had never got past the format check, so a single rustfmt pass
made it green, and the build behind it then failed on stable. `onyums 0.2.5` needed a
nightly feature; the fix moved to `onyums 0.3.1` with `artiqwest 0.4.1`, which must move
together because both embed arti. `aegis 0.9.7`'s vendored libaegis broke because Clang 22
and GCC 16 dropped the `evex512` target token (the cause was not `-mtune=native`);
`aegis 0.9.19` fixed it. The Rust job builds the frontend before cargo, because
`nisaba-tauri` needs the generated `frontendDist`. Clippy had no backlog: it exits 0
across all eight member crates. The frontend typecheck had never actually run. It now runs
with a pinned `vue-tsc` and without `continue-on-error`. The item also noted there was no
frontend lint or test step.

- [ ] Take `onyums 0.5.0` once `artiqwest` catches up
- [ ] Enable GitHub private vulnerability reporting on the repo (Settings → Code security)

## [Phase 1 — Test and verification foundation](docs/roadmap/phase-1-test-and-verification-foundation.md)

Tests now cover each adapter's mapping layer over hand-built response shapes: Squarespace
15, Amazon 13, and eBay 21 across the Inventory API's JSON and the Trading API's XML. XMR
Bazaar's scraper has golden-file tests, 15 for `parse_edit_form` and 9 for
`parse_sales_page`. Beyond the adapters there are tests for the sync engine over a fake
adapter (22), the vendor runtime (22), sandbox escapes (9) and a P2P payload round trip
without Tor (9). The migration tests show an empty database reaches all 22 tables and that
a second `migrate()` is a no-op. Writing these tests found real defects, all fixed.
`parse_edit_form` re-posted every named control on the page, so a quantity update also
submitted unchecked checkboxes as checked, and every radio option. The first stock-mode
push never persisted its version tag, so every later cycle re-pushed the stock mode to the
live XMR Bazaar listing. The plugin sandbox had three escapes: a file write anywhere
through zip entry names at install time, a module read anywhere through `file://` imports,
and `op_panic` aborting the whole app. `Nisaba` is now a plugin's only API. The round-trip
test found the P2P convergence bug (Phase 5).

On migrations, the `008` gap is intentional: steps that are an idempotent `ALTER TABLE` or
a data repair are written inline in Rust. `migrations/*.sql` were renumbered so each
file's number equals its schema version (N-3.2, 2026-10-20).
`014_cached_platform_listings.sql` became `017_…`, and the never-included `004`, `010` and
`012` files were deleted. The `if version < N` gate remains the runtime authority. Each
migration step is now transactional, and upgrades are tested from a populated database,
not only an empty one.

- [ ] Capture real (redacted) API/HTML responses for the fixtures
- [ ] Multi-cycle sync-engine tests

## [Phase 2 — Platform adapter completeness](docs/roadmap/phase-2-platform-adapter-completeness.md)

N-5.4 fixed Amazon's mapping. Quantity now sums every fulfilment channel instead of
reading `fulfillmentAvailability[0]`. Price comes from the configured marketplaces, in
configured order, and keeps its currency. ERROR-severity issues are carried through
`extras` instead of being dropped. On eBay, the Trading API mapping floors
`Quantity - QuantitySold` at zero (N-5.6).

eBay OAuth now sends and checks a `state` parameter. A failed `refresh_auth` mid-sync
raises a persistent dashboard banner that links to the setup wizard, and the
credential-health panel warns before expiry (N-11.2). Checked and left as is:
`detect_sales()` runs only for XMR Bazaar's stock-mode inventory, because `run_cycle`
gates it. The other platforms are therefore not silently reporting "no sales".

- [ ] Photo upload on eBay
- [ ] If eBay ever needs a true binary upload rather than `imageUrls`, it must go through the Media API (`createImageFromFile` / `createImageFromUrl`)
- [ ] Photo upload for Squarespace and Amazon
- [ ] Squarespace `set_price`
- [ ] Amazon description read/write via the SP-API listings feed
- [ ] Amazon is `enabled = false` by default in `config.example.toml` and has never been exercised end-to-end
- [ ] eBay's two read paths key listings differently
- [ ] Support eBay's modernized REST and GraphQL APIs
- [ ] Rate-limit handling per platform
- [ ] A fifth adapter would prove the trait is actually general
- [ ] Four `SyncError` variants are never constructed

### [Writes that report success without writing](docs/roadmap/phase-2-platform-adapter-completeness.md#writes-that-report-success-without-writing)

Every write path that the 2026-10-19 review found returning `Ok` without writing is
closed. On eBay, the `set_quantity` and `set_price` items are closed. `set_quantity`
swallowed per-SKU errors. `set_price` sent an offer with no `offerId`, never parsed the
response, and could fall back to quantity 0. Inventory-item writes now rebuild eBay's
full-replace body from the item just fetched. They model brand, MPN, UPC/EAN/ISBN/ePID,
condition and package fields, and carry every other field through. They also omit
quantity, so a stale read cannot bring back sold stock. `update_listing` fetches the
complete offer and refuses to send a partial `updateOffer` body (N-5.2).

On Amazon, PATCH bodies are no longer double-wrapped. Every write checks its submission
and fails on an `INVALID` status or any ERROR-severity issue (N-5.3). Pagination reads the
nested `pagination.nextToken`, falling back to the top-level key (N-5.4). On XMR Bazaar,
edits post without following redirects and must read their saved values back before
reporting success. A login bounce is an `AuthError`, and `set_stock_mode` fails when there
is no stock field. `create_listing` identifies the new listing from the redirect or from a
before-and-after diff of my-listings, never from links in the response body (N-5.5).
Fake-server tests cover the rejected-write cases on Amazon and XMR Bazaar.

### [Adapter correctness](docs/roadmap/phase-2-platform-adapter-completeness.md#adapter-correctness)

On XMR Bazaar, the in-memory order dedup is gone. It let a dry run or a failed cycle
consume a sale. Order marks now commit in the same transaction as the stock write that
uses the sale (N-4.4). N-5.6 fixed four more XMR Bazaar defects. Listing status matches
`active` exactly, so an inactive listing no longer reads as in stock. Error snippets
truncate on a UTF-8 character boundary instead of panicking. `set_price` sends the
currency with the price. `set_description` strips tags the way `create_listing` does.

N-5.6 also fixed pagination and Squarespace's error handling. eBay follows the response's
`next` href. Squarespace used to loop forever when `hasNextPage` was true with a null
cursor, and now stops on a null, empty or repeated cursor. Squarespace's `fetch_products`
maps a 401 to `AuthError`, and `update_listing` returns `Err` when asked for a price,
instead of silently dropping it.

- [ ] XMR orders always count as one unit (`scraper.rs` ~778, pinned by `sales_page.rs:71`)
- [ ] Hard-coded regions
- [ ] Dead code

## [Phase 3 — Vendor plugin platform](docs/roadmap/phase-3-vendor-plugin-platform.md)

The plugin platform shipped. It has a Deno sandbox exposing `Nisaba.fetch`, `sleep`, `log`
and `emitBatch`, metadata reads that do not run `fetchListings`, and multi-file plugins
with TypeScript transpile. The first plugin is Rothco Wholesale, on the GraphQL v2 API,
with SKU variants, tiered pricing, UPC and weight. `plugins/template` is both the contract
reference and a starter plugin, and tests run it against a fake supplier API.
`PluginLimits` bounds every run's wall-clock time, V8 heap and output. Before it, a plugin
allocating in a loop killed the whole app. `Nisaba.fetch` streams response bodies under a
size cap. Users can raise a plugin's time limit from 15 minutes to 12 hours, per machine;
the UI smoke test verified this in the running app.

A plugin declares `allowed_hosts`, which `Nisaba.fetch` enforces on every redirect hop.
Module top-level code, which runs at install time, gets no network. A plugin that declares
nothing stays unrestricted, so existing installs keep working. Its network access shows on
the marketplace card at first install; that was verified by a build and a database test,
not yet in the running app. Updated plugin code no longer runs without re-approval: a
changed code hash or allowlist resets the plugin to pending, and the run paths check
status. The host-side gaps are closed. `cache_images` fetched any URL with no host filter
or size cap, and `FetchPolicy` checked host names only. Private, loopback and link-local
addresses are now refused after DNS resolution. `secret: true` config values moved from
the plaintext database and P2P payload into the OS keyring, and they are per machine: a
peer that needs a token sets it locally (N-2.3). The importer caps zip entries and
decompressed size and refuses unsafe entry names. The auto-sync guard is now per company
instead of process-wide (N-7.5).

- [ ] Show the other limits (heap, output, response size) in the plugin's details, and decide whether any of them should be user-adjustable too
- [ ] Once the UI surfaces it, decide when an undeclared allowlist stops meaning "unrestricted"
- [ ] The `marketplace.vue` submit/approve/reject flow implies a plugin registry
- [ ] Plugin signing or checksum pinning before any non-local distribution happens
- [ ] Plugin version upgrade path

### [Plugin API v1 (freeze for 1.0)](docs/roadmap/phase-3-vendor-plugin-platform.md#plugin-api-v1-freeze-for-10)

Most of the v1 surface is in place. Plugin metadata carries an `api_version`, and the host
refuses a major version it does not know. The frozen `api_version: 1` surface is written
down in `plugins/template` and `SECURITY.md`. `VendorListing` was fixed before the freeze
(N-7.2). `images: string[]` replaces `image_url`, and a legacy value folds into
`images[0]`. Price is an exact decimal with a required currency. `quantity: null`, meaning
unknown, stays distinct from `0`. UPC/GTIN, brand, MPN, weight and dimensions are typed
fields, and `extras` stays free-form. Secrets come from the keyring and are injected into
every run path, never written to `config_json` or the P2P payload (N-2.3).

Plugins also gained `Nisaba.store`, a per-plugin key–value store: 1 MiB per plugin,
256-byte keys and string values, cleared on uninstall (N-7.3). Incremental sync lets
`fetchListings({ since, cursor })` return a `{ changes, deleted, cursor }` delta, which
the host merges in one transaction. Plugins that ignore the arguments keep full-pull
semantics (N-7.4). Each plugin has its own schedule, *Run now*, and a run history of
times, status, listings emitted, errors and log lines (N-7.5). For authors, `nisaba.d.ts`
ships in the template, generated from the Rust types with ts-rs and held in sync by a test
(N-12.2). The `nisaba-plugin` dev command and the fake-supplier test kit for authors were
not built; they are not 1.0 queue items.

- [ ] **Host-managed OAuth for supplier APIs.**
- [ ] **Feed suppliers.**
- [ ] **Capabilities manifest.**

### [Plugin kinds after 1.0](docs/roadmap/phase-3-vendor-plugin-platform.md#plugin-kinds-after-10)

- [ ] **Marketplace adapter plugins.**
- [ ] **Order (dropship) plugins.**
- [ ] **Notification plugins.**
- [ ] **Pricing rules**

## [Phase 4 — Sync engine depth](docs/roadmap/phase-4-sync-engine-depth.md)

Shipped: cache-first listing loads with a live/cached merge, and a vendor data refresh on
every sync cycle. The refresh updates `products.quantity`, which the ledger now carries to
the platforms. `resolve_deltas` is the engine's resolution path: each cycle turns every
platform's reading into a delta against its snapshot, and sums the deltas onto canonical
stock. The single strategy in `conflict.rs` became that path, folded into the ledger,
instead of a user-selectable strategy left as dead code.

### [Stock correctness](docs/roadmap/phase-4-sync-engine-depth.md#stock-correctness)

The engine was re-based on a stock ledger. Every change is a uniquely keyed event: a sale
(keyed by platform and order id), a restock, a manual adjustment or a vendor update.
Canonical stock is the sum of the events, and a push is the difference between canonical
and each platform's reading, per variant. Sync v2 will replicate the same model (Phase 5,
Step 2). The ledger closed the review's findings. Sales on two platforms in one cycle now
sum, so eBay 10 → 8 and Squarespace 10 → 9 resolve to 7. Stock changed in Nisaba reaches
every platform. A failed push is retried, and a new mapping corrected, on the next cycle.
Stock is keyed per variant (N-4.3). Re-mapping an unmapped listing works again: the unique
indexes are partial, and the mapping is recorded in the same transaction as the created
listing (N-4.5). The partial-failure policy is decided: never push to a platform that
could not be read this cycle. XMR orders are consumed in the same transaction as the stock
write that applies them, and a sale on an untracked product is left unconsumed (N-4.4).

`SyncEngine::dry_run()` reports planned changes without writing anything, locally or to
any platform. The dashboard offers it as *Preview sync*, next to a *Last sync cycle*
reconciliation card (N-11.6). The headless-daemon half was dropped along with
`crates/service` (N-10.3). Housekeeping prunes sync events after 90 days and downsamples
history to one row per product per day after 30. History is recorded only when the
quantity changes, and `execute_write` commits only on success (N-4.7). The vendor sync
runs as its own task, and the adapters' lock is no longer held for a whole cycle. The sync
schedule is an interval picker that rejects zero and applies on reload. Stock history is
charted (N-11.8).

- [ ] Oversell protection

## [Phase 5 — P2P and multi-company](docs/roadmap/phase-5-p2p-and-multi-company.md)

Shipped: the Tor onion service, a peer client and server, and company sync payloads, plus
the multi-company data model and company logos. The payloads carry no application-layer
encryption, only Tor's transport encryption. The peer trust model and the per-section
merge policy are written down in the README. The company secret is the only credential: a
secret holder receives everything, tokens included, and anything it sends is merged.
`tests/round_trip.rs` pins the merge policy.

Peers now converge instead of re-stamping and overwriting each other every cycle. Merges
keep the edit's timestamp, and `recalc_product_quantity` writes only rows that change.
Auth tokens had the same ping-pong, and now keep the edit's timestamp too. A variant's
source-plugin link now travels, so dropship stock survives P2P. Peer-supplied timestamps
that do not parse, or that run more than 10 minutes ahead, are refused and counted. The
company secret is compared in constant time.

- [ ] Vendor stock is still per peer
- [ ] The sync payload carries platform auth tokens and plugin configuration protected only by Tor's transport encryption
- [ ] **`[company].enabled` is never read.**
- [ ] Authenticate peers individually
- [ ] Enforce roles on the caller
- [ ] Consider a hybrid/logical clock for merges instead of wall time
- [ ] Key rotation for the company secret, with a migration path for existing peers
- [ ] Offline/rejoin behaviour after a peer has been away for longer than the retention window
- [ ] Bounded payload sizes

### [Today's P2P (Tor) — what the 2026-10-19 review found](docs/roadmap/phase-5-p2p-and-multi-company.md#todays-p2p-tor--what-the-2026-10-19-review-found)

PR #5 fixed both data-loss bugs. Leaving a legacy-migrated company no longer deletes the
app database, and the legacy company secret is no longer replaced by its hash when saving
it to the keyring fails. A merge is now one transaction, so an error rolls the whole merge
back. Product and variant deletes no longer leave orphaned rows. The peer admin commands
(approve, reject, set role, remove) returned `Ok` for a peer that did not exist and had no
admin check; that item is closed.

- [ ] **Join cannot produce a syncing pair.**
- [ ] **Platform-sync round-robin is dead code.**
- [ ] **One onion identity for all companies, tied to the launch directory.**
- [ ] Hard 2 MiB request limit and no client timeout
- [ ] No onion-service lifecycle
- [ ] Product and variant deletes are hard deletes (`db.rs` `delete_product`, `delete_variant`), so a merge re-inserts them from any peer that still has them
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

`release.yml` builds Linux, Windows and macOS bundles in one `fail-fast: false` matrix,
with a build-only dispatch mode. Linux builds on `ubuntu-22.04` for an older glibc, and
macOS builds a universal binary. Owner directive, 2026-09-25: every release covers all
three platforms, and a release missing one is a failed release. The first matrix run found
that `bundle.active` defaulted to `false`, so no bundle had ever been produced. With that
fixed and an explicit target list, build-only run 36218681088 (2026-09-26) produced the
project's first bundles on all three legs, unsigned. A `version-consistency` CI job
compares `Cargo.toml`, `tauri.conf.json` and `frontend/package.json` (N-10.2).

The updater is wired end to end. It has a signing key and `createUpdaterArtifacts` in the
v2 format; the key had been missing from `tauri.conf.json` entirely, so no `.sig` file
could exist. The release workflow assembles `latest.json`. Before, the endpoint was a raw
file on `master`, so any commit to it was an update push. Update checks run at launch and
every 6 hours, outside Tor, and now have an opt-out setting. CI's actions moved off the
deprecated Node 20 runtime, and CI gained a macOS leg. `crates/service` and `crates/tui`
were deleted rather than restored, because restoring them was close to a rewrite (N-10.3,
2026-10-22).

- [ ] Track the Tauri 3 alphas before they land on a stable line
- [ ] The tag-push path of `release.yml` has never run
- [ ] macOS code signing and notarization

## [Phase 7 — Frontend and UX](docs/roadmap/phase-7-frontend-and-ux.md)

Shipped: the Nuxt 3 SPA with Tailwind and shadcn-vue-style components on `radix-vue`. The
`shadcn-nuxt` module is a dependency but is not registered. Vendor cards show multi-tier
pricing, dealer price, UPC and weight. The in-app guide ships in `frontend/public/guide/`
(N-12.1). N-11.3 added empty and error states: product creation validates its fields and
shows backend errors inline, including a duplicate SKU; the dashboard shows an error
banner with a retry; *Sync now* shows a spinner and reports a failure; and inventory tells
a load error apart from an empty state. N-11.3 also routed every destructive action
through one `ConfirmDialog`, and made import preview-then-apply. The per-platform sales
breakdown now fills in, because the engine records the real platform on push events
(N-11.8).

Also closed: sync status is one shared store, batch commands replace per-product IPC
calls, the TypeScript types that had drifted from Rust are fixed, and the dead
composables, commands and npm dependencies are dealt with. A WebDriver harness,
`scripts/ui-smoke.py` with a `ui_fixture` install, drives the real app with no network,
inside `unshare -rn`. Its first green run was on Hyprland on 2026-09-26; there, WebKitGTK
needs `WEBKIT_DISABLE_DMABUF_RENDERER=1`. Its CI job was fixed by resolving `--target-dir`
to an absolute path. Startup failures are no longer silent: they show a native error
dialog naming the failing stage and write a log (N-11.5).

- [ ] The publish flow (`products/[id]/publish.vue`) should show, per platform, exactly which fields will be written and which are unsupported, driven by `PlatformCapabilities`
- [ ] Accessibility pass
- [ ] The window is `"decorations": false`
- [ ] Grow the UI smoke test beyond the marketplace

## [Services and integrations](docs/roadmap/services-and-integrations.md)

### [Hosted infrastructure](docs/roadmap/services-and-integrations.md#hosted-infrastructure)

Decided: no OAuth broker for Amazon, Squarespace or XMR Bazaar, because customers bring
their own API keys (owner decision, 2026-10-19, after Nisaba's own Amazon SP-API
application was denied). The exception is eBay, which gets a hosted broker for 1.0 (owner
decision, 2026-10-28, see *Gate: product*). Also no telemetry or
crash-reporting service. Instead, a local *Export diagnostics* bundle that the user
chooses to attach to a bug report covers the need.

- [ ] **Shared eBay keyset and hosted OAuth broker** (1.0)
- [ ] **iroh relay**
- [ ] **Update manifest**
- [ ] **Plugin registry**

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

PR #5 stopped `Db::open` deleting the WAL after a turso panic. Export/import is lossless:
export never contained secrets (verified 2026-10-19), and import no longer drops variant
fields or revives soft-deleted mappings. The config sections that did nothing are removed.
Saving config no longer persists environment-sourced secrets, and writes the file
atomically with `0600` permissions. Untrusted marketplace and plugin HTML no longer runs
with full IPC access. No command returns a secret to any webview: stored secrets come back
masked and the company secret is only shown natively (N-2.7), so Settings lives in the main
window. *Gate: security* ticks strict CSP and sanitised `v-html`.

Money is now exact: prices are `rust_decimal::Decimal` end to end, stored as decimal text
by migration 024, so a computed `0.1 + 0.2` goes out as `0.3` (N-6.1). Plugin prices are
exact decimals at the boundary (N-7.2). A scheduled `cargo update` / `cargo audit` CI job
exists, and `config.example.toml` points at the app's Products pages instead of the TUI.

- [ ] Secret handling audit
- [ ] Swallowed errors
- [ ] Structured logging levels that are useful in the shipped app, not just `tracing` defaults
- [ ] Nuxt 4 migration
- [ ] Dependency freshness pass
- [ ] `proc-macro-error2 v2.0.1` is flagged future-incompatible by cargo (`cargo report future-incompatibilities`)
