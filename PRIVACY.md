# Privacy Statement

Nisaba is a desktop inventory-sync manager. It keeps one canonical product
catalog on your own machine and synchronizes quantities and listings with the
marketplaces you sell on. **There is no Nisaba cloud account, no Nisaba
server that receives your data, no telemetry, and no crash-reporting
service.** This page says exactly what leaves your machine, and what never
does.

> Shipped with the app (in-app user guide → Privacy) and reviewed per
> platform in [`docs/roadmap/platform-terms-review.md`](docs/roadmap/platform-terms-review.md).
> Reviewed 2026-10-23 (N-12.4). If the app changes what it sends or stores,
> this statement changes with it.

## What leaves the machine

### 1. Marketplace API calls — only to platforms you connected

When you connect a platform and run a sync cycle, Nisaba sends requests to
that platform's servers. The data sent is your own catalog and listing data
(products, variants, prices, quantities, photos), and it goes **only to the
platform it belongs to**. Marketplace requests never pass through any
Nisaba-operated infrastructure — there isn't any.

What each adapter sends:

| Platform | Transport | What is sent |
|---|---|---|
| eBay | HTTPS to `api.ebay.com` (OAuth 2.0) | Your listings, offers, inventory quantities, category/business-policy lookups. OAuth scopes are `sell.inventory` and `sell.account.readonly` — Nisaba does not request order or buyer data. |
| Amazon | HTTPS to SP-API endpoints (LWA OAuth 2.0) | Your listings (Listings Items API) and quantity/price updates (Feeds API). Nisaba does not call the Orders API and never requests restricted PII data. |
| Squarespace | HTTPS to `api.squarespace.com` (your own API key) | Your products and inventory adjustments. |
| XMR Bazaar | HTTPS to your configured base URL (`https://xmrbazaar.com` by default) | Your own listings, inventory fields, and sales-page reads — exactly the pages your own logged-in account would open in a browser. |

Your credentials are stored in the **OS keyring** (Windows Credential Manager,
macOS Keychain, Linux Secret Service) and are sent only to that platform's own
login/token endpoints. They are never sent to any third party.

### 2. Update checks — GitHub, opt-out available

The app checks for updates at launch and then every 6 hours. Each check is a
single HTTPS request to the GitHub-hosted release feed configured in the app
(currently `https://github.com/basic-automation/nisaba/releases/latest/download/latest.json`).
GitHub sees what any HTTPS request reveals: your IP address, the app version,
and the OS. Nothing about your catalog, credentials, or sync activity is in
the request.

You can turn automatic checks off in **Settings** (the `update_check_enabled`
preference). A manual "Check for updates" is your own explicit action and is
not gated by the setting.

### 3. P2P company sync — only if you enable it, over Tor

Peer-to-peer sync between your own installs is **off by default**. When you
enable it:

- Traffic runs over Tor onion services. Tor hides each peer's IP from the
  other peers and from network observers.
- Payloads are AES-GCM encrypted peer-to-peer; company secrets are held in
  the OS keyring.
- What observers of the network *can* see is what any onion-service traffic
  reveals: timing and circuit metadata, not content. (See the in-app guide's
  P2P section for the current architecture.)

### 4. Vendor plugins — only hosts you allow

Vendor plugins (e.g. a supplier catalog importer) run in a sandboxed Deno
runtime. A plugin may make network requests **only** through the host API's
fetch operation, which is gated by a declared `allowed_hosts` allowlist, and
private-network addresses are refused. You approve each plugin and its hosts
when you install it; a plugin cannot reach the network silently beyond what
it declared.

## What never leaves the machine

- **No telemetry, no analytics calls home.** The analytics features are
  computed locally from your own database. No usage statistics, no
  crash reports, no phone-home of any kind.
- **Your full catalog and local database** stay on your machine. They are
  only pushed, listing by listing, to the marketplaces you configured.
- **API keys and OAuth tokens** live in the OS keyring and travel only to
  their own platform's endpoints.
- **Order and buyer data are not read at all.** As of 1.0, no adapter
  requests order APIs with buyer details (no eBay `sell.fulfillment` scope,
  no Amazon Orders API, no Squarespace Orders/Profiles access, and the
  XMR Bazaar sale-detection pass reads only your own sale records —
  order id, listing, quantity, status — not other users' data). Because no
  buyer PII is collected, none is stored, transmitted, or shared anywhere.
  If a future version ever reads order data, this statement and the
  platform-terms review must be updated first — that is a recorded
  requirement in [`docs/roadmap/platform-terms-review.md`](docs/roadmap/platform-terms-review.md).

## Your obligations (because your keys are yours)

Nisaba runs on your own developer credentials, not a shared keyset. That
means **you** accept each platform's developer terms when you create the
keys (see the per-platform review in
[`docs/roadmap/platform-terms-review.md`](docs/roadmap/platform-terms-review.md)), and you
are responsible for keeping your machine's keyring and backups safe.

## Questions

Privacy questions about this statement go through the same private channel as
security reports — see [`SECURITY.md`](SECURITY.md).
