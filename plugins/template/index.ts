// Nisaba vendor plugin template.
//
// Copy this directory, rename the plugin in `metadata`, point API_BASE and
// `allowed_hosts` at your supplier, and rewrite `toListing` for their data. Import it
// from the Vendors → Marketplace page (a directory or a .zip of it).
//
// The plugin runs in Nisaba's sandbox: there is no filesystem, no `Deno`, no `fetch`
// global and no timers — only the `Nisaba` object declared below. A run is limited to
// 30 minutes, a 1 GiB heap, 256 MiB of listing output and 64 MiB per response body.

// ── Plugin API v1 (frozen) ──────────────────────────────────────────────────
//
// This template targets plugin API v1 (`metadata.api_version: 1`). The host
// refuses to install a plugin whose `api_version` major it does not support,
// so this file also serves as the written-down v1 surface:
//
//   metadata: name, version, api_version, description, config_fields
//             (key, label, required, secret, placeholder), category, icon,
//             allowed_hosts
//   entry points: `metadata` (a plain object literal, read before any run)
//                 and `fetchListings(config): Promise<VendorListing[]>`
//   the Nisaba object: fetch, sleep, emitBatch, store, log — see "The host API" below
//   VendorListing: vendor_item_id, title, price, currency, quantity, sku,
//                  images, url, upc, brand, mpn, weight, dimensions,
//                  extras, group_key, variant_attributes
//
// Keep to this surface and the plugin keeps installing on every 1.x host.

// ── The host API ────────────────────────────────────────────────────────────

interface NisabaResponse {
  readonly ok: boolean // status is 2xx
  readonly status: number
  readonly headers: Record<string, string> // names are lower-case
  text(): string
  json(): any
}

declare const Nisaba: {
  /** HTTP request to a host listed in `metadata.allowed_hosts`. Redirects are followed
   *  only to listed hosts. Resolves for any HTTP status (check `ok`); rejects when the
   *  request cannot be made at all or the host is not allowed. */
  fetch(
    url: string,
    options?: { method?: string; headers?: Record<string, string>; body?: string },
  ): Promise<NisabaResponse>
  /** Wait — for rate limiting. There is no setTimeout. */
  sleep(ms: number): Promise<void>
  /** Hand listings to the app as they are built, so the UI can show progress on a long
   *  import. Batches are for display; the array `fetchListings` returns is the result. */
  emitBatch(listings: VendorListing[]): void
  /** Per-plugin key–value storage for cursors, ETags and last-seen timestamps.
   *
   *  Keys are scoped to this plugin *and* this company: a plugin can never read
   *  another plugin's keys, and the same plugin under another company gets a
   *  separate store. Values persist between runs in the company database (they
   *  are cleared if the plugin is uninstalled).
   *
   *  Both keys and values are strings — `JSON.stringify` objects first. Keys
   *  are at most 256 bytes. The whole store is capped at 1 MiB per plugin;
   *  `set` rejects past the cap, so keep values small (a cursor or an ETag,
   *  not a cached catalog).
   *
   *  Example — resume a paged supplier API where the last run left off:
   *
   *    const cursor = (await Nisaba.store.get('cursor')) ?? '1'
   *    // ... fetch pages starting at `cursor` ...
   *    await Nisaba.store.set('cursor', nextCursor)
   */
  store: {
    /** The stored string, or `null` when the key was never set. */
    get(key: string): Promise<string | null>
    /** Persist a string. Rejects when the write would exceed the 1 MiB cap. */
    set(key: string, value: string): Promise<void>
    /** Remove a key. Resolves to whether the key existed. */
    delete(key: string): Promise<boolean>
  }
  /** Lines in the app's log under the `vendor_plugin` target. `console.log` and friends
   *  write there too. */
  log: Record<'trace' | 'debug' | 'info' | 'warn' | 'error', (msg: unknown) => void>
}

/** One purchasable item. Only `vendor_item_id` and `title` are required. */
export interface VendorListing {
  /** Stable and unique within this vendor — how Nisaba recognises the item next run. */
  vendor_item_id: string
  title: string
  /** Exact decimal amount as a string, e.g. `"12.99"` — never binary floating
   *  point. A JSON number is still accepted (plugins written before the v1
   *  freeze), but new plugins must send the exact decimal text. `currency` is
   *  required whenever a price is present; the host refuses the listing
   *  otherwise. */
  price?: string | number | null
  /** ISO 4217, e.g. "USD". Required when `price` is present. */
  currency?: string | null
  /** Vendor stock on hand. `null` (or omitted) means UNKNOWN — the vendor gave
   *  no stock figure, and Nisaba must not treat it as 0. `0` means OUT OF
   *  STOCK. Getting this wrong marks unknown stock as sold out (or vice
   *  versa), so when the supplier's API has no stock field, leave it `null`. */
  quantity?: number | null // vendor stock on hand
  sku?: string | null
  /** Every known image URL, most representative first. Replaces the pre-freeze
   *  single `image_url` — the host still accepts `image_url` from old plugins
   *  and folds it into `images[0]`, but new plugins must send `images`. */
  images?: string[]
  url?: string | null // the item's page on the vendor's site
  /** UPC / GTIN / EAN. A string so leading zeros survive. */
  upc?: string | null
  brand?: string | null
  /** Manufacturer part number. */
  mpn?: string | null
  /** Typed weight, e.g. `{ value: 1.5, unit: "lb" }`. */
  weight?: { value: number; unit: string } | null
  /** Typed dimensions; all three share `unit`, e.g. `{ length: 12, width: 9,
   *  height: 4, unit: "in" }`. */
  dimensions?: { length: number; width: number; height: number; unit: string } | null
  /** Free-form strings shown on the vendor card. Nisaba recognises `description`,
   *  `msrp`, `dealer_price` and `map_price`. UPC/weight/brand/MPN/dimensions are
   *  typed fields now — do not also put them here. */
  extras?: Record<string, string>
  /** Listings sharing a `group_key` are variants of one product… */
  group_key?: string | null
  /** …told apart by these, e.g. { Size: 'L', Color: 'Olive' }. */
  variant_attributes?: Record<string, string>
}

// ── Metadata ────────────────────────────────────────────────────────────────
//
// Read when the plugin is imported, before `fetchListings` ever runs, and must be a
// plain object literal. Code at the top level of this module runs at that point too,
// with no network access — keep it to constants.

export const metadata = {
  name: 'Example Supplier', // shown in the app; also how re-imports are matched
  version: '1.0.0', // bump it to publish an update
  // The frozen plugin surface this plugin targets. A host whose API major is not
  // 1 refuses to install this plugin, and says why.
  api_version: 1,
  description: 'Template: imports the Example Supplier catalog.',
  category: 'vendor',
  // Each field becomes an input on the plugin's Configure dialog; values arrive in
  // `config` keyed by `key`. `secret: true` masks the input. Values are stored in the
  // app's database and shared with the company's P2P peers.
  config_fields: [
    {
      key: 'api_key',
      label: 'API key',
      required: true,
      secret: true,
      placeholder: 'From your Example Supplier account page',
    },
  ],
  // Every host `Nisaba.fetch` may reach: exact names, or '*.example.com' for any
  // subdomain. Users see this list before installing. [] means no network at all.
  allowed_hosts: ['api.example.com'],
}

// ── fetchListings ───────────────────────────────────────────────────────────

const API_BASE = 'https://api.example.com/v1'
const BATCH_SIZE = 200
const MAX_RETRIES = 5

/** Called on each vendor sync. Return the whole catalog.
 *
 *  Throw an Error when the import cannot succeed (bad credentials, the API is down):
 *  its message is shown to the user and the previous catalog is kept. For a problem
 *  with a single item, log it and carry on instead. */
export async function fetchListings(config: Record<string, string>): Promise<VendorListing[]> {
  const apiKey = config.api_key?.trim()
  if (!apiKey) throw new Error('Set an API key on the plugin before syncing.')

  const listings: VendorListing[] = []
  let pending: VendorListing[] = []

  for (let page = 1; page !== null; ) {
    const data = await getJson(`${API_BASE}/products?page=${page}`, apiKey)

    for (const product of data.items ?? []) {
      try {
        for (const listing of toListings(product)) {
          listings.push(listing)
          pending.push(listing)
        }
      } catch (e: any) {
        Nisaba.log.warn(`Skipping product ${product?.id}: ${e?.message ?? e}`)
      }
    }

    if (pending.length >= BATCH_SIZE) {
      Nisaba.emitBatch(pending)
      pending = []
    }
    page = data.next_page ?? null
  }

  Nisaba.log.info(`Example Supplier: ${listings.length} listings`)
  return listings
}

/** Map one product from the supplier's API to listings — one per variant. */
function toListings(product: any): VendorListing[] {
  if (product.id == null || !product.name) throw new Error('missing id or name')

  const variants: any[] = product.variants?.length ? product.variants : [product]
  return variants.map((v) => {
    const extras: Record<string, string> = {}
    if (product.description) extras.description = String(product.description)

    // All known images, most representative first.
    const images: string[] = []
    const primary = v.image ?? product.image
    if (primary) images.push(String(primary))
    for (const extra of v.images ?? product.images ?? []) {
      const url = String(extra)
      if (url && !images.includes(url)) images.push(url)
    }

    // quantity: null = the supplier gave no stock figure (unknown). 0 = out of
    // stock. Never invent one from the other.
    const quantity = typeof v.stock === 'number' ? v.stock : null

    return {
      vendor_item_id: String(v.sku ?? product.id),
      title: product.name,
      // Decimal text, never binary floating point: String(12.5) is "12.5".
      price: typeof v.price === 'number' ? String(v.price)
        : typeof v.price === 'string' ? v.price : null,
      currency: 'USD',
      quantity,
      sku: v.sku ?? null,
      images,
      url: product.url ?? null,
      upc: v.upc != null ? String(v.upc) : null,
      brand: product.brand ?? v.brand ?? null,
      mpn: v.mpn ?? null,
      weight: v.weight != null
        ? { value: Number(v.weight), unit: String(v.weight_unit ?? 'lb') }
        : null,
      dimensions: v.length != null
        ? {
            length: Number(v.length),
            width: Number(v.width ?? v.length),
            height: Number(v.height ?? v.length),
            unit: String(v.dimension_unit ?? 'in'),
          }
        : null,
      extras,
      group_key: product.variants?.length ? String(product.id) : null,
      variant_attributes: v.options ?? {},
    }
  })
}

/** GET with auth, honouring 429 Retry-After and backing off on 5xx. */
async function getJson(url: string, apiKey: string): Promise<any> {
  for (let attempt = 1; ; attempt++) {
    const res = await Nisaba.fetch(url, {
      headers: { Authorization: `Bearer ${apiKey}`, Accept: 'application/json' },
    })
    if (res.ok) return res.json()

    if (res.status === 401 || res.status === 403) {
      throw new Error('Example Supplier rejected the API key.')
    }
    const retryable = res.status === 429 || res.status >= 500
    if (!retryable || attempt >= MAX_RETRIES) {
      throw new Error(`Example Supplier returned HTTP ${res.status} for ${url}`)
    }
    const retryAfter = Number(res.headers['retry-after'])
    const waitMs = retryAfter > 0 ? retryAfter * 1000 : 500 * 2 ** attempt
    Nisaba.log.warn(`HTTP ${res.status}; retrying in ${waitMs} ms`)
    await Nisaba.sleep(waitMs)
  }
}
