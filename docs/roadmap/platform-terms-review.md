# Platform terms review

Gate: legal and platform terms — N-12.4, reviewed 2026-10-23.

Nisaba syncs the operator's own catalog to marketplaces through **the
operator's own developer credentials** (no shared Nisaba keyset, no hosted
service). Each platform below was reviewed against what Nisaba actually does
today: listings/inventory/pricing sync only — no order APIs with buyer
details are read (see [`PRIVACY.md`](../../PRIVACY.md)). Each note lists the
terms checked, where Nisaba stands, and any required behaviour changes filed
as follow-ups.

Review date: **2026-10-23**. Terms change; re-run this review whenever an
adapter is extended to new APIs or data types.

---

## 1. eBay — APIs License Agreement

Reviewed: 2026-10-23. Source: eBay Developers Program,
[API License Agreement](https://developer.ebay.com/join/api-license-agreement).

Nisaba's eBay adapter (`crates/platform-ebay`) uses OAuth 2.0 with the
scopes `https://api.ebay.com/oauth/api_scope`,
`.../oauth/api_scope/sell.inventory`, and
`.../oauth/api_scope/sell.account.readonly` — inventory and account
metadata only. It does **not** request `sell.fulfillment` (orders), buyer
data, or the Marketing/Analytics APIs.

Findings:

- **Marketplace Account Deletion/Closure notifications.** eBay requires every
  production keyset either to subscribe an HTTPS endpoint that deletes an
  eBay user's data on request, or to obtain an exemption. Because operators
  bring their own keys, each operator handles their own keyset. Status:
  **done** — confirmed against eBay's current developer docs in N-12.1 and
  shipped as a walkthrough in the in-app guide
  (`frontend/public/guide/ebay-deletion.html`). This satisfies the "confirm
  the current rule" follow-up; re-confirm if eBay changes the policy.
- **Termination / data destruction (§14–16).** On termination Nisaba's
  operator must destroy all eBay content and personal information. Since the
  adapter only stores the operator's own listings and inventory snapshots,
  and no buyer data is ever read, the exposure is the operator's own data.
  No behaviour change needed.
- **Rate limits / restricted APIs.** Nisaba stays on standard unrestricted
  seller APIs and its inventory scopes; an Application Growth Check would
  only be needed for restricted APIs or higher call limits, which are not
  requested. No change.
- **28 CFR Part 202 (country-of-concern) data brokerage provisions.** Nisaba
  is not a data broker: eBay data never leaves the operator's machine except
  to eBay itself, and is never shared with third parties. No change.

Follow-ups: none new. (Re-run if an order scope is ever added — that would
bring in buyer PII handling obligations and the deletion-notification path
would need to cover it.)

---

## 2. Amazon — SP-API Acceptable Use Policy (AUP) and Data Protection Policy (DPP)

Reviewed: 2026-10-23. Sources: Amazon SP-API
[Acceptable Use Policy](https://docs.developer.amazonservices.com/en_US/dev_guide/DG_AcceptableUsePolicy.html)
and
[Data Protection Policy](https://docs.developer.amazonservices.com/en_UK/dev_guide/DG_DataProtectionPolicy.html).

Nisaba's Amazon adapter (`crates/platform-amazon`) authenticates with LWA
OAuth and calls the **Listings Items API** (fetch) and the **Feeds API**
(quantity/price updates). It does **not** call the Orders API, the Reports
API with restricted report types, or any endpoint requiring a Restricted
Data Token (RDT).

Findings:

- **AUP.** Nisaba uses the official APIs (no screen scraping), the
  operator's own registered application, and the marketplace endpoints for
  their documented purpose (managing the operator's own listings). No
  prohibited use. No change.
- **DPP — not triggered.** The DPP's strict rules (30-day PII retention cap
  after order delivery, encryption in transit/at rest for PII, deletion or
  return within the required window, data attribution tagging) apply to
  **Information / PII received from Amazon**. Nisaba receives no buyer PII
  from Amazon today — no Orders API calls, no RDT requests — so these
  obligations do not attach. Listing data the operator *sends* to Amazon
  (their own SKUs, prices, quantities) is their own data under their own
  seller agreement.
- **Rate limits.** SP-API endpoints are rate-limited per operation; the
  adapter should respect `x-amzn-RateLimit-*` headers and back off on 429s.

Follow-ups (recorded requirements):

1. **If the Orders API (or any buyer-PII endpoint) is ever added:** a full
   DPP re-review is required *before* merge — retention caps (30 days),
   deletion-on-request, encryption, and data attribution must be designed
   in, and [`PRIVACY.md`](../../PRIVACY.md) must be updated. This is the
   "relevant as soon as order data with buyer details is read" condition
   from the roadmap gate.
2. **Rate-limit handling audit for the Amazon adapter:** verify the sync
   engine's Amazon path honours `x-amzn-RateLimit-*` headers and 429
   backoff rather than retrying blindly. Filed as a post-N-12.4 follow-up
   for the sync-engine task list. (Non-blocking for 1.0: the adapter polls
   on a 5-minute cycle and does not hammer endpoints.)

---

## 3. Squarespace — API terms

Reviewed: 2026-10-23. Source: Squarespace Developer Platform
([Commerce APIs](https://developers.squarespace.com/commerce-apis/making-requests),
[authentication & permissions](https://developers.squarespace.com/commerce-apis/authentication-and-permissions)).

Nisaba's Squarespace adapter (`crates/platform-squarespace`) calls the
Commerce APIs — **Inventory** and **Products** only (`/1.0/commerce/inventory`,
`/1.0/commerce/products`, `/1.0/commerce/store_pages`) — with an API key the
operator generates in their own Squarespace site (Settings → Advanced →
Developer API Keys). It does **not** use the Orders, Contacts/Profiles,
Transactions, or Analytics APIs, so no customer/order data is read.

Findings:

- Nisaba's usage is the documented purpose of the Commerce APIs (custom
  applications on Commerce Advanced/Premium plans). Requests are HTTPS-only
  (HTTP is rejected by the platform), the key is stored in the OS keyring,
  and it is never shared. No change.
- API keys are **per-site** and non-expiring; the guide already documents
  generating a key per site. The operator accepts Squarespace's terms when
  generating the key. No change.
- Squarespace's developer docs require a descriptive `User-Agent` on API
  requests.

Follow-ups:

1. **Descriptive User-Agent on Squarespace requests** (`N-12.4 follow-up`):
   verify the adapter sets a custom `User-Agent` (e.g. `nisaba/<version>`)
   rather than the reqwest default, per Squarespace's documented
   expectation. Filed as a non-blocking follow-up for the adapter's
   next pass.

---

## 4. XMR Bazaar — terms on automated access

Reviewed: 2026-10-23. Source: XMR Bazaar
[Terms](https://xmrbazaar.com/terms/) (fetched and read on the review date).

Nisaba's XMR Bazaar adapter (`crates/platform-xmrbazaar`) has no official
API — it logs into the operator's own account over HTTPS and reads/submits
the site's own HTML forms (listing pages, inventory fields, the
`/my-sales/` page for sale detection), with CSRF tokens and session
cookies like a browser would.

Findings:

- **No explicit automation/scraping clause.** The terms do not contain a
  "no bots / no automated access" clause. The operative rules are:
  (a) **no harm to users or the platform** — no spam, scam, phishing, hack,
  or other inconvenience, with bans for offenders; and
  (b) **no unauthorized security testing** — no probing, scanning, or
  vulnerability testing (the adapter does none of this; it uses normal
  authenticated form submissions).
- **What Nisaba does is within the terms as written:** it accesses only the
  **operator's own account** — the operator's own listings, own inventory,
  own sale records (order id, listing, quantity, status — never other users'
  data) — at human-scale frequency (the sync engine's 5-minute cycle), with
  no credential sharing and no bypass of access controls. It does not
  scrape other users' profiles, listings, or reviews.
- **Residual risk (accepted, disclosed):** the terms do not affirmatively
  authorize automation. XmrBazaar may change its terms, its HTML (which
  breaks the adapter), or ban accounts it considers abusive. Mitigations in
  place: own-account-only access, sync cadence, no bulk crawling. The
  in-app guide should state this honestly rather than promise continued
  compatibility.

Follow-ups:

1. **Document the residual risk in the guide** (credentials/XMR Bazaar
   section): automation is not expressly authorized; a terms or layout
   change can break the adapter or get the account flagged, and the
   operator accepts that risk by connecting the adapter. Filed as a
   non-blocking docs follow-up.
2. **Re-review if XMR Bazaar adds an explicit anti-automation clause** —
   the adapter would need to switch to any official API they offer, or
   the adapter gets marked deprecated.

---

## Cross-platform behaviour rules this review establishes

These are now part of the 1.0 contract, not suggestions:

1. **No order/buyer data in 1.0.** No adapter reads buyer PII today. Any
   adapter change that would read order or buyer data requires a fresh
   terms review and a `PRIVACY.md` update *before* it ships.
2. **Own credentials only.** Nisaba never ships a shared keyset; operators
   bring their own and accept each platform's terms when they do.
3. **Rate-respecting automation.** Adapters must honour documented rate
   limits and back off on 429s rather than retrying blindly (see the
   Amazon and Squarespace follow-ups above).
4. **Re-review triggers:** a new API surface, new data types (orders,
   customers), a platform policy change, or a terms page that gains an
   anti-automation clause — each re-opens this review for that platform.
