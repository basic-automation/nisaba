// Nisaba vendor plugin template.
//
// Copy this directory, rename the plugin in `metadata`, point API_BASE and
// `allowed_hosts` at your supplier, and rewrite `toListing` for their data. Import it
// from the Vendors → Marketplace page (a directory or a .zip of it).
//
// The plugin runs in Nisaba's sandbox: there is no filesystem, no `Deno`, no `fetch`
// global and no timers — only the `Nisaba` object declared in `nisaba.d.ts`. A run is
// limited to 30 minutes, a 1 GiB heap, 256 MiB of listing output and 64 MiB per
// response body.
//
// ── Plugin API v1 (frozen) ──────────────────────────────────────────────────
//
// The full v1 surface lives in `nisaba.d.ts` next to this file — generated from
// the host's Rust types, so it cannot drift from what the host actually
// accepts. It declares:
//
//   - the data types: `VendorListing`, `Weight`, `Dimensions`,
//     `FetchListingsArgs` (also as `FetchListingsOptions`), `ListingDelta`
//     (also as `ListingChanges`), `FetchListingsResult`, `PluginMetadata`,
//     `ConfigField`
//   - the host API: the `Nisaba` global (`fetch`, `sleep`, `emitBatch`,
//     `store`, `log`) and the `NisabaResponse`
//   - the entry points: `metadata: PluginMetadata` and
//     `fetchListings: FetchListingsFn`
//
// This template targets plugin API v1 (`metadata.api_version: 1`). The host
// refuses to install a plugin whose `api_version` major it does not support.
// Keep to the surface in `nisaba.d.ts` and the plugin keeps installing on
// every 1.x host.

import type {
  FetchListingsFn,
  PluginMetadata,
  VendorListing,
} from './nisaba'

// ── Metadata ────────────────────────────────────────────────────────────────
//
// Read when the plugin is imported, before `fetchListings` ever runs, and must be a
// plain object literal. Code at the top level of this module runs at that point too,
// with no network access — keep it to constants.

export const metadata: PluginMetadata = {
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

/** Called on each vendor sync — see `FetchListingsFn` in `nisaba.d.ts`.
 *
 *  This template does a full pull: the Example Supplier API has no delta
 *  endpoint. A supplier that does would look like this:
 *
 *    export const fetchListings: FetchListingsFn = async (config, options) => {
 *      const qs = options.since ? `?since=${encodeURIComponent(options.since)}` : ''
 *      const data = await getJson(`${API_BASE}/products/delta${qs}`, apiKey)
 *      return {
 *        changes: data.items.flatMap(toListings),
 *        deleted: data.deleted_ids ?? [],
 *        cursor: data.next_cursor ?? null,
 *      }
 *    }
 *
 *  Throw an Error when the import cannot succeed (bad credentials, the API is down):
 *  its message is shown to the user and the previous catalog is kept. For a problem
 *  with a single item, log it and carry on instead. */
export const fetchListings: FetchListingsFn = async (config, options) => {
  const apiKey = config.api_key?.trim()
  if (!apiKey) throw new Error('Set an API key on the plugin before syncing.')

  // `options.since`/`options.cursor` arrive on every run; this template ignores
  // them and pulls the whole catalog (full-pull semantics) — see the
  // incremental example in the doc comment above.

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
