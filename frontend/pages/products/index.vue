<script setup lang="ts">
import type { Platform, PlatformListing, PlatformMapping, PricingSnapshot, ProductVariant } from '~/types'
import { invoke } from '@tauri-apps/api/core'

const { products, loading, fetchProducts, createProduct, deleteProduct, getMappings, createMapping, deleteMapping, listVariants } = useProducts()

// Variants per product (used for images + counts)
const productVariants = ref<Record<string, ProductVariant[]>>({})

// Cached listing photo URLs per product (from DB)
const cachedPhotos = ref<Record<string, string>>({})

const route = useRoute()
const router = useRouter()

// Main tabs: 'all' | 'unmapped'
const view = computed(() => (route.query.view as string) || 'all')
const searchQuery = ref('')
const showCreateDialog = ref(false)
const notOnPlatform = ref<Platform | 'any'>('any')

// New product form
const newProduct = ref({ name: '', sku: '', quantity: 0, lowStockThreshold: null as number | null })

// Mapping data
const productMappings = ref<Record<string, PlatformMapping[]>>({})

// Cached prices from DB (populated by sync engine)
const cachedPrices = ref<PricingSnapshot[]>([])

// Cache freshness per product (most recent timestamp)
const cacheFreshness = ref<Record<string, string>>({})

// Platform listings for product cards (images)
const platforms: { key: Platform; label: string }[] = [
  { key: 'squarespace', label: 'Squarespace' },
  { key: 'ebay', label: 'eBay' },
  { key: 'xmrbazaar', label: 'XMR Bazaar' },
  { key: 'amazon', label: 'Amazon' },
]
const platformListings = ref<Record<Platform, PlatformListing[]>>({} as any)
const platformFetched = ref<Record<Platform, boolean>>({} as any)

const { companiesInitialized } = useCompanyContext()
const { ensureCached } = useImageCache()

onMounted(async () => {
  if (!companiesInitialized.value) return
  await fetchProducts()
  // All of these are independent — run in parallel instead of sequentially.
  // loadMappings/loadVariants internally parallelize per-product IPC calls too.
  await Promise.all([
    loadMappings(),
    loadVariants(),
    fetchCachedPrices(),
    loadCachedPhotos(),
    loadCacheFreshness(),
  ])
  // Cache images to local disk in background
  cacheAllProductImages()
  // Fetch platform listings for images + prices (nice-to-have, can fail silently)
  const mappedPlatforms = new Set<Platform>()
  for (const mappings of Object.values(productMappings.value)) {
    for (const m of mappings) {
      mappedPlatforms.add(m.platform)
    }
  }
  const fetches: Promise<void>[] = []
  for (const plat of mappedPlatforms) {
    if (!platformFetched.value[plat]) {
      fetches.push(fetchPlatformListings(plat))
    }
  }
  // After all platform fetches complete, refresh cached prices + photos
  if (fetches.length > 0) {
    await Promise.allSettled(fetches)
    await Promise.all([fetchCachedPrices(), loadCachedPhotos(), loadCacheFreshness()])
    cacheAllProductImages()
  }
})

/** Collect all known image URLs and ensure they're cached locally. */
function cacheAllProductImages() {
  const urls: (string | null)[] = []
  // Variant images
  for (const variants of Object.values(productVariants.value)) {
    for (const v of variants) {
      if (v.image_url) urls.push(v.image_url)
    }
  }
  // Cached listing photos from DB
  for (const url of Object.values(cachedPhotos.value)) {
    urls.push(url)
  }
  // Live platform listing images
  for (const platform of Object.keys(platformListings.value) as Platform[]) {
    for (const listing of platformListings.value[platform] || []) {
      if (listing.image_url) urls.push(listing.image_url)
    }
  }
  if (urls.length > 0) ensureCached(urls)
}

async function loadMappings() {
  const entries = await Promise.all(
    products.value.map(async (p) => {
      const m = await getMappings(p.id)
      return [p.id, m] as const
    })
  )
  const fresh: Record<string, PlatformMapping[]> = {}
  for (const [id, m] of entries) fresh[id] = m
  productMappings.value = fresh
}

async function loadVariants() {
  const entries = await Promise.all(
    products.value.map(async (p) => {
      try {
        return [p.id, await listVariants(p.id)] as const
      } catch {
        return [p.id, [] as ProductVariant[]] as const
      }
    })
  )
  const all: Record<string, ProductVariant[]> = {}
  for (const [id, v] of entries) all[id] = v
  productVariants.value = all
}

async function loadCachedPhotos() {
  try {
    const pairs = await invoke<[string, string][]>('get_all_product_first_photos')
    const map: Record<string, string> = {}
    for (const [productId, url] of pairs) {
      map[productId] = url
    }
    cachedPhotos.value = map
  } catch {
    cachedPhotos.value = {}
  }
}

async function fetchCachedPrices() {
  try {
    cachedPrices.value = await invoke<PricingSnapshot[]>('get_all_product_prices')
  } catch {
    cachedPrices.value = []
  }
}

async function loadCacheFreshness() {
  try {
    const pairs = await invoke<[string, string][]>('get_all_cache_freshness')
    const map: Record<string, string> = {}
    for (const [productId, ts] of pairs) {
      map[productId] = ts
    }
    cacheFreshness.value = map
  } catch {
    cacheFreshness.value = {}
  }
}

async function fetchPlatformListings(platform: Platform) {
  try {
    platformListings.value[platform] = await invoke<PlatformListing[]>('fetch_all_listings', {
      platform,
    })
    platformFetched.value[platform] = true
  } catch (e) {
    console.warn(`Failed to fetch ${platform} listings:`, e)
  }
}

type PlatformListingData = { platform: Platform; listing: PlatformListing }

const productPlatformData = computed(() => {
  const listingMap = new Map<string, PlatformListing>()
  for (const platform of Object.keys(platformListings.value) as Platform[]) {
    for (const listing of platformListings.value[platform] || []) {
      listingMap.set(`${platform}:${listing.platform_item_id}`, listing)
    }
  }

  const result: Record<string, PlatformListingData[]> = {}
  for (const product of products.value) {
    const mappings = productMappings.value[product.id] || []
    const data: PlatformListingData[] = []
    for (const mapping of mappings) {
      const found = listingMap.get(`${mapping.platform}:${mapping.platform_item_id}`)
      if (found) {
        data.push({ platform: mapping.platform, listing: found })
      }
    }
    result[product.id] = data
  }
  return result
})

// For each product, return its mapped platforms with prices (if available).
// This always shows which platforms a product is on, even when prices fail to load.
function getProductPlatformInfo(productId: string): { platform: Platform; price: number | null }[] {
  const mappings = productMappings.value[productId] || []
  const seen = new Set<Platform>()
  const result: { platform: Platform; price: number | null }[] = []

  // Build price lookup from DB cache + live listings
  const priceMap = new Map<Platform, number>()
  for (const snap of cachedPrices.value) {
    if (snap.product_id === productId) {
      priceMap.set(snap.platform, snap.amount)
    }
  }
  for (const { platform, listing } of productPlatformData.value[productId] || []) {
    if (listing.price != null && !priceMap.has(platform)) {
      priceMap.set(platform, listing.price)
    }
  }

  for (const m of mappings) {
    if (seen.has(m.platform)) continue
    seen.add(m.platform)
    result.push({ platform: m.platform, price: priceMap.get(m.platform) ?? null })
  }

  return result
}

const filteredProducts = computed(() => {
  let list = products.value
  if (searchQuery.value) {
    const q = searchQuery.value.toLowerCase()
    list = list.filter(p =>
      p.name.toLowerCase().includes(q) || p.canonical_sku.toLowerCase().includes(q)
    )
  }
  if (view.value === 'unmapped') {
    list = list.filter(p => (productMappings.value[p.id]?.length ?? 0) === 0)
  }
  if (notOnPlatform.value && notOnPlatform.value !== 'any') {
    const plat = notOnPlatform.value as Platform
    list = list.filter(p => {
      const mappings = productMappings.value[p.id] || []
      return !mappings.some(m => m.platform === plat)
    })
  }
  return list
})

async function handleCreateProduct() {
  await createProduct(
    newProduct.value.name,
    newProduct.value.sku,
    newProduct.value.quantity,
    newProduct.value.lowStockThreshold ?? undefined,
  )
  newProduct.value = { name: '', sku: '', quantity: 0, lowStockThreshold: null }
  showCreateDialog.value = false
  await loadMappings()
}

async function handleDeleteProduct(id: string) {
  if (confirm('Delete this product and all its mappings?')) {
    await deleteProduct(id)
    await loadMappings()
  }
}

function formatPrice(price: number | null): string {
  if (price === null || price === undefined) return ''
  return `$${price.toFixed(2)}`
}

function getProductImageUrl(productId: string): string | null {
  // 1. Variant image (local DB, instant)
  const variants = productVariants.value[productId]
  if (variants) {
    const withImage = variants.find(v => v.image_url)
    if (withImage?.image_url) return withImage.image_url
  }
  // 2. Cached listing photo (local DB, instant)
  const cached = cachedPhotos.value[productId]
  if (cached) return cached
  // 3. Live platform listing image (API call, async)
  const data = productPlatformData.value[productId]
  if (data) {
    const found = data.find(d => d.listing.image_url)
    if (found?.listing.image_url) return found.listing.image_url
  }
  return null
}

const { notify } = useNotifications()

async function handleExport() {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const result = await invoke<string>('export_data')
    notify({ type: 'success', title: 'Export complete', message: result, source: 'products' })
  } catch (e: any) {
    notify({ type: 'error', title: 'Export failed', message: String(e), detail: String(e), source: 'products' })
  }
}

async function handleImport() {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const result = await invoke<string>('import_data')
    if (result !== 'Cancelled') {
      await fetchProducts()
      await loadMappings()
      notify({ type: 'success', title: 'Import complete', message: result, source: 'products' })
    }
  } catch (e: any) {
    notify({ type: 'error', title: 'Import failed', message: String(e), detail: String(e), source: 'products' })
  }
}

// ── Tab bar ──
const productTabs = [
  { label: 'All', value: 'all' },
  { label: 'Unmapped', value: 'unmapped' },
]
const tabModel = computed({
  get: () => view.value,
  set: (val: string) => {
    navigateTo(val === 'unmapped' ? '/products?view=unmapped' : '/products')
  },
})
</script>

<template>
  <CompanyRequired>
  <div class="flex flex-col h-full">
    <!-- Header (pinned) -->
    <div class="flex items-end justify-between mb-10 shrink-0">
      <h2 class="text-sm font-medium tracking-widest uppercase text-muted/60">Products</h2>
      <div class="flex items-center gap-3">
        <Button variant="solid" color="default" @click="handleExport">Export</Button>
        <Button variant="solid" color="default" @click="handleImport">Import</Button>
        <Button variant="solid" color="accent" @click="showCreateDialog = true">+ New product</Button>
      </div>
    </div>

    <!-- Tabs + Search row (pinned) -->
    <div class="flex flex-wrap items-center gap-6 mb-4 shrink-0">
      <TabBar v-model="tabModel" :items="productTabs" />

      <Splatter trigger="focus" :opacity-ranges="[[0.08, 0.18], [0.08, 0.18], [0.03, 0.15]]" class="flex-1 min-w-[180px]">
        <input
          v-model="searchQuery"
          type="text"
          placeholder="Search..."
          class="relative z-[1] w-full bg-transparent border-b border-border/30 px-2 py-1.5 text-sm text-foreground placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
        >
      </Splatter>
    </div>

    <!-- Filter row -->
    <div class="flex items-center gap-3 mb-6 shrink-0">
      <Select v-model="notOnPlatform">
        <SelectTrigger class="not-on-select">
          <span class="select-label">Not on:</span>
          <SelectValue placeholder="Any platform" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem value="any">
            Any platform
          </SelectItem>
          <SelectItem v-for="p in platforms" :key="p.key" :value="p.key">
            {{ p.label }}
          </SelectItem>
        </SelectContent>
      </Select>
    </div>

    <!-- Products grid -->
    <div class="flex-1 overflow-auto min-h-0 -mr-6 pr-6">
    <div v-if="loading" class="py-16 text-sm text-muted/40">Loading...</div>

    <div v-else-if="filteredProducts.length > 0" class="product-grid">
      <NuxtLink
        v-for="product in filteredProducts"
        :key="product.id"
        :to="`/products/${product.id}`"
        class="flex"
      >
        <ProductCard
          :name="product.name"
          :sku="product.canonical_sku"
          :quantity="product.quantity"
          :low-stock-threshold="product.low_stock_threshold"
          :image-url="getProductImageUrl(product.id)"
          :platform-info="getProductPlatformInfo(product.id)"
          :variant-count="productVariants[product.id]?.length"
          :last-refreshed="cacheFreshness[product.id] ?? null"
        />
      </NuxtLink>
    </div>

    <div v-else class="py-16 text-sm text-muted/40">
      {{ searchQuery ? 'No products match your search.' : 'No products yet.' }}
    </div>
    </div>

    <!-- Create Product Dialog -->
    <Teleport to="body">
      <div v-if="showCreateDialog" class="fixed inset-0 z-50 flex items-center justify-center bg-black/35" @click.self="showCreateDialog = false">
        <GroundGlass :opacity="3" :blur="10" :sizes="['70%', '70%', '65%']" class="p-8 w-full max-w-md" style="background: rgba(30, 41, 59, 0.30)">
          <div class="relative z-[1]">
            <h3 class="text-sm font-medium tracking-widest uppercase text-foreground/70 mb-6">New product</h3>
            <div class="space-y-5">
              <div>
                <label class="block text-xs text-foreground/70 mb-1.5">Name</label>
                <GlassInput v-model="newProduct.name" />
              </div>
              <div>
                <label class="block text-xs text-foreground/70 mb-1.5">SKU</label>
                <GlassInput v-model="newProduct.sku" mono />
              </div>
              <div>
                <label class="block text-xs text-foreground/70 mb-1.5">Initial quantity</label>
                <InputNumber v-model="newProduct.quantity" :min="0" />
              </div>
              <div>
                <label class="block text-xs text-foreground/70 mb-1.5">Low stock threshold</label>
                <InputNumber v-model="newProduct.lowStockThreshold" :min="0" placeholder="Optional" />
              </div>
            </div>
            <div class="flex justify-end gap-4 mt-8">
              <Button variant="solid" color="muted" size="sm" @click="showCreateDialog = false">Cancel</Button>
              <Button variant="solid" color="accent" size="sm" @click="handleCreateProduct">Create</Button>
            </div>
          </div>
        </GroundGlass>
      </div>
    </Teleport>
  </div>
  </CompanyRequired>
</template>

<style scoped>
.product-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  gap: 1rem;
  align-content: start;
  padding-bottom: 1rem;
}

/* ════ Not-on platform select ════ */
.not-on-select {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  width: fit-content !important;
  padding: 4px 12px;
  font-size: 12px;
  color: rgba(176, 190, 201, 0.6);
  background: transparent;
  border: none;
  border-radius: 0.5rem;
  cursor: pointer;
  transition: color 0.2s ease;
  outline: none;
}
.not-on-select:hover {
  color: rgba(176, 190, 201, 0.8);
}
.not-on-select .select-label {
  text-transform: uppercase;
  letter-spacing: 0.05em;
  font-size: 11px;
  opacity: 0.6;
}
</style>
