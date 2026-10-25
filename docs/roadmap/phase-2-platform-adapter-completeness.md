# Phase 2 — Platform adapter completeness

[← Back to the checklist](../../ROADMAP.md#phase-2--platform-adapter-completeness)

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

## Photo upload on eBay

Correcting an earlier reading of this: there is no dead implementation to verify or delete — `upload_photo` in `crates/platform-ebay/src/lib.rs` is a stub that returns an error explaining that the Sell Inventory API has no per-photo upload call, and `can_upload_photos = false` is therefore *accurate*. The real work is to set photos the way the Inventory API expects — the `imageUrls` array on the inventory item — and then decide whether `upload_photo` becomes a set-photos operation or the trait grows one. Correction (2026-10-19): photos at *create* time already work. `create_listing` sends `photo_urls` as `imageUrls` (`lib.rs` ~625), and the PUT paths keep existing images. What is missing is changing the photos of an existing listing.

## If eBay ever needs a true binary upload rather than `imageUrls`, it must go through the Media API (`createImageFromFile` / `createImageFromUrl`)

The Trading API's `UploadSiteHostedPictures` is decommissioned on 2026-09-30. Nisaba uses the Sell Inventory API throughout and so is not exposed to that deadline today. https://developer.ebay.com/updates/newsletter/q2_2025

## Photo upload for Squarespace and Amazon

No implementation at all. XMR Bazaar's `create_listing` already uploads up to 6 photos as multipart parts (`client.rs` ~505–610), so for XMR the gap is changing photos on an existing listing.

## Squarespace `set_price`

The only platform that cannot be repriced from Nisaba

## Amazon is `enabled = false` by default in `config.example.toml` and has never been exercised end-to-end

Run a real sandbox seller account through fetch → set quantity → set price, and mark the adapter verified or list what broke

## `detect_sales()` is only meaningful for XMR Bazaar's stock-mode inventory

Confirmed correct: `run_cycle` gates the call on `has_stock_mode_inventory`, so eBay, Squarespace and Amazon never reach the default impl at all — they are not silently reporting "no sales" each cycle. `detect_sales_is_only_called_on_stock_mode_platforms` holds the gate in place.

## Amazon counts only `fulfillmentAvailability[0]`, so a listing stocked on both merchant-fulfilled and FBA reports one channel's quantity

And Amazon does not guarantee that array's order. Because the sync engine resolves the minimum across platforms, a listing with most of its stock in FBA drags canonical stock down and pushes that number to eBay and Squarespace. Decide whether to sum the channels or filter to a configured one. Pinned by `only_the_first_fulfilment_channel_is_counted`. [N-5.4: quantity now sums across all fulfilment channels deterministically; the pinned test is replaced by `quantities_sum_across_fulfilment_channels` (13 for the cap).]

## Amazon takes `offers[0]` for price with no marketplace filtering, so a seller listed in more than one marketplace gets an arbitrary offer, and `to_listing` drops the currency

Filter on the configured `marketplace_ids`. [N-5.4: price is selected from the configured marketplace in configured order and `ListingPrice` keeps its currency.]

## Amazon drops ERROR-severity `issues` in every mapping, so a suppressed listing reaches the UI as a normal listing with zero stock and no explanation

Carry them through `extras` and surface them. [N-5.4: ERROR issues are carried through `FullListing.extras` (`amazon_error_issues` / count / per-issue entries); `fetch_listings` now requests `issues`.]

## eBay's two read paths key listings differently

The Inventory API by SKU, the Trading API by ItemID — so a mapping created from one will not match the other. Decide which is canonical, or store both.

## `trading_to_listing` computes `Quantity - QuantitySold` with no floor, so eBay reporting more sold than listed yields a negative quantity in the mapping UI and makes that…

…listing the minimum in any comparison. The sync engine floors canonical stock at zero, so nothing writes it back today. [N-5.6: floored at 0 in both `trading_to_listing` and `trading_to_full_listing` (`mapping.rs`), pinned by the updated mapping fixture test (2 listed, 5 sold -> 0).]

## Rate-limit handling per platform

eBay, Amazon and Squarespace all throttle, and the adapters have no backoff distinct from `max_retries`. Squarespace also rescans the whole catalog for every per-item read, which multiplies its request count.

## Token refresh failure path

What the UI shows when `refresh_auth()` fails mid-sync. Also warn *before* expiry. eBay's `refresh_token_expires_in` is parsed but unused (`platform-ebay/src/types.rs:11`), and Amazon LWA client secrets have a rotation deadline; check the current window before building this. [N-11.2: a failed `refresh_auth` mid-sync emits `AuthExpired`, which now surfaces as a persistent dashboard banner with a "Renew in setup wizard" deep-link (not a silent tooltip change); advance warnings come from the credential-health panel — eBay's refresh-token expiry is persisted from the token response, Amazon's client-secret rotation is tracked from `client_secret_updated_at`.]

## A fifth adapter would prove the trait is actually general

Superseded: the next adapter is WooCommerce, then Shopify, built as marketplace plugins (Phase 3, *Plugin kinds after 1.0*; *Services and integrations*).

## eBay OAuth sends no `state` parameter (`platform-ebay/src/auth.rs` ~54–60), and the callback accepts a `code` from any page the auth window reaches (~279–283)

Add and check `state`.

## Four `SyncError` variants are never constructed

`RateLimited`, `EbayRevisionLimit`, `ConflictError` and `PlatformDisabled` (`core/src/error.rs` ~12–34). In particular, nothing counts eBay's per-listing daily revision limit. A sync schedule that pushes often enough will hit it, and the failure looks like any other API error.

## Writes that report success without writing

Found by the 2026-10-19 review, on both `master` and PR #2 (which touches no adapter).
Line numbers are from that review. This is the highest-risk gap in the project: the sync
engine treats `Ok` as "the platform now holds this quantity". A silent no-op therefore
leaves a sold-out listing live on another marketplace, and nothing in the UI says so. Fix
these before adding new capabilities, and give every write path a test that a rejected
write surfaces as `Err`.

### eBay `set_quantity` swallows per-SKU errors

`bulk_update_price_quantity` (`platform-ebay/src/client.rs` ~177–195) logs each item error at `warn!` and returns `Ok(())`, and `statusCode` is parsed but never read (`types.rs:81`). Listings created on eBay's website are keyed by ItemID; `set_quantity` sends that ItemID as a SKU, so every push to such a listing no-ops behind an `Ok`. Fails with the "two read paths" item above.

### eBay `set_price` probably never changes the price

The `offers: [{price}]` entry carries no `offerId` (`client.rs:633`), and the 200/207 body is never parsed. When `availability` is absent, the same path falls back to quantity `0` (`lib.rs:454`), so a price edit can zero stock. Verify against the Inventory API docs and a sandbox.

### eBay read-modify-write erases fields it doesn't model

`EbayInventoryItem` (`types.rs` ~16–31) carries only title, description, imageUrls, aspects, condition and quantity. `createOrReplaceInventoryItem` is a full replace, so every description edit (`client.rs` ~406, ~756) drops brand, MPN, UPC, `packageWeightAndSize` and `conditionDescriptors`, and on the `set_description` path `conditionDescription` too. It also writes back a quantity read earlier, which may be stale by then. Model the item losslessly by carrying unknown fields through, or switch to a partial update. [N-5.2: `EbayInventoryItem`/`EbayProductInfo`/`EbayAvailability` now model brand/MPN/UPC/EAN/ISBN/ePID, `conditionDescription`, `conditionDescriptors` and `packageWeightAndSize` explicitly and carry every other field through `#[serde(flatten)]` extra maps; `build_inventory_item_put_body` rebuilds the full-replace body from the just-fetched item, applies only the caller's overrides, and omits quantity entirely so a stale read can never resurrect sold stock (fixture tests in `tests/read_modify_write.rs`).]

### eBay `update_listing` sends a partial body to `updateOffer`

, which is also a full replace (`lib.rs` ~543–568): it drops price and quantity, or is rejected. [N-5.2: `update_listing` now goes through `update_offer_fields` — fetch the complete offer, rebuild a complete body losslessly via `build_offer_put_body`, PUT that — and `update_offer` itself validates completeness and refuses a partial body before sending; a missing offer or a fetched offer lacking a required field is an explicit error, as is a tags edit eBay cannot represent.]

### Amazon PATCH bodies are double-wrapped, and failures report `Ok`

`AmazonPatch::value` is a `Vec<Value>`, and the client pushes `json!([{…}])` into it (`platform-amazon/src/client.rs` ~214, ~299), which serializes as `"value": [[{…}]]`. Even when Amazon marks the submission INVALID, `update_quantity` only warns (~252), `update_price` never reads the body, and `create_listing` discards the submission (`lib.rs` ~360). [N-5.3: PATCH bodies are now built by `build_quantity_patch_request` / `build_price_patch_request` with the attribute object directly in `value` (`"value": [{…}]`, pinned by serialisation tests); all three write paths parse the submission and pass it through `check_submission`, which returns `Err` on an `INVALID` status or any `ERROR`-severity issue — `update_price` now reads its response body and the adapter's `create_listing` checks the submission instead of discarding it. Fake-server tests prove a rejected write surfaces as `Err` for each of `update_quantity`, `update_price` and `create_listing`.]

### Amazon pagination may read the wrong key

`types.rs:17` expects a top-level `nextToken`. If `searchListingsItems` nests it under `pagination`, only the first 20 SKUs per marketplace are ever read. The hand-built fixture encodes the same assumption, which is the case for real captured fixtures (Phase 1). [N-5.4: the token is now read from the documented nested `pagination.nextToken` (with a legacy top-level fallback), pinned by the fixture's `pagination` block.]

### XMR Bazaar writes cannot tell success from failure

`client.rs` ~174, ~392 and ~766 accept any final 2xx/3xx on a client that follows redirects, so a POST bounced to `/login/` or a re-rendered form with errors counts as success. `set_stock_mode` also returns `Ok` when no stock field is found (~343). Detect a successful save positively: the redirect target, or the saved value read back. [N-5.5: all edit writes (`update_quantity`, `set_stock_mode`, `update_listing_field`) now POST with a no-redirect client via `submit_edit_and_verify` — a redirect to `/login/` is an `AuthError`, a 200 carrying form errors or the login page is an `Err`, and the saved values must read back from a fresh GET of the edit form before `Ok`. `set_stock_mode` returns `Err` when no stock field exists. Fake-server tests cover the login-bounce, error-form and redirect-without-save cases for each path.]

### XMR `create_listing` can record the wrong listing id

It takes `listings.first()` as the newest listing (~650), and on a 200 body it accepts any `/listing/<id>/` link (~662). A wrong mapping points later stock toggles at an unrelated listing. [N-5.5: `create_listing` snapshots my-listings before creating and identifies the created listing deterministically — the id in the redirect `Location`, or the pre/post diff of my-listings (exactly one new listing, or exactly one new listing carrying the requested title). Body links are never trusted and `listings.first()` is gone; a login bounce, an error form, no new listing, or an ambiguous diff is an `Err`. Fake-server tests pin each case, including a decoy body link.]

## Adapter correctness

### The in-memory order dedup in the XMR adapter consumes sales

Orders are marked seen before the engine processes them (`platform-xmrbazaar/src/lib.rs` ~121), and `run_cycle` calls `detect_sales` in dry-run too (`sync_engine.rs` ~189). A dry run can then make the next real cycle skip a sale, which breaks Phase 4's dry-run guarantee. A cycle that errors mid-batch also loses its sales. The DB dedup table already exists, so remove the in-memory set. [N-4.4: removed `seen_orders`; the engine now defers order marks and applies them in the same transaction as the stock write that consumes the sale (`Db::apply_stock_and_consume_xmr_sales`); a dry run consumes nothing and a failed cycle can no longer lose a sale.]

### XMR orders always count as one unit (`scraper.rs` ~778, pinned by `sales_page.rs:71`)

Confirm whether XMR Bazaar allows multi-unit orders in unlimited-stock mode; if it does, read the quantity.

### XMR listing status

`status.contains("active")` also matches `"inactive"` (`scraper.rs:146`), so an inactive listing reads as in stock. `parse_listings` has no test. [N-5.6: exact `status == "active"` match; `parse_listings` now has tests (`tests/listings.rs`) covering active/inactive/other statuses.]

### XMR error snippets slice `&body[..body.len().min(N)]` (`client.rs` ~183, ~407, ~672, ~776), which panics when byte N splits a UTF-8 character in the remote HTML

Truncate on a char boundary. [N-5.6: all four sites go through `client::body_snippet`, which backs off to a char boundary; pinned by unit tests with multibyte bodies.]

### XMR `set_price` drops the currency (`lib.rs` ~290), and `set_description` posts raw HTML (~300) while `create_listing` strips tags (`client.rs` ~476)

Pick one behaviour. [N-5.6: `set_price` submits price + currency in one form POST (`client::update_listing_fields`); `set_description` strips tags via `strip_html_tags`, matching `create_listing`. Both pinned by unit tests.]

### eBay pagination ignores the `next` href (`types.rs` ~50, ~101)

If `getOffers` requires `sku`, as the docs suggest, `fetch_all_offers` (`client.rs` ~314) always fails. That failure is only warned, so Inventory-API listings would have no prices. [N-5.6: `fetch_inventory_items`, `fetch_offers` and `fetch_all_offers` follow the response's `next` href via `client::next_page_url` (absent/empty/self-referential hrefs end pagination; offset arithmetic remains the fallback), pinned by a fake-server test asserting the second request goes to the href's offset.]

### Squarespace pagination loops forever if `hasNextPage` is true with a null cursor (`client.rs` ~81, ~135), and only `fetch_inventory` maps a 401 to `AuthError` (~57)

[N-5.6: both loops advance only via `client::next_cursor`, which terminates on a null/empty/repeated cursor even when `hasNextPage` is true; `fetch_products` now also maps 401 to `AuthError`. Pinned by unit tests.]

### Squarespace `update_listing` silently drops `request.price` (`lib.rs` ~324–336)

Return an error, as the capability matrix already says it cannot set a price. [N-5.6: returns `Err` before any other field is written when `request.price` is present; pinned by a unit test.]

### Hard-coded regions

eBay always uses `EBAY_US` (`lib.rs` ~653) and `EbayConfig` has no marketplace field; Amazon's consent URL is North America only (`auth.rs:9`) despite the configured `region`.

### Dead code

The XMR `stock_mode_field` cache is never written (`lib.rs` ~33, ~91), and `client.update_quantity` (~100–190) is unused.
