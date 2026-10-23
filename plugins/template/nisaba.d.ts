// nisaba.d.ts — the Nisaba plugin API v1, generated from the Rust types.
//
// DO NOT EDIT BY HAND. Regenerate with:
//   cargo run -p nisaba-vendor-runtime --example gen_nisaba_dts
//
// Every data type below is derived from the Rust structs via ts-rs
// (`crates/vendor-runtime/src/dts.rs`), so this file cannot drift from
// the host. The `Nisaba` host object at the end is hand-written to
// mirror `crates/vendor-runtime/src/runtime_js.js` — the JS shim
// creates it, not a Rust type, so keep the two in sync by hand.

export interface Weight {
  /**
   * The amount, in `unit`.
   */
  value: number;
  /**
   * Unit of `value`, e.g. `"lb"`, `"oz"`, `"kg"`, `"g"`.
   */
  unit: string;
}

export interface Dimensions {
  length: number;
  width: number;
  height: number;
  /**
   * Linear unit of the three measures, e.g. `"in"`, `"cm"`.
   */
  unit: string;
}

export interface VendorListing {
  /**
   * Stable and unique within this vendor — how Nisaba recognises the item
   * on the next run.
   */
  vendor_item_id: string;
  title: string;
  /**
   * Exact decimal amount as a string, e.g. `"19.99"` — never binary
   * floating point. A JSON number is still accepted for plugins written
   * before the v1 freeze, but new plugins must send the decimal text.
   * `currency` is required whenever a price is present.
   */
  price?: string | number | null;
  /**
   * ISO 4217 code, e.g. `"USD"`. Required whenever `price` is present —
   * the host refuses the listing otherwise.
   */
  currency?: string | null;
  /**
   * Vendor stock on hand. `null` (or omitted) means UNKNOWN — the vendor
   * gave no stock figure, and Nisaba must not treat it as 0. `0` means OUT
   * OF STOCK. The import layer preserves the distinction.
   */
  quantity?: number | null;
  sku?: string | null;
  /**
   * Every known image URL, most representative first. A legacy `image_url`
   * in a plugin's JSON is folded into `images` (first, unless already
   * listed), so old plugins keep working; new plugins must send `images`.
   */
  images?: Array<string>;
  /**
   * The item's page on the vendor's site.
   */
  url?: string | null;
  /**
   * UPC / GTIN / EAN — a string so leading zeros survive.
   */
  upc?: string | null;
  brand?: string | null;
  /**
   * Manufacturer part number.
   */
  mpn?: string | null;
  /**
   * Typed weight, e.g. `{ "value": 1.5, "unit": "lb" }`.
   */
  weight?: Weight | null;
  /**
   * Typed dimensions; all three share `unit`, e.g.
   * `{ "length": 12, "width": 9, "height": 4, "unit": "in" }`.
   */
  dimensions?: Dimensions | null;
  /**
   * Free-form strings shown on the vendor card. Nisaba recognises
   * `description`, `msrp`, `dealer_price` and `map_price`. UPC, weight,
   * brand, MPN and dimensions are typed fields now — do not also put them
   * here.
   */
  extras?: { [key in string]: string };
  /**
   * Listings sharing a `group_key` are variants of one product, told apart
   * by `variant_attributes`.
   */
  group_key?: string | null;
  /**
   * Variant discriminators, e.g. `{ "Size": "L", "Color": "Olive" }`.
   */
  variant_attributes?: { [key in string]: string };
}

export interface FetchListingsArgs {
  /**
   * UTC timestamp (`"%Y-%m-%d %H:%M:%S"`) of the last successful sync —
   * `null` on the first run, when the plugin should return the whole
   * catalog.
   */
  since?: string | null;
  /**
   * Opaque cursor the plugin returned on the previous run — `null` when
   * the plugin never returned one (or never ran).
   */
  cursor?: string | null;
}

export interface ListingDelta {
  /**
   * Only the listings that are new or changed since `since`/`cursor`.
   * Upserted by `vendor_item_id`.
   */
  changes: Array<VendorListing>;
  /**
   * `vendor_item_id`s that no longer exist at the supplier. Removed from
   * the cached catalog.
   */
  deleted?: Array<string>;
  /**
   * Opaque cursor for the next run — a page token, an ETag, a timestamp.
   */
  cursor?: string | null;
}

export type FetchListingsResult = Array<VendorListing> | ListingDelta;

export interface PluginMetadata {
  /**
   * Shown in the app; also how re-imports are matched.
   */
  name: string;
  /**
   * Bump it to publish an update.
   */
  version: string;
  /**
   * Which revision of the frozen plugin surface this plugin targets. A plugin
   * that does not declare one deserializes to 0, which is not a real revision
   * and is refused at install — so missing and unknown both fail the same way.
   */
  api_version: number;
  /**
   * Shown on the plugin's card in the marketplace.
   */
  description?: string;
  /**
   * Each field becomes an input on the plugin's Configure dialog; values
   * arrive in `config` keyed by `key`. `secret: true` masks the input.
   */
  config_fields?: Array<ConfigField>;
  category?: string;
  /**
   * Optional icon shown on the plugin's card.
   */
  icon?: string | null;
  /**
   * Hosts `Nisaba.fetch` may reach — exact names, or `*.example.com` for subdomains.
   * `None` (not declared) means unrestricted network access, which the UI flags;
   * `Some(vec![])` means no network at all.
   */
  allowed_hosts?: Array<string> | null;
}

export interface ConfigField {
  /**
   * Key under which the value arrives in `config`.
   */
  key: string;
  /**
   * Label shown on the Configure dialog.
   */
  label: string;
  required?: boolean;
  /**
   * When true, the input is masked in the UI.
   */
  secret?: boolean;
  /**
   * Hint text shown inside the input, e.g. where to find the value.
   */
  placeholder?: string | null;
}

export interface PluginInfo {
  file_name: string;
  metadata: PluginMetadata;
}

/**
 * The plugin API major version this host implements. A host refuses
 * to install a plugin whose `metadata.api_version` differs.
 */
export declare const HOST_PLUGIN_API_VERSION: 1;

/**
 * What the host passes as the second argument to `fetchListings`.
 *
 * `since` is the UTC timestamp of the last successful sync (`null` on the
 * first run — return the whole catalog then). `cursor` is the opaque cursor
 * your plugin returned on the previous run (`null` when it returned none).
 * A plugin may ignore both and keep returning the full catalog; the host
 * then keeps full-pull semantics.
 */
export type FetchListingsOptions = FetchListingsArgs;

/**
 * The incremental alternative to returning the whole catalog. The host
 * merges it into the cached catalog: `changes` are upserted by
 * `vendor_item_id`, ids in `deleted` are removed, and everything else is
 * left untouched. Return a new `cursor` so the next run can pick up where
 * this one stopped.
 */
export type ListingChanges = ListingDelta;

// ── The host API ────────────────────────────────────────────────────────────
//
// HAND-WRITTEN section of nisaba.d.ts: mirrors
// `crates/vendor-runtime/src/runtime_js.js`, the JS shim that creates the
// sandboxed `Nisaba` global before any plugin code runs. The shim is not a
// Rust type, so this cannot be generated — keep the two in sync by hand.
//
// The plugin runs in Nisaba's sandbox: there is no filesystem, no `Deno`, no
// `fetch` global and no timers — only the `Nisaba` object declared below. A
// run is limited to 30 minutes, a 1 GiB heap, 256 MiB of listing output and
// 64 MiB per response body.

/** What `Nisaba.fetch` resolves with. */
export interface NisabaResponse {
  /** Whether the status is 2xx. */
  readonly ok: boolean;
  readonly status: number;
  /** Response headers; names are lower-case. */
  readonly headers: Record<string, string>;
  /** The full response body as text. */
  text(): string;
  /** The body parsed as JSON. */
  json(): any;
}

/** Options for `Nisaba.fetch`. */
export interface NisabaFetchOptions {
  method?: string;
  headers?: Record<string, string>;
  body?: string;
}

/**
 * Per-plugin key–value storage for cursors, ETags and last-seen timestamps.
 *
 * Keys are scoped to this plugin *and* this company: a plugin can never read
 * another plugin's keys, and the same plugin under another company gets a
 * separate store. Values persist between runs in the company database (they
 * are cleared if the plugin is uninstalled).
 *
 * Both keys and values are strings — `JSON.stringify` objects first. Keys
 * are at most 256 bytes. The whole store is capped at 1 MiB per plugin;
 * `set` rejects past the cap, so keep values small (a cursor or an ETag,
 * not a cached catalog).
 */
export interface NisabaStore {
  /** The stored string, or `null` when the key was never set. */
  get(key: string): Promise<string | null>;
  /** Persist a string. Rejects when the write would exceed the 1 MiB cap. */
  set(key: string, value: string): Promise<void>;
  /** Remove a key. Resolves to whether the key existed. */
  delete(key: string): Promise<boolean>;
}

/** Log levels for `Nisaba.log`. Lines land in the app's log under the `vendor_plugin` target. */
export type NisabaLogLevel = 'trace' | 'debug' | 'info' | 'warn' | 'error';

/** The plugin's only API. `console.log` and friends write to `Nisaba.log` too. */
export interface NisabaApi {
  /**
   * HTTP request to a host listed in `metadata.allowed_hosts`. Redirects are
   * followed only to listed hosts. Resolves for any HTTP status (check `ok`);
   * rejects when the request cannot be made at all or the host is not allowed.
   */
  fetch(url: string, options?: NisabaFetchOptions): Promise<NisabaResponse>;
  /** Wait — for rate limiting. There is no setTimeout. */
  sleep(ms: number): Promise<void>;
  /**
   * Hand listings to the app as they are built, so the UI can show progress
   * on a long import. Batches are for display; the array `fetchListings`
   * returns is the result.
   */
  emitBatch(listings: VendorListing[]): void;
  store: NisabaStore;
  log: Record<NisabaLogLevel, (msg: unknown) => void>;
}

declare global {
  /** The plugin's only API — see {@link NisabaApi}. */
  const Nisaba: NisabaApi;
}

// ── Plugin entry points ─────────────────────────────────────────────────────
//
// A plugin module must export:
//
//   `metadata: PluginMetadata` — a plain object literal, read at import time
//     before `fetchListings` ever runs. Code at the top level of the module
//     runs at that point too, with no network access — keep it to constants.
//
//   `fetchListings: FetchListingsFn` — called on each vendor sync.
//
// Keep to this surface and the plugin keeps installing on every 1.x host.

/**
 * Called on each vendor sync.
 *
 * The second argument carries the host's incremental state (`options.since`
 * is the UTC timestamp of the last successful sync, `null` on the first run;
 * `options.cursor` the opaque cursor this plugin returned last time). Two
 * return shapes are accepted:
 *
 * - a `VendorListing[]` — the whole catalog (full pull). Plugins that ignore
 *   `options` keep working exactly like this.
 * - a `ListingChanges` — only what changed since `since`/`cursor`, plus
 *   `deleted` ids and the cursor for the next run. The host merges it into
 *   the cached catalog.
 *
 * Throw an `Error` when the import cannot succeed (bad credentials, the API
 * is down): its message is shown to the user and the previous catalog is
 * kept. For a problem with a single item, log it and carry on instead.
 */
export type FetchListingsFn = (
  config: Record<string, string>,
  options: FetchListingsOptions,
) => Promise<FetchListingsResult>;
