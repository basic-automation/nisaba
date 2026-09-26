// Nisaba vendor plugin template.
//
// Copy this directory, rename the plugin in `metadata`, point API_BASE and
// `allowed_hosts` at your supplier, and rewrite `toListing` for their data. Import it
// from the Vendors → Marketplace page (a directory or a .zip of it).
//
// The plugin runs in Nisaba's sandbox: there is no filesystem, no `Deno`, no `fetch`
// global and no timers — only the `Nisaba` object declared below. A run is limited to
// 30 minutes, a 1 GiB heap, 256 MiB of listing output and 64 MiB per response body.

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
  /** Lines in the app's log under the `vendor_plugin` target. */
  log: Record<'trace' | 'debug' | 'info' | 'warn' | 'error', (msg: unknown) => void>
}

/** One purchasable item. Only `vendor_item_id` and `title` are required. */
export interface VendorListing {
  /** Stable and unique within this vendor — how Nisaba recognises the item next run. */
  vendor_item_id: string
  title: string
  price?: number | null
  currency?: string | null // ISO 4217, e.g. "USD"
  quantity?: number | null // vendor stock on hand
  sku?: string | null
  image_url?: string | null
  url?: string | null // the item's page on the vendor's site
  /** Free-form strings shown on the vendor card. Nisaba recognises `upc`, `weight`,
   *  `description`, `msrp`, `dealer_price` and `map_price`. */
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
    if (v.upc) extras.upc = String(v.upc)
    if (product.description) extras.description = String(product.description)

    return {
      vendor_item_id: String(v.sku ?? product.id),
      title: product.name,
      price: typeof v.price === 'number' ? v.price : null,
      currency: 'USD',
      quantity: typeof v.stock === 'number' ? v.stock : null,
      sku: v.sku ?? null,
      image_url: v.image ?? product.image ?? null,
      url: product.url ?? null,
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
