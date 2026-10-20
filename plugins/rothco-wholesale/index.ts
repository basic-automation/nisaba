// Rothco Wholesale Vendor Plugin (v2 API)
// Imports product catalog via GraphQL API with SKU variants, pricing, and inventory.

import type {
  ProductLineListResponse,
  ProductLineDetailResponse,
  InventoryResponse,
  SkuImagesResponse,
  RothcoSku,
} from './types.ts'

export const metadata = {
  name: 'Rothco Wholesale',
  version: '2.5.0',
  description:
    'Import Rothco product catalog with SKU variants and inventory via GraphQL v2 API.',
  category: 'vendor',
  config_fields: [
    {
      key: 'api_token',
      label: 'API Token (Bearer)',
      required: true,
      secret: true,
      placeholder: 'Your Rothco API token',
    },
    {
      key: 'image_base_url',
      label: 'Image Base URL',
      required: false,
      secret: false,
      placeholder: 'https://www.rothco.com/upload/product/product/',
    },
  ],
  // Every request goes to the GraphQL endpoint; image URLs are only built, never fetched.
  allowed_hosts: ['www.rothco.com'],
}

const GRAPHQL_ENDPOINT = 'https://www.rothco.com/graphql'
const DEFAULT_IMAGE_BASE = 'https://www.rothco.com/upload/product/product/'
const PAGE_SIZE = 100
const INVENTORY_PAGE_SIZE = 1000

// ── GraphQL Queries ────────────────────────────────────────────

const PRODUCT_LINE_LIST_QUERY = `
query ProductLineList($first: Int!, $page: Int!) {
  productLine(first: $first, page: $page) {
    data {
      product_line_id
      product_line_code
      product_line_name
    }
    paginatorInfo {
      currentPage
      lastPage
      hasMorePages
      total
    }
  }
}
`

const PRODUCT_LINE_DETAIL_QUERY = `
query ProductLineById($id: ID!) {
  productLineById(product_line_id: $id) {
    product_line_id
    product_line_code
    product_line_name
    short_description
    skus {
      sku_code
      upc
      image
      weight
      prices {
        price
        case_price
        map_price
      }
      specifications {
        spec_name
        value
      }
    }
  }
}
`

// v2: inventory is now paginated (replaces allInventory)
const INVENTORY_QUERY = `
query Inventory($first: Int!, $page: Int!) {
  inventory(first: $first, page: $page) {
    data {
      upc
      inventory_level
    }
    paginatorInfo {
      currentPage
      lastPage
      hasMorePages
      total
    }
  }
}
`

// v2: separate query for SKU images
const SKU_IMAGES_QUERY = `
query SkuImages($first: Int!, $page: Int!) {
  skuImages(first: $first, page: $page) {
    data {
      sku_code
      image_id
    }
    paginatorInfo {
      currentPage
      lastPage
      hasMorePages
      total
    }
  }
}
`

// ── Plugin Entry Point ─────────────────────────────────────────

export async function fetchListings(
  config: Record<string, string>
): Promise<any[]> {
  const token = config.api_token
  if (!token) {
    throw new Error('Rothco API token is required. Configure it in plugin settings.')
  }
  const imageBase = config.image_base_url || DEFAULT_IMAGE_BASE

  // Step 1: Fetch inventory (UPC → level map) — now paginated in v2
  Nisaba.log.info('Fetching Rothco inventory levels...')
  const inventoryMap = await fetchInventory(token)
  Nisaba.log.info(`Loaded inventory for ${Object.keys(inventoryMap).length} UPCs`)

  // Step 2: Fetch SKU images (sku_code → filename map) — non-fatal, falls back to sku.image
  Nisaba.log.info('Fetching Rothco SKU images...')
  let skuImageMap: Record<string, string> = {}
  try {
    skuImageMap = await fetchSkuImages(token)
    Nisaba.log.info(`Loaded images for ${Object.keys(skuImageMap).length} SKUs`)
  } catch (e: any) {
    Nisaba.log.warn(`SKU images fetch failed (will use fallback): ${e?.message ?? e}`)
  }

  // Step 3: Paginate through all product lines
  Nisaba.log.info('Fetching Rothco product line list...')
  const productLineIds = await fetchAllProductLineIds(token)
  Nisaba.log.info(`Found ${productLineIds.length} product lines`)

  // Step 4: Fetch detail for each product line and build listings
  const listings: any[] = []
  let processed = 0

  for (const plId of productLineIds) {
    try {
      const detail = await fetchProductLineDetail(token, plId)
      if (!detail || !detail.skus || detail.skus.length === 0) continue

      for (const sku of detail.skus) {
        const qty = inventoryMap[sku.upc] ?? 0
        // Prefer image from skuImages query, fall back to sku.image field
        const imageFilename = skuImageMap[sku.sku_code] ?? sku.image
        const imageUrl = resolveImageUrl(imageFilename, imageBase)
        const attrs = buildVariantAttributes(sku)

        const extras: Record<string, string> = {}
        if (sku.upc) extras.upc = sku.upc
        if (sku.weight != null) extras.weight = String(sku.weight)
        if (detail.product_line_code) extras.product_line_code = detail.product_line_code
        if (detail.short_description) extras.description = detail.short_description
        const prices = sku.prices
        if (prices?.case_price != null) extras.dealer_price = String(prices.case_price)
        if (prices?.map_price != null) extras.map_price = String(prices.map_price)

        // Use MAP (minimum advertised price) as listing price, fall back to base price
        const skuPrice = prices?.map_price ?? prices?.price ?? null

        listings.push({
          vendor_item_id: sku.sku_code,
          title: detail.product_line_name,
          price: skuPrice,
          currency: 'USD',
          quantity: qty,
          sku: sku.sku_code,
          image_url: imageUrl,
          url: null,
          extras,
          group_key: detail.product_line_code || String(detail.product_line_id),
          variant_attributes: attrs,
        })
      }

      processed++
      if (processed % 50 === 0) {
        Nisaba.log.info(`Processed ${processed}/${productLineIds.length} product lines (${listings.length} SKUs)`)
        Nisaba.emitBatch(listings)
      }
    } catch (e: any) {
      Nisaba.log.warn(`Failed to fetch product line ${plId}: ${e?.message || e}`)
    }

    // Throttle to avoid rate limiting
    await Nisaba.sleep(300)
  }

  Nisaba.log.info(`Rothco import complete: ${listings.length} SKUs from ${processed} product lines`)
  return listings
}

// ── Helper Functions ───────────────────────────────────────────

async function gql(
  token: string,
  query: string,
  variables: Record<string, any> = {}
): Promise<any> {
  const maxRetries = 3
  for (let attempt = 0; attempt <= maxRetries; attempt++) {
    const resp = await Nisaba.fetch(GRAPHQL_ENDPOINT, {
      method: 'POST',
      headers: {
        Authorization: `Bearer ${token}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({ query, variables }),
    })

    // Retry on 429 (rate limit) and 5xx (server errors) per Rothco API docs
    if (resp.status === 429 || resp.status >= 500) {
      if (attempt < maxRetries) {
        const delay = 2000 * (attempt + 1)
        Nisaba.log.warn(`HTTP ${resp.status}, retrying in ${delay}ms (attempt ${attempt + 1}/${maxRetries})`)
        await Nisaba.sleep(delay)
        continue
      }
      throw new Error(`GraphQL request failed: HTTP ${resp.status} after ${maxRetries} retries`)
    }

    if (!resp.ok) {
      throw new Error(`GraphQL request failed: HTTP ${resp.status}`)
    }

    const data = await resp.json()
    if (data.errors && data.errors.length > 0) {
      const msg = data.errors[0].message ?? 'Unknown error'
      // Retry on server-side GraphQL errors (e.g. "Internal server error")
      if (/internal server error/i.test(msg) && attempt < maxRetries) {
        const delay = 2000 * (attempt + 1)
        Nisaba.log.warn(`GraphQL error: ${msg}, retrying in ${delay}ms (attempt ${attempt + 1}/${maxRetries})`)
        await Nisaba.sleep(delay)
        continue
      }
      throw new Error(`GraphQL error: ${msg}`)
    }

    return data
  }
}

// v2: inventory query is now paginated
async function fetchInventory(
  token: string
): Promise<Record<string, number>> {
  const map: Record<string, number> = {}
  let page = 1
  let hasMore = true

  while (hasMore) {
    const resp = (await gql(token, INVENTORY_QUERY, {
      first: INVENTORY_PAGE_SIZE,
      page,
    })) as InventoryResponse

    const data = resp.data.inventory
    for (const item of data.data) {
      const level = parseInt(item.inventory_level, 10)
      map[item.upc] = isNaN(level) ? 0 : level
    }

    hasMore = data.paginatorInfo.hasMorePages
    page++

    if (page % 5 === 0) {
      Nisaba.log.info(`Inventory: page ${data.paginatorInfo.currentPage}/${data.paginatorInfo.lastPage} (${Object.keys(map).length} UPCs)`)
    }
  }

  return map
}

// v2: separate query for SKU images
async function fetchSkuImages(
  token: string
): Promise<Record<string, string>> {
  const map: Record<string, string> = {}
  let page = 1
  let hasMore = true

  while (hasMore) {
    const resp = (await gql(token, SKU_IMAGES_QUERY, {
      first: INVENTORY_PAGE_SIZE,
      page,
    })) as SkuImagesResponse

    const data = resp.data.skuImages
    for (const item of data.data) {
      if (item.image_id) {
        map[item.sku_code] = item.image_id
      }
    }

    hasMore = data.paginatorInfo.hasMorePages
    page++
  }

  return map
}

async function fetchAllProductLineIds(token: string): Promise<number[]> {
  const ids: number[] = []
  let page = 1
  let hasMore = true

  while (hasMore) {
    const resp = (await gql(token, PRODUCT_LINE_LIST_QUERY, {
      first: PAGE_SIZE,
      page,
    })) as ProductLineListResponse

    const data = resp.data.productLine
    for (const item of data.data) {
      ids.push(item.product_line_id)
    }

    hasMore = data.paginatorInfo.hasMorePages
    page++
  }

  return ids
}

async function fetchProductLineDetail(
  token: string,
  productLineId: number
): Promise<any> {
  const resp = (await gql(token, PRODUCT_LINE_DETAIL_QUERY, {
    id: productLineId,
  })) as ProductLineDetailResponse
  return resp.data.productLineById
}

function resolveImageUrl(
  image: string | null,
  baseUrl: string
): string | null {
  if (!image) return null
  if (image.startsWith('http')) return image
  return baseUrl + image
}

function buildVariantAttributes(sku: RothcoSku): Record<string, string> {
  const attrs: Record<string, string> = {}
  if (sku.specifications) {
    for (const spec of sku.specifications) {
      if (spec.spec_name && spec.value) {
        attrs[spec.spec_name] = spec.value
      }
    }
  }
  return attrs
}
