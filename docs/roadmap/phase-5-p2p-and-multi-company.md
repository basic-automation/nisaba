# Phase 5 — P2P and multi-company

[← Back to the checklist](../../ROADMAP.md#phase-5--p2p-and-multi-company)

## Tor onion service, peer client/server and company sync payloads

(An earlier wording here said "encrypted" payloads: they carry no application-layer encryption, only Tor's transport encryption — see the sealing item below.)

## Peers never converged, and could overwrite each other's edits

Merges are last-writer-wins on `updated_at`, but the merge wrote variants with `insert_variant`/ `update_variant`, which stamp `now`, and then ran `recalc_product_quantity`, which re-stamped every variant and the product even when nothing changed. So each receiving peer's copy looked newer than the sender's, flowed back on the next sync, and the two rewrote each other every cycle, forever — and a stale copy carrying a fresh stamp could beat a genuine edit (an on-hand stock change) made on the other peer between payload load and merge. Fixed by `Db::upsert_variant_from_peer`, which keeps the edit's timestamp, and by `recalc_product_quantity` writing only rows whose quantity actually changes. `two_peers_converge_after_syncing_both_ways` reproduced it (every round rewrote the product and its variant) and now converges after the first sync.

## Dropship stock did not survive P2P

A variant's effective quantity is on-hand plus vendor stock when its source plugin has "include vendor stock" on, but the variant's source-plugin link was not in the payload (and the merge wrote `None`), so a receiving peer could never attach vendor stock to it and counted on-hand only. The link now travels (`SyncableVariant::source_plugin_id`/`source_vendor_item_id`, defaulted so older peers still parse, and a peer that sends none does not erase a local link); each peer's own vendor sync supplies the vendor quantity, and peers holding the same plugin data agree and converge.

## Vendor stock is still per peer

A peer that has not installed and synced the source plugin counts that variant's on-hand stock only, and the peers disagree until it does. Decide whether to sync `product_vendor_data` itself, or require every peer that runs platform sync to have the company's vendor plugins installed.

## The sync payload carries platform auth tokens and plugin configuration protected only by Tor's transport encryption

Decide whether that is enough, or whether sensitive sections should be sealed with a key derived from the company secret as the at-rest company config already is. The iroh move below changes the question: QUIC to an endpoint id proven in the handshake is a stronger channel than "anyone who holds the secret", so revisit this after the transport lands rather than before.

## `[company].enabled` is never read

`CompanyConfig::enabled` is parsed, but `init_company_context` starts the Tor onion service and periodic P2P sync for every company unconditionally, so `enabled = false` (the default in `config.example.toml`) does not keep an install off Tor. Honouring it would stop P2P for anyone who relies on today's behaviour without setting the flag, so decide first: gate on it (and set it where it matters), or drop the setting. The README no longer claims it works.

## Peer trust model

Documented in the README's P2P section, from `server.rs` and `sync.rs`: the company secret is the only credential; a secret holder receives everything (tokens included) and anything it sends is merged. The items below are what that reading found.

## Authenticate peers individually

`/api/sync` authorizes by the `sender_peer_id` / `sender_onion` in the request body — self-asserted, so any secret holder can speak as any authorized peer, and a sync under another peer's id rewrites that peer's onion address to the sender's (`update_peer_onion_address`). `/api/address-update` repoints any peer with no check that the caller is that peer. Needs a per-peer credential: Tor onion-service client authorization, or requests signed with a per-peer key. **Resolved by the iroh transport below** — each peer's Ed25519 endpoint id is proven in the QUIC handshake, so authorization keys off the connection, not the body.

## Enforce roles on the caller

`check_admin` checks the receiving node's own role, so on an admin's node every secret holder can add, approve and remove peers and change roles. Depends on the item above — a caller's role means nothing until the caller is authenticated.

## Bound peer-supplied timestamps

Last-writer-wins compared the sender's `updated_at` verbatim, so a row stamped `9999-12-31` — or anything sorting after a digit, like `"Z"` — won every later merge and could never be corrected. Every merged section now refuses a row whose timestamp does not parse (SQLite or RFC 3339) or is more than 10 minutes ahead of the receiver's clock, and counts it in `MergeSummary::rejected_timestamps`.

## Consider a hybrid/logical clock for merges instead of wall time

Even bounded, last-writer-wins on wall clocks lets a peer with a fast clock win ties it should lose. Planned as part of Sync v2, Step 2 (per-field registers on a hybrid logical clock).

## Auth tokens ping-ponged like variants did

`upsert_auth_token` stamped `now` on receipt, so a received token flowed straight back re-stamped, and a stale copy could overwrite a token the other peer had just refreshed — an auth failure on the next sync. Merges now use `upsert_auth_token_from_peer`, which keeps the edit's timestamp.

## Key rotation for the company secret, with a migration path for existing peers

`/api/rotate-secret` exists but is a stub that only acknowledges the request. Under iroh this becomes per-peer revocation (drop an endpoint id from the allow list), and the shared secret only seals company config at rest — see below.

## Conflict resolution for P2P merges was `merge_remote_payload`'s implicit policy

It is now written down per section in the README and pinned by `tests/round_trip.rs` (newer-wins both ways, tombstones, idempotence, two-way convergence) — which is how the convergence bug above was found.

## Bounded payload sizes

A full sync payload currently grows with the whole catalog. Answered by Sync v2's op log and compaction (Step 4 below), which also covers the offline/rejoin item above.

## Today's P2P (Tor) — what the 2026-10-19 review found

On the PR #2 branch, P2P company sync is **not functional end to end**: the join flow
cannot produce a syncing pair, and platform-sync coordination is never invoked. Sync v2
replaces this transport. Fix the data-loss and secret-loss items now, have the README say
P2P is experimental, and do not repair the Tor join path.

### Leaving a legacy-migrated company deletes the app database

At startup the legacy company is registered with `config.database.path` (`main.rs` ~150–151), which is the app registry database. Leaving the company then runs `remove_file` on that path (`commands/company.rs` ~327–330). Fixed in PR #5.

### The legacy company secret can be lost

Startup replaces the stored secret with its hash even when saving it to the keyring failed (`main.rs` ~143–148). Fixed in PR #5.

### Join cannot produce a syncing pair

`join_company` appends `.onion` to a name that onyums already suffixes (`company.rs` ~247), so the admin stores `x.onion.onion`. The member stores the admin only in `company_peers_v2`, under a random peer id, while `sync_all_peers` reads the legacy `company_peers` table (`p2p/src/lib.rs` ~126). The member therefore never starts a sync, and `handle_sync`'s lookups miss for the admin.

### Platform-sync round-robin is dead code

`is_our_turn_to_sync` and `record_platform_sync_and_push` have no callers. Every peer pushes to the marketplaces on its own timer, with credentials shared over P2P, and XMR order dedup converges only at the P2P interval, so a sale can be processed twice. Sync v2 needs a single writer per platform regardless (Step 2).

### One onion identity for all companies, tied to the launch directory

Every company calls `onyums::serve(router, "nisaba")`. onyums keeps one global `ONION_NAME` and state under `./tor/onyums/state` (artiqwest under `./tor/arti`), so a second company saves the first company's address as its own. A packaged build launched from `/` or Program Files may be unable to write the state, and a different launch directory means a new address.

### Hard 2 MiB request limit and no client timeout

`handle_sync` takes `Json<SyncRequest>` under axum's default 2 MiB body limit, so once a catalog's payload outgrows it, every sync fails with 413. artiqwest sets no timeout, so one hung peer stalls the sequential peer loop.

### The merge is not transactional

Each row was applied with `?`, so the first error left earlier sections applied and skipped the rest (config, tokens, plugins, variants), and the same payload failed the same way every cycle. `merge_remote_payload` now plans every write from the local state first, then applies the batch in a single transaction (`Db::execute_write_batch`): any error rolls the whole merge back and leaves the database unchanged. Pinned by `a_failing_merge_leaves_the_database_unchanged` in `tests/round_trip.rs`.

### No onion-service lifecycle

`start_onion_service` returns before Tor has bootstrapped, with no retry. `Stop` ends only the sync loop, so a company that was left keeps serving its database. Companies created or joined at runtime drop sync and P2P events and get no vendor-sync hook until restart.

### Product and variant deletes orphaned rows in `listing_descriptions`, `listing_photos`, `pricing_snapshots` and `cached_listings`

`delete_product` now removes those tables too (with the dependents it already removed) in its transaction, and `delete_variant`/`delete_variants_for_product` also remove `product_vendor_data`, so a delete leaves no rows pointing at a product or variant that no longer exists.

### Product and variant deletes are hard deletes (`db.rs` `delete_product`, `delete_variant`), so a merge re-inserts them from any peer that still has them

Sync v2's observed-remove sets cover this half.

### README still says "AES-GCM encrypted payloads" (`README.md:27`)

Payloads have no application-layer encryption.

## Sync v2: CRDT replication over iroh

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

### Step 0: decisions before code

#### Privacy model

Tor hides each peer's IP address from the other peers and from network observers; iroh does not. Direct connections show IPs to peers. Relays and discovery see endpoint ids, IPs and connection timing, though not content, which is end-to-end encrypted. If you keep the defaults, those relays and discovery are n0's public infrastructure. For a tool whose users sell on XMR Bazaar, that is a real downgrade. Options: (a) iroh only, with a self-hosted `iroh-relay` and peers found through invite tickets instead of public discovery; (b) iroh by default, with Tor kept as a per-company opt-in transport (keeps `onyums`/`artiqwest` and their maintenance); (c) iroh on n0's public relays. Record the choice in the README's P2P section and in `SECURITY.md`.

#### CRDT design doc

(`docs/sync-v2.md`) covering every synced section of `SyncPayload`: which CRDT each field uses, how stock works, and what stays outside the CRDT (below). Code starts after it is agreed.

#### Engine spike

Build one product with variants, mappings and stock events in each candidate. Judge them on: delta export since a version vector; compaction (garbage collection of history); schema evolution; queryability from the Tauri commands, which all read SQLite today; and memory at catalog scale.

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

### Step 1: transport — today's protocol over iroh

Do this first. It gives per-peer authentication on its own, independent of the CRDT design.

#### Persist each install's iroh secret key in the OS keyring, next to the company secret

Losing it means a new identity, so export/import and moving to a new machine must carry it, or document that the peer re-joins.

#### Use a `nisaba/sync/1` ALPN with an `iroh::protocol::Router` handler in place of the axum router

One bidirectional stream per request, with length-prefixed JSON for `SyncRequest`, join, ping, company info and peer administration. Leave `sync.rs` untouched so `tests/round_trip.rs` keeps pinning the merge while only the wire changes.

#### Authorize by the connection's remote endpoint id

Add an `endpoint_id` column to the peers table (a migration) and drop `sender_peer_id`/`sender_onion` from the request types.

#### Join by ticket

The admin issues an invite with its endpoint address and a one-time join token, shared by copy-paste or QR; approval binds the joiner's endpoint id. The company secret then never crosses the wire and stops being a bearer credential.

#### Enforce roles on the caller's endpoint id

This replaces `check_admin`'s check of the node's own role.

#### Revocation instead of rotation

Removing a peer drops its endpoint id and takes effect on its next connection; retire the `/api/rotate-secret` stub.

#### Transport tests on loopback endpoints with relays off

Join by ticket, sync both ways, and refuse an unknown endpoint id, a revoked peer, and a non-admin caller on admin routes. These replace "the Tor transport is untested" in Phase 1.

### Step 2: the CRDT data layer

#### Scalar fields

Name, SKU, threshold, image, sort order, attributes, company config, logo — become **per-field last-writer-wins registers** stamped with a **hybrid logical clock** plus the endpoint id as tie-breaker. This replaces whole-row, wall-clock comparison and closes the hybrid-clock item above. Keep a bound on how far ahead a peer's physical clock may be, as `plausible_timestamp` does today.

#### Collections

A product's variants, platform mappings, vendor plugins and the peer list — become **add-wins observed-remove sets** keyed by stable UUIDs. A delete that races an edit then resolves the same way on every peer, and tombstones can be compacted once every peer has seen them.

#### Stock is not a register

Model it as a grow-only set of uniquely identified stock events, with quantity = the sum of the events:

- a sale, keyed by platform + order id, so two peers that detect the same sale count
  it once (this generalizes today's `xmr_processed_orders` dedup table);
- a restock;
- a manual adjustment, keyed by UUID.

A manual "set on-hand to N" is recorded as an adjustment computed against the
writer's current view. Two concurrent sets therefore both apply, so surface the pair
for review instead of summing them silently.
This is the same ledger Phase 4 builds on a single machine (*Stock correctness*);
Sync v2 replicates it rather than inventing a second model.

#### Marketplace writes stay outside the CRDT

Replicas converge on local state, not on what eBay or Squarespace hold. Pushes stay with the designated peer and must be idempotent. `platform_snapshots` and the version tags become per-peer local state. If peers need to see them, replicate them as observations, never as commands.

#### Secrets stay out of the replicated log in plaintext

That covers auth tokens, plugin `secret: true` fields and the iroh key. Seal them with a company key, or distribute them out of band. This closes the sealing item above and the plugin-secrets item in Phase 3.

#### Sign every op with its author's endpoint key, and validate it before applying

The signature, the author's role at that point, the schema, and value bounds. Because ops are attributed, revoking a compromised peer can also undo its ops since a chosen point.

### Step 3: oversell protection — escrow over the stock CRDT

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

#### Rights state per variant

A matrix of `rights[from][to]` transfers plus each peer's spent count. Both only ever grow, so the matrix is itself a CRDT. A peer's available rights are what it was given, minus what it gave away, minus what it spent. Stock remains the sum of stock events (Step 2); rights only limit who may *decrement* it.

#### Gate every decrement on available rights

A detected sale, a manual reduction, a reservation. A restock adds rights to the peer that records it. A sale detected on a marketplace has already happened, so it is recorded regardless. If it overdraws the detecting peer's rights, it is an **oversell incident**: logged, surfaced in the UI, and triggering the zero-stock push to the other platforms immediately rather than at the next cycle.

#### Allocation policy for each peer's share

The starting point is to give the designated peer, which runs platform sync and so sees every marketplace sale, all the rights, and other peers request what they need for manual edits. A later step can give each platform's poller a share proportional to that platform's recent sales velocity.

#### Rights transfer

A peer short of rights asks peers with spare rights over the iroh connection. A transfer is just another op, so it replicates and stays idempotent. Define what the UI shows while a request is outstanding, and the timeout after which an edit is refused as "not enough stock rights on this machine, others offline".

#### Rebalancing

Periodically move rights toward the peers that use them, and pull rights back from a peer that has been offline longer than a threshold. Rights held by an offline peer are stock that nobody else can sell.

#### Revocation and lost keys

When a peer is revoked or its iroh key is lost, its unspent rights must return to the pool. Only an admin can reclaim them, recorded as a signed op. Without this, revoking a peer quietly shrinks sellable stock.

#### Fold Phase 4's "oversell protection: a configurable reserve buffer" into this

A reserve is a share of rights held back from every peer and released by an admin.

#### Tests

Property tests in which concurrent decrements across N partitioned peers never take stock below zero under any delivery order; rights stay conserved, meaning the sum of available rights plus the sum of spent equals the sum of stock events; and an overdraw from a marketplace sale is reported as an incident and never absorbed silently.

### Step 4: replication protocol

#### A per-author op log with sequence numbers

A sync exchanges version vectors and sends only the ops the other side lacks, so payloads are bounded by the difference instead of the catalog. For state-based reconciliation, range-based set reconciliation (`negentropy` 0.5, the approach `iroh-docs` uses) is the reference design.

#### Compaction

Ops older than the causal-stability frontier, meaning seen by every active peer, fold into a snapshot. A peer away longer than retention receives the snapshot plus newer ops. This closes the offline/rejoin and bounded-payload items above.

#### Push on change

`iroh-gossip` announces new ops to online peers, who then pull, and a periodic anti-entropy sync covers missed announcements.

#### Schema evolution

Version the op format, have peers advertise the versions they support, and make an unknown field round-trip instead of dropping it.

### Step 5: verification and migration

#### Property-based convergence tests with `proptest`

N replicas, random ops, and random delivery order, duplication, loss and redelivery. Every replica must end in the same state, stock must equal the sum of its events, and no delete may resurface. Add a deterministic simulation of partitions and rejoins. Port the scenarios in `tests/round_trip.rs`.

#### Migration

One peer turns the existing database into a genesis snapshot under protocol v2; the other peers re-join from it by ticket. Once no supported release speaks v1, remove the full-payload last-writer-wins path (`load_full_sync_payload` and `merge_remote_payload`). Unless option (b) was chosen, also remove `onyums`, `artiqwest` and the Tor startup path, re-run `cargo report future-incompatibilities`, and close the `proc-macro-error2` item.
