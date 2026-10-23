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
