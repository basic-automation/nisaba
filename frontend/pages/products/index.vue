<script setup lang="ts">
import type { Platform, PlatformListing, PlatformMapping, PricingSnapshot, ProductVariant, ImportPreview } from '~/types'
import { invoke } from '@tauri-apps/api/core'

const { products, loading, fetchProducts, createProduct, createMapping, deleteMapping, getMappingsBatch, listVariantsBatch } = useProducts()

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
// N-11.3: inline validation / backend error shown inside the create dialog
const createError = ref('')

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

async function initProducts() {
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
}

onMounted(initProducts)
watch(companiesInitialized, initProducts)

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

// N-11.7: one batch IPC call instead of a sequential per-product loop.
async function loadMappings() {
  const ids = products.value.map((p) => p.id)
  const fresh: Record<string, PlatformMapping[]> = {}
  if (ids.length > 0) {
    const batch = await getMappingsBatch(ids)
    for (const p of products.value) fresh[p.id] = batch[p.id] ?? []
  }
  productMappings.value = fresh
}

// N-11.7: one batch IPC call instead of a sequential per-product loop.
async function loadVariants() {
  const ids = products.value.map((p) => p.id)
  const all: Record<string, ProductVariant[]> = {}
  if (ids.length > 0) {
    try {
      const batch = await listVariantsBatch(ids)
      for (const p of products.value) all[p.id] = batch[p.id] ?? []
    } catch {
      for (const p of products.value) all[p.id] = []
    }
  }
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
  createError.value = ''
  const name = newProduct.value.name.trim()
  const sku = newProduct.value.sku.trim()
  if (!name) {
    createError.value = 'Name is required.'
    return
  }
  if (!sku) {
    createError.value = 'SKU is required.'
    return
  }
  if (newProduct.value.quantity < 0) {
    createError.value = 'Quantity cannot be negative.'
    return
  }
  try {
    await createProduct(
      name,
      sku,
      newProduct.value.quantity,
      newProduct.value.lowStockThreshold ?? undefined,
    )
  } catch (e: any) {
    // A duplicate SKU hits the unique index — surface it instead of failing silently.
    createError.value = friendlyCreateError(e)
    return
  }
  newProduct.value = { name: '', sku: '', quantity: 0, lowStockThreshold: null }
  showCreateDialog.value = false
  await loadMappings()
}

function friendlyCreateError(e: any): string {
  const msg = String(e?.message ?? e ?? '')
  if (/unique|duplicate/i.test(msg)) return 'A product with this SKU already exists.'
  return msg || 'Failed to create product.'
}

function openCreateDialog() {
  createError.value = ''
  showCreateDialog.value = true
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

// N-11.3: all destructive actions on this page go through the shared confirm dialog
const { dialog, confirming, requestConfirm, confirmDialogAction, cancelDialog } = useConfirmDialog()

async function handleExport() {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const result = await invoke<string>('export_data')
    notify({ type: 'success', title: 'Export complete', message: result, source: 'products' })
  } catch (e: any) {
    notify({ type: 'error', title: 'Export failed', message: String(e), detail: String(e), source: 'products' })
  }
}

// ── N-11.3: import shows a preview before replacing anything ──
const importPreview = ref<ImportPreview | null>(null)
const importApplying = ref(false)

async function handleImport() {
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const preview = await invoke<ImportPreview | null>('preview_import_data')
    if (preview) importPreview.value = preview
    // null = user cancelled the file picker; nothing to do
  } catch (e: any) {
    notify({ type: 'error', title: 'Import failed', message: String(e), detail: String(e), source: 'products' })
  }
}

// The preview dialog's Import button asks through the shared confirm dialog,
// since applying replaces live rows (INSERT OR REPLACE).
function confirmImportApply() {
  const preview = importPreview.value
  if (!preview) return
  const replacedTotal = preview.products_replaced + preview.variants_replaced + preview.snapshots_replaced
  requestConfirm({
    title: 'Apply import',
    message:
      `Import "${preview.file_name}"?\n\n` +
      `${preview.products_new} new products, ${preview.products_replaced} replaced.\n` +
      (replacedTotal > 0
        ? 'Replaced rows are overwritten in place — this cannot be undone.'
        : 'Nothing already in your catalog will be touched.'),
    confirmLabel: 'Import',
    action: applyImport,
  })
}

async function applyImport() {
  const preview = importPreview.value
  if (!preview) return
  importApplying.value = true
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const result = await invoke<string>('apply_import_data', { filePath: preview.file_path })
    importPreview.value = null
    await fetchProducts()
    await loadMappings()
    notify({ type: 'success', title: 'Import complete', message: result, source: 'products' })
  } catch (e: any) {
    notify({ type: 'error', title: 'Import failed', message: String(e), detail: String(e), source: 'products' })
  } finally {
    importApplying.value = false
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
        <Button variant="solid" color="accent" @click="openCreateDialog">+ New product</Button>
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
            <p v-if="createError" class="mt-4 text-xs text-red-400">{{ createError }}</p>
            <div class="flex justify-end gap-4 mt-8">
              <Button variant="solid" color="muted" size="sm" @click="showCreateDialog = false">Cancel</Button>
              <Button variant="solid" color="accent" size="sm" @click="handleCreateProduct">Create</Button>
            </div>
          </div>
        </GroundGlass>
      </div>
    </Teleport>

    <!-- N-11.3: import preview — shows what would change before anything is written -->
    <Teleport to="body">
      <div v-if="importPreview" class="fixed inset-0 z-50 flex items-center justify-center bg-black/35" @click.self="importPreview = null">
        <GroundGlass :opacity="3" :blur="10" :sizes="['70%', '70%', '65%']" class="p-8 w-full max-w-lg max-h-[80vh] overflow-auto" style="background: rgba(30, 41, 59, 0.30)">
          <div class="relative z-[1]">
            <h3 class="text-sm font-medium tracking-widest uppercase text-foreground/70 mb-2">Import preview</h3>
            <p class="text-xs text-muted/40 mb-6 font-mono">{{ importPreview.file_name }}</p>

            <table class="w-full text-sm mb-6">
              <thead>
                <tr class="text-left text-[11px] text-muted/40 uppercase tracking-wider">
                  <th class="pb-2 font-medium"></th>
                  <th class="pb-2 font-medium text-right">New</th>
                  <th class="pb-2 font-medium text-right">Replaced</th>
                </tr>
              </thead>
              <tbody class="text-foreground/80">
                <tr class="border-t border-border/20">
                  <td class="py-2">Products</td>
                  <td class="py-2 text-right tabular-nums">{{ importPreview.products_new }}</td>
                  <td class="py-2 text-right tabular-nums" :class="importPreview.products_replaced > 0 ? 'text-warning' : ''">{{ importPreview.products_replaced }}</td>
                </tr>
                <tr class="border-t border-border/20">
                  <td class="py-2">Variants</td>
                  <td class="py-2 text-right tabular-nums">{{ importPreview.variants_new }}</td>
                  <td class="py-2 text-right tabular-nums" :class="importPreview.variants_replaced > 0 ? 'text-warning' : ''">{{ importPreview.variants_replaced }}</td>
                </tr>
                <tr class="border-t border-border/20">
                  <td class="py-2">Mappings</td>
                  <td class="py-2 text-right tabular-nums">{{ importPreview.mappings_new }}</td>
                  <td class="py-2 text-right tabular-nums text-muted/40" title="Existing mappings are kept, never overwritten">{{ importPreview.mappings_skipped }} kept</td>
                </tr>
                <tr class="border-t border-border/20">
                  <td class="py-2">Snapshots</td>
                  <td class="py-2 text-right tabular-nums">{{ importPreview.snapshots_new }}</td>
                  <td class="py-2 text-right tabular-nums" :class="importPreview.snapshots_replaced > 0 ? 'text-warning' : ''">{{ importPreview.snapshots_replaced }}</td>
                </tr>
                <tr class="border-t border-border/20">
                  <td class="py-2">XMR orders</td>
                  <td class="py-2 text-right tabular-nums">{{ importPreview.orders_new }}</td>
                  <td class="py-2 text-right tabular-nums text-muted/40" title="Already-imported orders are kept">{{ importPreview.orders_skipped }} kept</td>
                </tr>
              </tbody>
            </table>

            <div v-if="importPreview.replaced_skus.length > 0" class="mb-4">
              <p class="text-[11px] uppercase tracking-wider text-warning/80 mb-2">
                Will be replaced ({{ importPreview.products_replaced }})
              </p>
              <ul class="text-xs text-muted/60 space-y-1 font-mono max-h-28 overflow-auto">
                <li v-for="sku in importPreview.replaced_skus" :key="sku">{{ sku }}</li>
                <li v-if="importPreview.replaced_skus_extra > 0" class="text-muted/30">
                  …and {{ importPreview.replaced_skus_extra }} more
                </li>
              </ul>
            </div>

            <div v-if="importPreview.new_skus.length > 0" class="mb-4">
              <p class="text-[11px] uppercase tracking-wider text-muted/40 mb-2">
                Will be added ({{ importPreview.products_new }})
              </p>
              <ul class="text-xs text-muted/60 space-y-1 font-mono max-h-28 overflow-auto">
                <li v-for="sku in importPreview.new_skus" :key="sku">{{ sku }}</li>
                <li v-if="importPreview.new_skus_extra > 0" class="text-muted/30">
                  …and {{ importPreview.new_skus_extra }} more
                </li>
              </ul>
            </div>

            <div class="flex justify-end gap-4 mt-8">
              <Button variant="solid" color="muted" size="sm" @click="importPreview = null">Cancel</Button>
              <Button variant="solid" color="accent" size="sm" :disabled="importApplying" @click="confirmImportApply">
                {{ importApplying ? 'Importing…' : 'Import' }}
              </Button>
            </div>
          </div>
        </GroundGlass>
      </div>
    </Teleport>

    <ConfirmDialog
      :open="dialog.open"
      :title="dialog.title"
      :message="dialog.message"
      :confirm-label="dialog.confirmLabel"
      :danger="dialog.danger"
      :confirming="confirming"
      @confirm="confirmDialogAction"
      @cancel="cancelDialog"
    />
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
