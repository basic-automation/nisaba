# Services and integrations

[← Back to the checklist](../../ROADMAP.md#services-and-integrations)

Two kinds of "service" matter here: hosted infrastructure that Nisaba itself would have to
run, and external platforms that Nisaba syncs with. Nisaba is a desktop app that never
holds a customer's keys anywhere but their own machine, so the bar for the first kind is
high: every hosted service is an availability dependency, a privacy promise and a running
cost.

## Hosted infrastructure

### No OAuth broker, except for eBay

Customers bring their own API keys for Amazon, Squarespace and XMR Bazaar (see *1.0 release*, owner decision 2026-10-19, after Nisaba's own Amazon SP-API application was denied). eBay is the exception: see the next section (owner decision 2026-10-28).

### **Shared eBay keyset and hosted OAuth broker** (1.0)

Owner decision 2026-10-28: ship one-click eBay sign-in in 1.0. Nothing here is built yet.

**Why a broker.** eBay's authorize URL needs the app's client ID, and the code-for-token exchange and every refresh authenticate with the client secret. Nisaba is open source and a desktop app, so a secret inside it would leak and eBay could revoke the keyset for every user. A small stateless service holds the secret instead. There is no password login option: eBay's API accepts no username and password.

**Design.** The app opens eBay's login. eBay redirects to the broker, which exchanges the code with Nisaba's secret and hands the tokens to the app; refreshes go through it too. The broker stores nothing and logs no tokens. It issues and checks a `state` per attempt, allows only the app's redirect, and the tokens live in the user's own keyring as they do today. The "Authorize with eBay" button becomes the default and the client ID, client secret and RuName fields move behind "Use my own keys".

**Prerequisites only the owner can do.**
1. Register Nisaba in the eBay Developers Program and obtain a production keyset and a RuName.
2. Choose where the broker runs and under which domain.
3. Subscribe to eBay's account deletion notifications, or obtain the exemption for not storing eBay user data (a stateless broker likely qualifies; confirm with eBay).

**Open questions.** (a) Whether eBay's token endpoint accepts PKCE without a client secret; searches found no confirmation and the working assumption is that it does not, which is why the design uses a broker. (b) How long production keyset approval takes. (c) Whether eBay's terms allow one keyset to serve many unrelated sellers, which the platform terms review must confirm before launch.

**Not in scope.** Amazon stays bring-your-own, because Nisaba's own SP-API application was denied.

### iroh relay

Only if Sync v2 picks option (a) in Step 0. Host one relay for customers who want it, and document running your own; `iroh-relay` is a single binary.

### Update manifest

GitHub Releases, with no new service (Phase 6). Checks can be turned off (Phase 6).

### Plugin registry

No hosted registry for 1.0. Plugins are imported from a file or URL, and shared within a company. Revisit with plugin signing (Phase 3).

### No telemetry or crash-reporting service

A local *Export diagnostics* bundle that the user chooses to attach to a bug report covers the need without data leaving the machine unasked (*1.0 release*, Gate: product).

## Marketplace and storefront integrations

Shipping today: eBay, Squarespace, XMR Bazaar, and Amazon (Beta for 1.0). Every candidate
below uses keys or tokens the customer creates for their own account, so none needs a
broker. Which ones are worth building depends on two facts this list does not have: where
customers' sales actually come from, and whether they sell in person. Ask before building
past the *Should* group.

### Priority: WordPress and the WooCommerce ecosystem

Owner direction 2026-10-19: WordPress and its ecosystem come first. WooCommerce is the
next adapter, ahead of Shopify. It has four advantages over the rest of the list:

- it is self-hosted, so customers own their store and their data, which matches Nisaba's
  stance;
- it is common with sellers who take crypto, which matches the XMR Bazaar audience;
- it is the only candidate that CI can verify against a real store, using the official
  WordPress and WooCommerce Docker images;
- its official channel extensions can carry stock onward to Google, Meta and TikTok.

#### WooCommerce adapter

(as a marketplace plugin if the Phase 3 bridge is ready, otherwise as a Rust crate that later moves behind it):

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

#### Live verification in CI

Start `wordpress` with WooCommerce in a service container, seed products and variations, and run the adapter's full write path against it, including a rejected write. No other platform allows a real-API test in CI, so this adapter becomes the template for "writes report what really happened" (Phase 2).

#### Blind stock writes race checkout

A REST stock update is an absolute set, so an order placed between Nisaba's read and write is overwritten, losing that decrement. Every platform has this race; WooCommerce is the one where Nisaba can ship server-side code to close it. If polling cannot keep the window small enough, build the companion plugin below.

#### Companion WordPress plugin "Nisaba Connector"

(optional; build only if the race above or the REST API's limits demand it). It would provide an atomic adjust endpoint ("decrement by N") or a compare-and-set ("set to X if current is Y"), a bulk stock read keyed by SKU, and an order-change cursor. Authentication would use WordPress Application Passwords or the WooCommerce keys. Publish it on wordpress.org so customers install it from their dashboard.

#### Channel extensions instead of direct adapters

Google's, Meta's and TikTok's official WooCommerce extensions sync the store's catalog and availability to Merchant Center, Meta catalogs and TikTok Shop. For a WooCommerce seller, a correct WooCommerce stock count therefore reaches all three without a Nisaba adapter. Document the recommended setup, and verify that each extension pushes stock changes promptly and does not wait for a daily feed. Direct Merchant Center, Meta and TikTok adapters (below) are then for sellers without WooCommerce.

#### Crypto checkout guide

BTCPay Server for WooCommerce, and the Monero WooCommerce gateway, are the recommended way for XMR Bazaar sellers to take crypto on their own store. No integration is needed, because Nisaba sees WooCommerce orders whatever the payment method; this is a setup guide.

#### WordPress.com-hosted WooCommerce

Confirm that the REST API and its keys work on WordPress.com's commerce plans, so customers who don't want to self-host are covered.

#### Move a catalog into WooCommerce

Use Nisaba's cross-platform create-listing to copy a Squarespace or eBay catalog into a new WooCommerce store, with mappings created as it goes. This turns "set up a WordPress store" from a migration project into a button.

#### *Could:* WordPress multi-vendor marketplaces such as Dokan, WCFM and WC Vendors, for sellers who list on someone else's WordPress-based marketplace

Dokan exposes a REST API for vendors. Build one when a customer sells on one.

- *Not building:* Easy Digital Downloads and other WordPress digital-goods plugins. They
  sell downloads, so there is no physical stock to sync.

### Should — after WordPress, in recommended order

#### Shopify

The largest small-business storefront. The GraphQL Admin API has first-class inventory levels per location. A custom app created in the store's admin gives an access token with no app review.

#### Google Merchant Center

A direct adapter for sellers whose store is *not* WooCommerce (Squarespace today); WooCommerce sellers are covered by Google's extension (above). Merchant Center disapproves items whose availability does not match the store page, so accurate stock is exactly what it needs. Build on the new Merchant API, not the Content API for Shopping it replaces; check the Content API's sunset date first.

#### Square POS — if customers sell in person

(shows, a shop). A sale at a table is an oversell Nisaba cannot see today. Square issues a personal access token for the seller's own account, which fits bring-your-own keys. Square is unrelated to Squarespace.

#### Walmart Marketplace

A large marketplace, but sellers must be approved by Walmart first. Its API keys are per seller.

#### Build these as **marketplace plugins** rather than new Rust crates (Phase 3, *Plugin kinds after 1.0*)

A new marketplace then ships without an app release. WooCommerce, followed by Shopify, proves that the plugin bridge is general. This supersedes the "fifth adapter" item in Phase 2.

### Could — build when a customer actually sells there

#### BigCommerce

A storefront like Shopify, with store-level API tokens. It also has a WordPress plugin (BigCommerce for WordPress), so it overlaps with the WordPress priority above.

#### Meta (Facebook/Instagram) catalog

For sellers without WooCommerce, since WooCommerce sellers are covered by Meta's extension. Catalog and availability sync only: Meta moved US Shops checkout to sellers' own websites, so this channel sends traffic rather than taking sales. Check the current state of Shops checkout before building.

#### TikTok Shop

For sellers without WooCommerce, since WooCommerce sellers are covered by TikTok's extension. A growing channel, but it needs partner app approval, the same risk that sank Nisaba's own Amazon application (*1.0 release*).

#### BTCPay Server store and point-of-sale apps

A crypto-native storefront that tracks item inventory; the audience matches XMR Bazaar's. Confirm that the Greenfield API exposes item inventory for read and write.

#### GunBroker

Only if customers' catalogs lean toward tactical accessories. It has an API.

#### Other crypto-native marketplaces besides XMR Bazaar

Ask current customers which ones they sell on.

### Not building, and why

- **Mercari and Poshmark** — neither offers a public API for sellers, and automating
  their apps would break their terms.
- **Etsy** — its seller policy is built around handmade, vintage and craft supplies, so
  reselling a wholesale catalog such as Rothco's is a policy problem, not a technical one.
  Revisit only for a customer whose catalog qualifies.
- **Faire** — a marketplace for selling *to* retailers at wholesale; Nisaba's customers
  buy at wholesale and sell at retail.

## Ancillary integrations

### Should

#### A generic CSV/XML feed vendor plugin

, built in, with a screen that maps a supplier's columns to `VendorListing` fields and reads the feed over a URL or SFTP. Most wholesalers publish a feed rather than an API, so this covers suppliers without anyone writing a plugin. It depends on Phase 3's feed-supplier item (`parseCsv`, `parseXml`, the SFTP decision).

#### Off-site encrypted backups

, to a destination the customer chooses: an S3-compatible bucket (e.g. Backblaze B2) or a folder synced elsewhere (a NAS, Syncthing). Encrypt with a key the customer holds before anything leaves the machine. Local backups are a 1.0 gate (*Gate: data safety*); this is the step after.

#### Notifications

Low stock, an oversell incident, credentials about to expire, sync failing — to ntfy, email, Matrix, Slack or Discord (Phase 3, notification plugins).

### Could

#### UPC/product-data lookup

Fill in brand, MPN, images and dimensions when creating a listing from a bare UPC. These are paid APIs with per-customer keys.

#### Shipping labels

(Shippo, EasyPost, ShipStation) and **accounting** (QuickBooks, Xero) — these would turn Nisaba into an order-management tool, which is a product decision more than an integration. If accounting is wanted, start with a CSV export of sales and stock valuation rather than an API integration; QuickBooks and Xero both require app review for production access, the hurdle the Amazon application failed.
