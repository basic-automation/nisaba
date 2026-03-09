<script setup lang="ts">
import type { FullListing, Platform, PlatformListing, PlatformMapping, Product, ProductVariant, VendorListingForProduct } from '~/types'
import { invoke } from '@tauri-apps/api/core'

const route = useRoute()
const productId = route.params.id as string

const { getProduct, updateProduct, deleteProduct, getMappings, createMapping, deleteMapping, listVariants, createVariant, updateVariant, deleteVariant } = useProducts()
const { diffListings } = useListings()
const { photos: cachedPhotoTuples, fetchProductPhotos } = usePhotos()
const { resolveImage, resolveThumb, ensureCached } = useImageCache()

const { companiesInitialized } = useCompanyContext()

const product = ref<Product | null>(null)
const productLoading = ref(true)
const mappings = ref<PlatformMapping[]>([])
const editing = ref(false)
const editForm = ref({ name: '', sku: '', lowStockThreshold: null as number | null })

// Variant state
const variants = ref<ProductVariant[]>([])
const showAddVariant = ref(false)
const editingVariantId = ref<string | null>(null)
const variantForm = ref({ sku: '', name: '', quantity: 0, imageUrl: '', attrKey: '', attrValue: '', attributes: {} as Record<string, string> })

// Listings comparison state
const comparisonListings = ref<FullListing[]>([])
const comparisonLoading = ref(false)
const comparisonError = ref('')

// Vendor listing state
const vendorListings = ref<VendorListingForProduct[]>([])

// Variant filter for listings comparison
const selectedVariantId = ref<string | null>(null)

// Variant selector state
const variantSelections = ref<Record<string, string>>({})

const variantAttributeKeys = computed(() => {
  const keys = new Set<string>()
  for (const v of variants.value) {
    for (const k of Object.keys(v.attributes)) keys.add(k)
  }
  return [...keys]
})

const variantAttributeOptions = computed(() => {
  const options: Record<string, string[]> = {}
  for (const key of variantAttributeKeys.value) {
    const vals = new Set<string>()
    // Only show values that are compatible with current selections on OTHER keys
    for (const v of variants.value) {
      const matches = Object.entries(variantSelections.value).every(
        ([k, sel]) => k === key || !sel || v.attributes[k] === sel
      )
      if (matches && v.attributes[key]) vals.add(v.attributes[key])
    }
    options[key] = [...vals].sort()
  }
  return options
})

const selectedVariant = computed(() => {
  if (variantAttributeKeys.value.length === 0) return variants.value[0] ?? null
  const sels = variantSelections.value
  const allSelected = variantAttributeKeys.value.every(k => sels[k])
  if (!allSelected) return null
  return variants.value.find(v =>
    variantAttributeKeys.value.every(k => v.attributes[k] === sels[k])
  ) ?? null
})

function onVariantSelectionChange(key: string, value: string) {
  variantSelections.value = { ...variantSelections.value, [key]: value === '__all__' ? '' : value }
}

// Keep selectedVariantId in sync with the hero variant selector
watch(selectedVariant, (v) => {
  selectedVariantId.value = v?.id ?? null
})

// Auto-fetch comparison when a variant is selected and there's no data yet
watch(selectedVariantId, (vid) => {
  if (!vid) return
  const variantKeys = new Set(
    mappings.value
      .filter(m => m.variant_id === vid)
      .map(m => `${m.platform}:${m.platform_item_id}`)
  )
  const hasData = comparisonListings.value.some(l =>
    variantKeys.has(`${l.platform}:${l.platform_item_id}`)
  )
  if (!hasData && variantKeys.size > 0 && !comparisonLoading.value) {
    fetchComparison()
  }
})

// Confirm-dialog state
const confirmDialog = ref<{ show: boolean; title: string; message: string; action: (() => Promise<void>) | null }>({
  show: false, title: '', message: '', action: null,
})
function requestConfirm(title: string, message: string, action: () => Promise<void>) {
  confirmDialog.value = { show: true, title, message, action }
}
async function confirmAction() {
  if (confirmDialog.value.action) await confirmDialog.value.action()
  confirmDialog.value = { show: false, title: '', message: '', action: null }
}
function cancelConfirm() {
  confirmDialog.value = { show: false, title: '', message: '', action: null }
}

// Platform accordion state + grouped mappings
const openPlatforms = ref(new Set<Platform>())

const mappingsByPlatform = computed(() => {
  const grouped = new Map<Platform, PlatformMapping[]>()
  for (const m of mappings.value) {
    const list = grouped.get(m.platform) ?? []
    list.push(m)
    grouped.set(m.platform, list)
  }
  return grouped
})

function togglePlatform(platform: Platform) {
  const s = new Set(openPlatforms.value)
  if (s.has(platform)) s.delete(platform)
  else s.add(platform)
  openPlatforms.value = s
}

/** Look up variant for a mapping so we can show attributes instead of raw IDs. */
function variantForMapping(m: PlatformMapping): ProductVariant | undefined {
  if (!m.variant_id) return undefined
  return variants.value.find(v => v.id === m.variant_id)
}

/** Human label for a mapping row: variant attributes or fallback. */
function mappingLabel(m: PlatformMapping): string {
  const v = variantForMapping(m)
  if (v) {
    const attrs = Object.entries(v.attributes)
    if (attrs.length > 0) return attrs.map(([k, val]) => `${k}: ${val}`).join(', ')
    return v.name || v.sku
  }
  return 'Product-level'
}

// Add mapping state
const showAddMapping = ref(false)
const addMappingPlatform = ref<Platform>('ebay')
const addMappingMode = ref<'browse' | 'manual'>('browse')
const addMappingListings = ref<PlatformListing[]>([])
const addMappingLoading = ref(false)
const addMappingError = ref('')
const manualItemId = ref('')
const manualSku = ref('')
const browseSearch = ref('')
const mappingVariantId = ref<string | null>(null)

async function initProductPage() {
  if (!companiesInitialized.value) return
  product.value = await getProduct(productId)
  productLoading.value = false
  // All of these are independent DB reads — run in parallel
  await Promise.all([
    loadMappings(),
    loadVariants(),
    fetchProductPhotos(productId),
    loadVendorListings(),
    loadCachedComparison(),
  ])
  // Then refresh from live APIs in background
  if (mappings.value.length > 0) {
    fetchComparison()
  }
}

onMounted(initProductPage)
watch(companiesInitialized, initProductPage)

function getVendorStockForVariant(variantId: string): number | null {
  // Direct match first
  const vl = vendorListings.value.find(v => v.variant_id === variantId)
  if (vl?.quantity != null) return vl.quantity
  // Fallback: look for attribute-equivalent variants (bridges vendor/platform duplicates)
  const target = variants.value.find(v => v.id === variantId)
  if (!target || Object.keys(target.attributes).length === 0) return null
  const tAttrs = new Map(
    Object.entries(target.attributes).map(([k, v]) => [k.toLowerCase().trim(), v.toLowerCase().trim()]),
  )
  for (const v of variants.value) {
    if (v.id === variantId) continue
    const vAttrs = new Map(
      Object.entries(v.attributes).map(([k, val]) => [k.toLowerCase().trim(), val.toLowerCase().trim()]),
    )
    if (vAttrs.size !== tAttrs.size) continue
    let match = true
    for (const [key, val] of tAttrs) {
      if (vAttrs.get(key) !== val) { match = false; break }
    }
    if (match) {
      const eq = vendorListings.value.find(vl => vl.variant_id === v.id)
      if (eq?.quantity != null) return eq.quantity
    }
  }
  return null
}

async function loadVendorListings() {
  try {
    vendorListings.value = await invoke<VendorListingForProduct[]>('get_vendor_listings_for_product', { productId })
    console.log('[product] vendor listings loaded:', vendorListings.value.length, 'for product', productId)
  } catch (e) {
    console.error('[product] get_vendor_listings_for_product FAILED:', e)
  }
}

async function loadCachedComparison() {
  try {
    const cached = await invoke<FullListing[]>('get_cached_listings', { productId })
    if (cached.length > 0) {
      comparisonListings.value = cached
    }
  } catch { /* no cache yet */ }
}

// All photos from variants + cached DB photos + live platform photos (deduplicated)
const allPhotos = computed(() => {
  const seen = new Set<string>()
  const photos: { url: string; platform: Platform | null }[] = []
  // 1. Variant images (local, instant)
  for (const v of variants.value) {
    if (v.image_url && !seen.has(v.image_url)) {
      seen.add(v.image_url)
      photos.push({ url: v.image_url, platform: null })
    }
  }
  // 2. Cached listing photos from DB (local, instant)
  for (const [plat, photo] of cachedPhotoTuples.value) {
    if (!seen.has(photo.url)) {
      seen.add(photo.url)
      photos.push({ url: photo.url, platform: plat })
    }
  }
  // 3. Live comparison listing photos (API, async)
  for (const listing of comparisonListings.value) {
    if (!listing.photos) continue
    for (const photo of listing.photos) {
      if (!seen.has(photo.url)) {
        seen.add(photo.url)
        photos.push({ url: photo.url, platform: listing.platform })
      }
    }
  }
  return photos
})

const showAllPhotos = ref(false)
const PHOTOS_PER_PAGE = 4 // 2 rows of 2

const heroPhotos = computed(() => {
  if (showAllPhotos.value || allPhotos.value.length <= PHOTOS_PER_PAGE) {
    return allPhotos.value
  }
  return allPhotos.value.slice(0, PHOTOS_PER_PAGE)
})

const hasMorePhotos = computed(() => allPhotos.value.length > PHOTOS_PER_PAGE && !showAllPhotos.value)

// Cache all photo URLs to local disk whenever they change
watch(allPhotos, (photos) => {
  if (photos.length > 0) {
    ensureCached(photos.map(p => p.url))
  }
}, { immediate: true })

async function fetchComparison() {
  comparisonLoading.value = true
  comparisonError.value = ''
  try {
    comparisonListings.value = await diffListings(productId)
    // Cache listing details in DB so subsequent visits have data
    const cachePromises = comparisonListings.value.map(listing =>
      invoke('cache_listing_detail', {
        productId,
        platform: listing.platform,
        platformItemId: listing.platform_item_id,
      }).catch((e: unknown) => console.warn('cache_listing_detail failed:', e))
    )
    Promise.all(cachePromises).then(() => fetchProductPhotos(productId))
  } catch (e: any) {
    comparisonError.value = e?.toString() || 'Failed to fetch listings'
  } finally {
    comparisonLoading.value = false
  }
}

async function loadMappings() {
  mappings.value = await getMappings(productId)
}

async function loadVariants() {
  variants.value = await listVariants(productId)
}

function resetVariantForm() {
  variantForm.value = { sku: '', name: '', quantity: 0, imageUrl: '', attrKey: '', attrValue: '', attributes: {} }
  editingVariantId.value = null
}

function addVariantAttribute() {
  const key = variantForm.value.attrKey.trim()
  const val = variantForm.value.attrValue.trim()
  if (key) {
    variantForm.value.attributes[key] = val
    variantForm.value.attrKey = ''
    variantForm.value.attrValue = ''
  }
}

function removeVariantAttribute(key: string) {
  delete variantForm.value.attributes[key]
}

function openAddVariant() {
  resetVariantForm()
  showAddVariant.value = true
}

function openEditVariant(v: ProductVariant) {
  editingVariantId.value = v.id
  variantForm.value = {
    sku: v.sku,
    name: v.name,
    quantity: v.on_hand_quantity,
    imageUrl: v.image_url || '',
    attrKey: '',
    attrValue: '',
    attributes: { ...v.attributes },
  }
  showAddVariant.value = true
}

async function saveVariant() {
  if (editingVariantId.value) {
    await updateVariant(editingVariantId.value, variantForm.value.sku, variantForm.value.name, variantForm.value.attributes, variantForm.value.quantity, variantForm.value.imageUrl || undefined)
  } else {
    await createVariant(productId, variantForm.value.sku, variantForm.value.name, variantForm.value.attributes, variantForm.value.quantity, variantForm.value.imageUrl || undefined)
  }
  showAddVariant.value = false
  resetVariantForm()
  await loadVariants()
  product.value = await getProduct(productId)
}

function handleDeleteVariant(id: string) {
  requestConfirm('Delete variant', 'Delete this variant? This cannot be undone.', async () => {
    await deleteVariant(id)
    await loadVariants()
    product.value = await getProduct(productId)
  })
}

function startEdit() {
  if (!product.value) return
  editForm.value = {
    name: product.value.name,
    sku: product.value.canonical_sku,
    lowStockThreshold: product.value.low_stock_threshold,
  }
  editing.value = true
}

async function saveEdit() {
  await updateProduct(productId, editForm.value.name, editForm.value.sku, editForm.value.lowStockThreshold ?? undefined)
  product.value = await getProduct(productId)
  editing.value = false
}

function handleDelete() {
  requestConfirm('Delete product', 'Delete this product and all its mappings? This cannot be undone.', async () => {
    await deleteProduct(productId)
    navigateTo('/products')
  })
}

async function handleDeleteMapping(id: number) {
  await deleteMapping(id)
  await loadMappings()
}

function openAddMapping() {
  showAddMapping.value = true
  addMappingPlatform.value = 'ebay'
  addMappingMode.value = 'browse'
  addMappingListings.value = []
  addMappingError.value = ''
  manualItemId.value = ''
  manualSku.value = ''
  browseSearch.value = ''
  mappingVariantId.value = selectedVariant.value?.id ?? null
  fetchPlatformListings()
}

watch(addMappingPlatform, () => {
  addMappingListings.value = []
  addMappingError.value = ''
  browseSearch.value = ''
  if (showAddMapping.value && addMappingMode.value === 'browse') {
    fetchPlatformListings()
  }
})

const filteredBrowseListings = computed(() => {
  if (!browseSearch.value) return addMappingListings.value
  const q = browseSearch.value.toLowerCase()
  return addMappingListings.value.filter(l =>
    l.title.toLowerCase().includes(q) ||
    l.platform_item_id.toLowerCase().includes(q) ||
    (l.sku && l.sku.toLowerCase().includes(q))
  )
})

async function fetchPlatformListings() {
  addMappingLoading.value = true
  addMappingListings.value = []
  addMappingError.value = ''
  try {
    addMappingListings.value = await invoke<PlatformListing[]>('fetch_unmapped_listings', {
      platform: addMappingPlatform.value,
    })
  } catch (e: any) {
    addMappingError.value = e?.toString() || 'Failed to fetch listings'
  } finally {
    addMappingLoading.value = false
  }
}

async function resolveVariantForMapping(sku?: string, attrs?: Record<string, string>): Promise<string> {
  // Use explicitly selected variant
  if (mappingVariantId.value) return mappingVariantId.value
  // Try to match by SKU
  if (sku) {
    const match = variants.value.find(v => v.sku.toLowerCase() === sku.toLowerCase())
    if (match) return match.id
  }
  // Try to match by variant attributes (e.g., Color + Size)
  if (attrs && Object.keys(attrs).length > 0) {
    const normalized = new Map(
      Object.entries(attrs).map(([k, v]) => [k.toLowerCase().trim(), v.toLowerCase().trim()]),
    )
    const match = variants.value.find(v => {
      const vAttrs = new Map(
        Object.entries(v.attributes).map(([k, val]) => [k.toLowerCase().trim(), val.toLowerCase().trim()]),
      )
      if (vAttrs.size === 0) return false
      for (const [key, val] of normalized) {
        if (vAttrs.get(key) !== val) return false
      }
      return true
    })
    if (match) return match.id
  }
  // Single variant — use it
  if (variants.value.length === 1) return variants.value[0].id
  throw new Error('Select a variant before linking a mapping')
}

async function linkListing(listing: PlatformListing) {
  const variantId = await resolveVariantForMapping(listing.sku ?? undefined, listing.variant_attributes ?? undefined)
  await createMapping(productId, addMappingPlatform.value, listing.platform_item_id, variantId, listing.sku ?? undefined)
  await loadMappings()
  showAddMapping.value = false
  await Promise.all([fetchComparison(), loadVendorListings()])
}

async function linkManual() {
  if (!manualItemId.value.trim()) return
  const variantId = await resolveVariantForMapping(manualSku.value.trim() || undefined)
  await createMapping(productId, addMappingPlatform.value, manualItemId.value.trim(), variantId, manualSku.value.trim() || undefined)
  await loadMappings()
  showAddMapping.value = false
  await Promise.all([fetchComparison(), loadVendorListings()])
}

// refreshAndCache is now just fetchComparison — caching is built in
const refreshAndCache = fetchComparison

// Collect all variant IDs that share the same attributes as the selected variant.
// This bridges vendor-imported variants (which hold vendor_data) with platform-created
// duplicates (which hold platform_mappings) so both listings appear side-by-side.
const equivalentVariantIds = computed<Set<string>>(() => {
  if (!selectedVariantId.value) return new Set()
  const selected = variants.value.find(v => v.id === selectedVariantId.value)
  if (!selected || Object.keys(selected.attributes).length === 0) {
    return new Set([selectedVariantId.value])
  }
  const selAttrs = new Map(
    Object.entries(selected.attributes).map(([k, v]) => [k.toLowerCase().trim(), v.toLowerCase().trim()]),
  )
  const ids = new Set<string>()
  for (const v of variants.value) {
    if (v.id === selectedVariantId.value) { ids.add(v.id); continue }
    const vAttrs = new Map(
      Object.entries(v.attributes).map(([k, val]) => [k.toLowerCase().trim(), val.toLowerCase().trim()]),
    )
    if (vAttrs.size !== selAttrs.size) continue
    let match = true
    for (const [key, val] of selAttrs) {
      if (vAttrs.get(key) !== val) { match = false; break }
    }
    if (match) ids.add(v.id)
  }
  return ids
})

// Variant-scoped listing filters
const filteredVendorListings = computed(() => {
  if (!selectedVariantId.value) return vendorListings.value
  const eqIds = equivalentVariantIds.value
  // Show vendor listings that match any equivalent variant OR have no variant_id (product-level)
  return vendorListings.value.filter(vl => !vl.variant_id || eqIds.has(vl.variant_id))
})

const filteredComparisonListings = computed(() => {
  if (!selectedVariantId.value) return comparisonListings.value
  const eqIds = equivalentVariantIds.value
  const matchingItemIds = new Set(
    mappings.value
      .filter(m => m.variant_id != null && eqIds.has(m.variant_id))
      .map(m => `${m.platform}:${m.platform_item_id}`)
  )
  return comparisonListings.value.filter(l =>
    matchingItemIds.has(`${l.platform}:${l.platform_item_id}`)
  )
})

// Per-platform price summary: range when no variant selected, exact when one is
const priceSummary = computed(() => {
  const listings = filteredComparisonListings.value.filter(l => l.price)

  const byPlatform = new Map<string, { label: string; platform?: Platform; min: number; max: number; currency: string }>()

  // Vendor prices
  for (const vl of filteredVendorListings.value) {
    if (vl.price == null) continue
    const key = `vendor:${vl.plugin_name}`
    const existing = byPlatform.get(key)
    if (existing) {
      existing.min = Math.min(existing.min, vl.price)
      existing.max = Math.max(existing.max, vl.price)
    } else {
      byPlatform.set(key, {
        label: vl.plugin_name,
        min: vl.price,
        max: vl.price,
        currency: vl.currency ?? 'USD',
      })
    }
  }

  // Platform prices
  for (const l of listings) {
    if (!l.price) continue
    const key = `platform:${l.platform}`
    const existing = byPlatform.get(key)
    if (existing) {
      existing.min = Math.min(existing.min, l.price.amount)
      existing.max = Math.max(existing.max, l.price.amount)
    } else {
      byPlatform.set(key, {
        label: l.platform,
        platform: l.platform,
        min: l.price.amount,
        max: l.price.amount,
        currency: l.price.currency,
      })
    }
  }

  return [...byPlatform.values()].map(({ label, platform, min, max, currency }) => ({
    label,
    platform,
    min,
    max,
    currency,
    isRange: min !== max,
  }))
})

// Get variant attributes for a platform listing (from mapping → variant)
function getListingAttributes(listing: FullListing): [string, string][] {
  // Check if variant_id is set and look up the variant's attributes
  if (listing.variant_id) {
    const v = variants.value.find(vr => vr.id === listing.variant_id)
    if (v && Object.keys(v.attributes).length > 0) {
      return Object.entries(v.attributes)
    }
  }
  // Also check equivalent variants via mappings
  const mapping = mappings.value.find(
    m => m.platform === listing.platform && m.platform_item_id === listing.platform_item_id,
  )
  if (mapping?.variant_id) {
    const v = variants.value.find(vr => vr.id === mapping.variant_id)
    if (v && Object.keys(v.attributes).length > 0) {
      return Object.entries(v.attributes)
    }
  }
  return []
}

function formatTimeAgo(isoTimestamp: string): string {
  const now = Date.now()
  const then = new Date(isoTimestamp + (isoTimestamp.includes('Z') || isoTimestamp.includes('+') ? '' : 'Z')).getTime()
  if (isNaN(then)) return ''
  const diffMs = now - then
  const minutes = Math.floor(diffMs / 60000)
  if (minutes < 1) return 'just now'
  if (minutes < 60) return `${minutes}m ago`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `${hours}h ago`
  const days = Math.floor(hours / 24)
  return `${days}d ago`
}
</script>

<template>
  <CompanyRequired>
  <div class="flex flex-col h-full">
    <!-- Back link (pinned) -->
    <NuxtLink to="/products" class="inline-block text-muted hover:text-foreground text-[11px] tracking-[0.15em] uppercase mb-10 transition-colors shrink-0">
      &larr; Products
    </NuxtLink>

    <!-- Scrollable content -->
    <div class="flex-1 overflow-auto min-h-0 -mr-6 pr-6">
    <div v-if="productLoading" class="flex items-center justify-center py-32">
      <span class="text-muted text-sm">Loading...</span>
    </div>

    <div v-else-if="product">

      <!-- ════════ HERO: Images + Product Info ════════ -->
      <div class="hero-layout mb-16">

        <!-- Left — stacked images -->
        <div>
          <div v-if="heroPhotos.length > 0" class="photo-grid">
            <div
              v-for="(photo, i) in heroPhotos"
              :key="photo.url"
              class="photo-frame group"
            >
              <img :src="resolveImage(photo.url) ?? photo.url" :alt="`${product.name} — ${i + 1}`" loading="lazy">
              <div v-if="photo.platform" class="photo-badge">
                <PlatformBadge :platform="photo.platform" icon-only />
              </div>
            </div>
          </div>
          <Button
            v-if="hasMorePhotos"
            variant="solid"
            color="default"
            size="xs"
            class="w-full mt-2"
            @click="showAllPhotos = true"
          >Load more ({{ allPhotos.length - PHOTOS_PER_PAGE }} more)</Button>
          <div v-else-if="allPhotos.length === 0" class="photo-empty">
            <svg class="w-10 h-10 text-muted/20" fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="1">
              <path stroke-linecap="round" stroke-linejoin="round" d="M2.25 15.75l5.159-5.159a2.25 2.25 0 013.182 0l5.159 5.159m-1.5-1.5l1.409-1.409a2.25 2.25 0 013.182 0l2.909 2.909M3.75 21h16.5A2.25 2.25 0 0022.5 18.75V5.25A2.25 2.25 0 0020.25 3H3.75A2.25 2.25 0 001.5 5.25v13.5A2.25 2.25 0 003.75 21z" />
            </svg>
          </div>
        </div>

        <!-- Right — product info (sticky) -->
        <div class="lg:sticky lg:top-6 lg:self-start">
          <div v-if="!editing">
            <p class="label-sm mb-3">{{ product.canonical_sku }}</p>
            <h1 class="text-3xl lg:text-4xl font-bold text-foreground leading-tight mb-8">{{ product.name }}</h1>

            <!-- ── Variants (selector + detail + actions) ── -->
            <div class="mb-8">
              <div class="flex items-center justify-between mb-3">
                <span class="label-sm">Variants <span class="text-muted/30 font-normal">({{ variants.length }})</span></span>
                <Button variant="solid" color="accent" size="xs" @click="openAddVariant">Add Variant</Button>
              </div>

              <!-- Attribute selectors -->
              <div v-if="variants.length > 0 && variantAttributeKeys.length > 0" class="flex flex-wrap gap-3 mb-4">
                <div v-for="key in variantAttributeKeys" :key="key" class="min-w-[120px] flex-1">
                  <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1">{{ key }}</label>
                  <Select
                    :model-value="variantSelections[key] || '__all__'"
                    @update:model-value="onVariantSelectionChange(key, $event)"
                  >
                    <SelectTrigger>
                      <SelectValue placeholder="All" />
                    </SelectTrigger>
                    <SelectContent>
                      <SelectItem value="__all__">All</SelectItem>
                      <SelectItem v-for="val in variantAttributeOptions[key]" :key="val" :value="val">{{ val }}</SelectItem>
                    </SelectContent>
                  </Select>
                </div>
              </div>

              <!-- Selected variant detail -->
              <div v-if="selectedVariant" class="variant-detail">
                <div class="flex items-center gap-2 mb-1">
                  <span class="text-xs text-foreground font-mono">{{ selectedVariant.sku }}</span>
                  <span class="text-[11px] text-muted truncate">{{ selectedVariant.name }}</span>
                </div>
                <div class="flex items-center justify-between">
                  <span class="text-sm text-foreground font-medium">Qty: {{ selectedVariant.quantity }}</span>
                </div>
                <div class="flex gap-2 mt-3">
                  <Button variant="solid" color="accent" size="xs" @click="openEditVariant(selectedVariant!)">Edit Variant</Button>
                  <Button variant="solid" color="danger" size="xs" @click="handleDeleteVariant(selectedVariant!.id)">Delete Variant</Button>
                </div>
              </div>

              <!-- No selection yet hint -->
              <p v-else-if="variants.length > 0 && variantAttributeKeys.length > 0" class="text-xs text-muted/40 py-2">Select options above to view a variant.</p>

              <!-- No attributes — simple list fallback -->
              <div v-else-if="variants.length > 0 && variantAttributeKeys.length === 0" class="space-y-px">
                <div
                  v-for="v in variants"
                  :key="v.id"
                  class="mapping-row group"
                >
                  <div class="flex-1 min-w-0">
                    <div class="flex items-center gap-2">
                      <span class="text-xs text-foreground font-mono">{{ v.sku }}</span>
                      <span class="text-[11px] text-muted truncate">{{ v.name }}</span>
                    </div>
                  </div>
                  <div class="flex items-center gap-3 shrink-0">
                    <span class="text-xs text-foreground">{{ v.quantity }}</span>
                    <Button variant="solid" color="accent" size="xs" @click="openEditVariant(v)">Edit</Button>
                    <Button variant="solid" color="danger" size="xs" @click="handleDeleteVariant(v.id)">Del</Button>
                  </div>
                </div>
              </div>

              <p v-else class="text-xs text-muted/40 py-2">No variants yet.</p>
            </div>

            <!-- Per-source prices (vendor + platform; range when no variant selected, exact when one is) -->
            <div class="space-y-3 mb-10">
              <template v-if="priceSummary.length > 0">
                <div
                  v-for="ps in priceSummary"
                  :key="ps.label"
                  class="flex items-center gap-3"
                >
                  <PlatformBadge v-if="ps.platform" :platform="ps.platform" />
                  <VendorBadge v-else :name="ps.label" />
                  <span v-if="ps.isRange" class="text-2xl font-semibold text-foreground tabular-nums">
                    {{ ps.min.toFixed(2) }} – {{ ps.max.toFixed(2) }}
                  </span>
                  <span v-else class="text-2xl font-semibold text-foreground tabular-nums">
                    {{ ps.min.toFixed(2) }}
                  </span>
                  <span class="text-sm text-muted">{{ ps.currency }}</span>
                </div>
              </template>
              <p v-else class="text-sm text-muted/60">No price data</p>
            </div>

            <!-- Details -->
            <div class="border-t border-border/20 pt-6 mb-8 space-y-4">
              <div class="detail-row">
                <span class="label-sm">Quantity</span>
                <div class="flex items-center gap-2">
                  <span class="text-sm font-medium text-foreground">{{ product.quantity ?? 0 }}</span>
                  <span class="text-[10px] text-muted/30">(sum of variants)</span>
                  <span
                    v-if="product.low_stock_threshold && (product.quantity ?? 0) <= product.low_stock_threshold"
                    class="text-[10px] uppercase tracking-wider text-yellow-400 bg-yellow-400/10 px-2 py-0.5"
                  >Low stock</span>
                </div>
              </div>

              <div v-if="product.low_stock_threshold" class="detail-row">
                <span class="label-sm">Alert threshold</span>
                <span class="text-sm font-medium text-foreground">&le; {{ product.low_stock_threshold }}</span>
              </div>

              <div class="detail-row">
                <span class="label-sm">Photos</span>
                <span class="text-sm font-medium text-foreground">{{ allPhotos.length }}</span>
              </div>

            </div>

            <!-- Platform Mappings (accordion per platform) -->
            <div class="border-t border-border/20 pt-6 mb-8">
              <div class="flex items-center justify-between mb-3">
                <span class="label-sm">Platforms <span class="text-muted/30 font-normal">({{ mappings.length }})</span></span>
                <div class="flex gap-2">
                  <Button
                    variant="solid"
                    color="default"
                    size="xs"
                    :to="`/products/${productId}/publish`"
                  >Publish</Button>
                  <Button
                    variant="solid"
                    color="accent"
                    size="xs"
                    @click="openAddMapping"
                  >Map Listing</Button>
                </div>
              </div>

              <div v-if="mappingsByPlatform.size > 0" class="space-y-1.5">
                <div
                  v-for="[platform, pMappings] in mappingsByPlatform"
                  :key="platform"
                  class="platform-accordion"
                >
                  <!-- Accordion header -->
                  <button
                    class="platform-accordion-header"
                    @click="togglePlatform(platform)"
                  >
                    <div class="flex items-center gap-2.5">
                      <svg
                        class="w-3 h-3 text-muted/40 transition-transform duration-200"
                        :class="{ 'rotate-90': openPlatforms.has(platform) }"
                        fill="none" viewBox="0 0 24 24" stroke="currentColor" stroke-width="2.5"
                      ><path stroke-linecap="round" stroke-linejoin="round" d="M9 5l7 7-7 7" /></svg>
                      <PlatformBadge :platform="platform" />
                      <span class="text-[11px] text-muted/40">{{ pMappings.length }} mapping{{ pMappings.length > 1 ? 's' : '' }}</span>
                    </div>
                  </button>

                  <!-- Accordion body -->
                  <Transition name="accordion">
                    <div v-if="openPlatforms.has(platform)" class="platform-accordion-body">
                      <div
                        v-for="m in pMappings"
                        :key="m.id"
                        class="mapping-row group"
                      >
                        <div class="flex items-center gap-2 flex-1 min-w-0">
                          <span class="text-xs text-foreground">{{ mappingLabel(m) }}</span>
                          <span v-if="m.platform_sku" class="text-[10px] text-muted/40 font-mono">{{ m.platform_sku }}</span>
                        </div>
                        <button
                          class="text-[11px] text-red-400/40 hover:text-red-400 opacity-0 group-hover:opacity-100 transition-all shrink-0"
                          @click="handleDeleteMapping(m.id)"
                        >Unlink</button>
                      </div>
                    </div>
                  </Transition>
                </div>
              </div>
              <p v-else class="text-xs text-muted/40 py-3">No platform mappings yet.</p>
            </div>

            <!-- Actions -->
            <div class="flex gap-3">
              <Button variant="solid" color="accent" class="flex-1" @click="startEdit">Edit Product</Button>
              <Button variant="solid" color="danger" @click="handleDelete">Delete</Button>
            </div>
          </div>

          <!-- Edit form -->
          <div v-else>
            <h3 class="text-lg font-semibold text-foreground mb-5">Edit Product</h3>
            <div class="space-y-5">
              <div>
                <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Name</label>
                <input v-model="editForm.name" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
              </div>
              <div>
                <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">SKU</label>
                <input v-model="editForm.sku" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground font-mono focus:outline-none focus:border-accent/50 transition-colors">
              </div>
              <div>
                <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Low Stock Threshold</label>
                <InputNumber v-model="editForm.lowStockThreshold" :min="0" placeholder="Optional" />
              </div>
            </div>
            <div class="flex gap-4 mt-6">
              <Button variant="solid" color="accent" class="flex-1" @click="saveEdit">Save</Button>
              <Button variant="solid" color="muted" @click="editing = false">Cancel</Button>
            </div>
          </div>
        </div>
      </div>

      <!-- ════════ LISTINGS COMPARISON ════════ -->
      <section v-if="!variantAttributeKeys.length || selectedVariant" class="mb-16">
        <div class="flex items-center justify-between mb-6">
          <h2 class="section-title">Listings Comparison</h2>
          <Button
            v-if="mappings.length > 0"
            variant="solid"
            color="accent"
            size="xs"
            :disabled="comparisonLoading"
            @click="refreshAndCache"
          >{{ comparisonLoading ? 'Fetching...' : 'Refresh' }}</Button>
        </div>

        <div v-if="comparisonLoading && filteredComparisonListings.length === 0 && filteredVendorListings.length === 0" class="py-16 text-center text-sm text-muted/40">
          Loading listings...
        </div>

        <div v-else-if="comparisonError && filteredComparisonListings.length === 0 && filteredVendorListings.length === 0" class="text-sm text-red-400 py-4">
          {{ comparisonError }}
        </div>

        <div v-else-if="filteredComparisonListings.length > 0 || filteredVendorListings.length > 0" class="comparison-grid">
          <!-- Vendor listings -->
          <div
            v-for="(vl, idx) in filteredVendorListings"
            :key="`vendor-${vl.vendor_item_id}`"
            class="comparison-card"
            :class="{ 'comparison-card--divider': idx > 0 }"
          >
            <div class="card-header">
              <VendorBadge :name="vl.plugin_name" />
              <span class="text-[10px] text-muted/40 font-mono">{{ vl.vendor_item_id }}</span>
              <span class="text-[10px] text-muted/30 ml-auto">{{ formatTimeAgo(vl.fetched_at) }}</span>
            </div>

            <div class="p-5 space-y-5">
              <div>
                <p class="label-sm mb-1">Title</p>
                <p class="text-sm text-foreground">{{ vl.title }}</p>
              </div>

              <div v-if="vl.extras?.description">
                <p class="label-sm mb-1">Description</p>
                <div class="desc-box">
                  <p class="whitespace-pre-wrap">{{ vl.extras.description }}</p>
                </div>
              </div>

              <div class="grid grid-cols-2 gap-3 text-xs">
                <div>
                  <p class="label-sm mb-1">Quantity</p>
                  <p class="text-foreground font-medium">{{ vl.quantity ?? 0 }}</p>
                </div>
                <div v-if="vl.price != null">
                  <p class="label-sm mb-1">Price</p>
                  <p class="text-foreground font-medium">{{ vl.price.toFixed(2) }} {{ vl.currency ?? 'USD' }}</p>
                </div>
                <div v-if="vl.sku" class="col-span-2">
                  <p class="label-sm mb-1">SKU</p>
                  <p class="text-foreground font-mono">{{ vl.sku }}</p>
                </div>
              </div>

              <div v-if="vl.image_url">
                <p class="label-sm mb-1.5">Photos (1)</p>
                <div class="flex gap-1 flex-wrap">
                  <img :src="resolveThumb(vl.image_url) ?? vl.image_url" :alt="vl.title" class="w-10 h-10 object-cover" loading="lazy">
                </div>
              </div>

              <div v-if="Object.keys(vl.variant_attributes).length > 0">
                <p class="label-sm mb-1">Attributes</p>
                <div class="flex gap-2 flex-wrap">
                  <span
                    v-for="(val, key) in vl.variant_attributes"
                    :key="key"
                    class="text-[10px] text-muted/60"
                  >{{ key }}: {{ val }}</span>
                </div>
              </div>
            </div>
            <div v-if="vl.url" class="px-5 pb-5 pt-2">
              <a :href="vl.url" target="_blank" rel="noopener" class="block w-full text-center text-[11px] text-accent/60 hover:text-accent transition-colors py-1.5 border border-accent/20 rounded">View source</a>
            </div>
          </div>

          <!-- Platform listings -->
          <div
            v-for="(listing, idx) in filteredComparisonListings"
            :key="`${listing.platform}-${listing.platform_item_id}`"
            class="comparison-card"
            :class="{ 'comparison-card--divider': idx > 0 || filteredVendorListings.length > 0 }"
          >
            <div class="card-header">
              <PlatformBadge :platform="listing.platform" />
              <span class="text-[10px] text-muted/40 font-mono">{{ listing.platform_item_id }}</span>
              <span v-if="listing.fetched_at" class="text-[10px] text-muted/30 ml-auto">{{ formatTimeAgo(listing.fetched_at) }}</span>
            </div>

            <div class="p-5 space-y-5">
              <div>
                <p class="label-sm mb-1">Title</p>
                <p class="text-sm text-foreground">{{ listing.title }}</p>
              </div>

              <div v-if="listing.description">
                <p class="label-sm mb-1">Description</p>
                <div class="desc-box">
                  <pre v-if="listing.platform === 'xmrbazaar'" class="whitespace-pre-wrap font-sans text-xs m-0">{{ listing.description.plain_text || listing.description.html }}</pre>
                  <div v-else-if="listing.description.html" class="prose-listing" v-html="listing.description.html" />
                  <p v-else-if="listing.description.plain_text" class="whitespace-pre-wrap">{{ listing.description.plain_text }}</p>
                  <p v-else class="text-muted/40 italic">No description</p>
                </div>
              </div>

              <div class="grid grid-cols-2 gap-3 text-xs">
                <div>
                  <p class="label-sm mb-1">Quantity</p>
                  <p class="text-foreground font-medium">{{ listing.quantity }}</p>
                </div>
                <div v-if="listing.price">
                  <p class="label-sm mb-1">Price</p>
                  <p class="text-foreground font-medium">{{ listing.price.amount }} {{ listing.price.currency }}</p>
                </div>
                <div v-if="listing.sku" class="col-span-2">
                  <p class="label-sm mb-1">SKU</p>
                  <p class="text-foreground font-mono">{{ listing.sku }}</p>
                </div>
              </div>

              <div v-if="listing.photos && listing.photos.length > 0">
                <p class="label-sm mb-1.5">Photos ({{ listing.photos.length }})</p>
                <div class="flex gap-1 flex-wrap">
                  <img
                    v-for="(photo, i) in listing.photos.slice(0, 6)"
                    :key="i"
                    :src="resolveThumb(photo.url) ?? photo.url"
                    :alt="photo.alt_text ?? ''"
                    class="w-10 h-10 object-cover"
                    loading="lazy"
                  >
                  <span v-if="listing.photos.length > 6" class="w-10 h-10 flex items-center justify-center text-[10px] text-muted bg-background/60">
                    +{{ listing.photos.length - 6 }}
                  </span>
                </div>
              </div>

              <div v-if="getListingAttributes(listing).length > 0">
                <p class="label-sm mb-1">Attributes</p>
                <div class="flex gap-2 flex-wrap">
                  <span
                    v-for="[key, val] in getListingAttributes(listing)"
                    :key="key"
                    class="text-[10px] text-muted/60"
                  >{{ key }}: {{ val }}</span>
                </div>
              </div>
            </div>
            <div class="px-5 pb-5 pt-2">
              <Button
                variant="solid"
                color="accent"
                size="xs"
                class="w-full"
                :to="`/products/${productId}/publish?mode=edit&platform=${listing.platform}&itemId=${listing.platform_item_id}`"
              >
                Edit listing
              </Button>
            </div>
          </div>
        </div>

        <p v-else class="text-sm text-muted/40 py-6">No listing data available.</p>
      </section>
    </div>

    <div v-else class="flex items-center justify-center py-32">
      <p class="text-muted text-sm">Product not found.</p>
    </div>
    </div>

    <!-- ════════ ADD/EDIT VARIANT MODAL ════════ -->
    <Teleport to="body">
      <Transition name="modal">
        <div v-if="showAddVariant" class="fixed inset-0 z-50 flex items-center justify-center bg-black/35" @click.self="showAddVariant = false">
          <GroundGlass :opacity="3" :blur="10" :sizes="['70%', '70%', '65%']" class="p-8 w-full max-w-md" style="background: rgba(30, 41, 59, 0.30)">
            <div class="relative z-[1]">
              <h3 class="text-sm font-medium tracking-widest uppercase text-foreground/70 mb-6">{{ editingVariantId ? 'Edit variant' : 'Add variant' }}</h3>
              <div class="space-y-5">
                <div>
                  <label class="block text-xs text-foreground/70 mb-1.5">SKU</label>
                  <GlassInput v-model="variantForm.sku" mono placeholder="e.g. PROD-BLK-L" />
                </div>
                <div>
                  <label class="block text-xs text-foreground/70 mb-1.5">Name</label>
                  <GlassInput v-model="variantForm.name" placeholder="e.g. Black / Large" />
                </div>
                <div>
                  <label class="block text-xs text-foreground/70 mb-1.5">On-Hand Quantity</label>
                  <InputNumber v-model="variantForm.quantity" :min="0" />
                  <div v-if="editingVariantId && getVendorStockForVariant(editingVariantId)" class="mt-1.5 text-[11px] text-muted/40">
                    Vendor stock: {{ getVendorStockForVariant(editingVariantId) }}
                    <span class="text-foreground/60 ml-1">Available: {{ variantForm.quantity + (getVendorStockForVariant(editingVariantId) || 0) }}</span>
                  </div>
                </div>
                <div>
                  <label class="block text-xs text-foreground/70 mb-1.5">Image URL (optional)</label>
                  <GlassInput v-model="variantForm.imageUrl" placeholder="https://..." />
                </div>
                <div>
                  <label class="block text-xs text-foreground/70 mb-1.5">Attributes</label>
                  <div v-if="Object.keys(variantForm.attributes).length > 0" class="flex flex-wrap gap-1.5 mb-3">
                    <span
                      v-for="(val, key) in variantForm.attributes"
                      :key="key"
                      class="inline-flex items-center gap-1 text-[11px] bg-accent/10 text-accent/80 px-2.5 py-1 rounded-full"
                    >
                      {{ key }}: {{ val }}
                      <button class="text-red-400/60 hover:text-red-400 ml-1" @click="removeVariantAttribute(key as string)">&times;</button>
                    </span>
                  </div>
                  <div class="flex gap-2 items-center">
                    <GlassInput v-model="variantForm.attrKey" class="flex-1" placeholder="Key (e.g. Size)" @keydown.enter.prevent="addVariantAttribute" />
                    <GlassInput v-model="variantForm.attrValue" class="flex-1" placeholder="Value (e.g. Large)" @keydown.enter.prevent="addVariantAttribute" />
                    <button class="text-accent hover:text-accent/80 text-lg font-medium transition-colors px-1" @click="addVariantAttribute">+</button>
                  </div>
                </div>
              </div>
              <div class="flex justify-end gap-4 mt-8">
                <Button variant="solid" color="muted" size="sm" @click="showAddVariant = false">Cancel</Button>
                <Button variant="solid" color="accent" size="sm" :disabled="!variantForm.sku.trim() || !variantForm.name.trim()" @click="saveVariant">
                  {{ editingVariantId ? 'Save' : 'Add' }}
                </Button>
              </div>
            </div>
          </GroundGlass>
        </div>
      </Transition>
    </Teleport>

    <!-- ════════ ADD MAPPING MODAL ════════ -->
    <Teleport to="body">
      <Transition name="modal">
        <div v-if="showAddMapping" class="fixed inset-0 z-50 flex items-center justify-center bg-black/70" @click.self="showAddMapping = false">
          <div class="bg-surface border border-border/30 p-6 w-full max-w-2xl space-y-4 max-h-[80vh] flex flex-col">
            <h3 class="text-lg font-semibold text-foreground">Add Platform Mapping</h3>

            <!-- Variant selector for mapping -->
            <div v-if="variants.length > 0" class="space-y-1">
              <label class="label-sm block">Attach to variant</label>
              <select v-model="mappingVariantId" class="input-field">
                <option :value="null">Product-level (no variant)</option>
                <option v-for="v in variants" :key="v.id" :value="v.id">
                  {{ v.name }} <span v-if="v.sku">({{ v.sku }})</span>
                </option>
              </select>
            </div>

            <div class="flex gap-2 items-center">
              <select v-model="addMappingPlatform" class="input-field !w-auto">
                <option value="ebay">eBay</option>
                <option value="squarespace">Squarespace</option>
                <option value="xmrbazaar">XMR Bazaar</option>
              </select>
              <div class="flex border border-border/30 overflow-hidden ml-2">
                <button
                  class="px-3 py-2 text-xs transition-colors"
                  :class="addMappingMode === 'browse' ? 'bg-accent/15 text-accent' : 'text-muted hover:text-foreground'"
                  @click="addMappingMode = 'browse'"
                >Browse</button>
                <button
                  class="px-3 py-2 text-xs transition-colors"
                  :class="addMappingMode === 'manual' ? 'bg-accent/15 text-accent' : 'text-muted hover:text-foreground'"
                  @click="addMappingMode = 'manual'"
                >Manual</button>
              </div>
            </div>

            <template v-if="addMappingMode === 'browse'">
              <div class="flex gap-2 items-center">
                <button
                  class="btn-sm text-accent hover:text-accent/80"
                  :disabled="addMappingLoading"
                  @click="fetchPlatformListings"
                >{{ addMappingLoading ? 'Loading...' : 'Fetch Listings' }}</button>
                <input
                  v-if="addMappingListings.length > 0"
                  v-model="browseSearch"
                  type="text"
                  placeholder="Filter..."
                  class="input-field flex-1"
                >
              </div>
              <div v-if="addMappingError" class="text-sm text-red-400">{{ addMappingError }}</div>
              <div v-if="filteredBrowseListings.length > 0" class="space-y-1 overflow-auto flex-1 min-h-0">
                <p class="text-[11px] text-muted mb-1">{{ filteredBrowseListings.length }} of {{ addMappingListings.length }} listings</p>
                <div
                  v-for="listing in filteredBrowseListings"
                  :key="listing.platform_item_id"
                  class="flex items-center gap-3 border border-border/20 p-3 hover:border-accent/30 transition-colors"
                >
                  <div class="w-10 h-10 shrink-0 rounded overflow-hidden bg-surface/30">
                    <img
                      v-if="listing.image_url"
                      :src="resolveThumb(listing.image_url) ?? listing.image_url"
                      :alt="listing.title"
                      class="w-full h-full object-cover"
                      loading="lazy"
                      @error="($event.target as HTMLImageElement).style.display = 'none'"
                    >
                  </div>
                  <div class="min-w-0 flex-1">
                    <p class="text-sm text-foreground truncate">{{ listing.title }}</p>
                    <p class="text-[11px] text-muted">
                      {{ listing.platform_item_id }}
                      <span v-if="listing.sku"> / {{ listing.sku }}</span>
                      / Qty: {{ listing.quantity }}
                      <span v-if="listing.price != null"> / ${{ listing.price.toFixed(2) }}</span>
                    </p>
                  </div>
                  <button class="btn-sm text-accent hover:text-accent/80 ml-3" @click="linkListing(listing)">Link</button>
                </div>
              </div>
              <p v-else-if="browseSearch && addMappingListings.length > 0" class="text-sm text-muted">No match.</p>
              <p v-else-if="!addMappingLoading && !addMappingError && addMappingListings.length === 0" class="text-sm text-muted/60 py-4 text-center">Click "Fetch Listings" to browse.</p>
            </template>

            <template v-else>
              <div class="space-y-3">
                <div>
                  <label class="label-sm block mb-1.5">Platform Item ID</label>
                  <input v-model="manualItemId" placeholder="e.g. SKU-123" class="input-field">
                </div>
                <div>
                  <label class="label-sm block mb-1.5">Platform SKU (optional)</label>
                  <input v-model="manualSku" placeholder="Optional" class="input-field">
                </div>
                <button
                  class="btn-primary w-full disabled:opacity-40"
                  :disabled="!manualItemId.trim()"
                  @click="linkManual"
                >Link Mapping</button>
              </div>
            </template>

            <div class="flex justify-end pt-3 border-t border-border/20 shrink-0">
              <button class="text-sm text-muted hover:text-foreground transition-colors" @click="showAddMapping = false">Close</button>
            </div>
          </div>
        </div>
      </Transition>
    </Teleport>

    <!-- Confirm dialog -->
    <Teleport to="body">
      <Transition name="modal">
        <div v-if="confirmDialog.show" class="fixed inset-0 z-[60] flex items-center justify-center bg-black/40" @click.self="cancelConfirm">
          <GroundGlass :opacity="3" :blur="12" :sizes="['70%', '70%', '65%']" class="p-8 w-full max-w-sm" style="background: rgba(30, 41, 59, 0.35)">
            <div class="relative z-[1]">
              <h3 class="text-sm font-medium tracking-widest uppercase text-foreground/70 mb-4">{{ confirmDialog.title }}</h3>
              <p class="text-sm text-muted/60 mb-8">{{ confirmDialog.message }}</p>
              <div class="flex justify-end gap-3">
                <Button variant="solid" color="muted" size="sm" @click="cancelConfirm">Cancel</Button>
                <Button variant="solid" color="danger" size="sm" @click="confirmAction">Delete</Button>
              </div>
            </div>
          </GroundGlass>
        </div>
      </Transition>
    </Teleport>
  </div>
  </CompanyRequired>
</template>

<style scoped>
/* ════ Hero Layout ════ */
.hero-layout {
  display: grid;
  grid-template-columns: auto minmax(50%, 1fr);
  gap: 3rem;
}
@media (max-width: 1023px) {
  .hero-layout { grid-template-columns: 1fr; }
}

/* ════ Photo Grid ════ */
.photo-grid {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 6px;
}

.photo-frame {
  position: relative;
  overflow: hidden;
  background: #080c14;
}
.photo-frame img {
  width: 100%;
  aspect-ratio: 9 / 16;
  max-height: 800px;
  display: block;
  object-fit: cover;
}

.photo-badge {
  position: absolute;
  top: 8px;
  left: 8px;
  opacity: 0;
  transition: opacity 0.2s ease;
}
.photo-frame:hover .photo-badge { opacity: 1; }

.photo-empty {
  background: #080c14;
  aspect-ratio: 4 / 3;
  display: flex;
  align-items: center;
  justify-content: center;
}

/* ════ Typography ════ */
.label-sm {
  font-size: 0.65rem;
  font-weight: 500;
  text-transform: uppercase;
  letter-spacing: 0.1em;
  color: #64748b;
}

.section-title {
  font-size: 0.65rem;
  font-weight: 600;
  text-transform: uppercase;
  letter-spacing: 0.12em;
  color: #64748b;
}

/* ════ Detail rows ════ */
.detail-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
}

/* ════ Mapping rows ════ */
.mapping-row {
  display: flex;
  align-items: center;
  justify-content: space-between;
  padding: 0.75rem 0;
  border-bottom: 1px solid rgba(71, 85, 105, 0.15);
}


/* ════ Platform accordion ════ */
.platform-accordion {
  border: 1px solid rgba(71, 85, 105, 0.15);
  border-radius: 6px;
  overflow: hidden;
}

.platform-accordion-header {
  width: 100%;
  display: flex;
  align-items: center;
  padding: 0.625rem 0.75rem;
  background: rgba(255, 255, 255, 0.02);
  transition: background 0.15s ease;
  cursor: pointer;
}
.platform-accordion-header:hover {
  background: rgba(255, 255, 255, 0.04);
}

.platform-accordion-body {
  border-top: 1px solid rgba(71, 85, 105, 0.1);
  padding: 0 0.75rem;
}

/* Accordion slide transition */
.accordion-enter-active,
.accordion-leave-active {
  transition: all 0.2s ease;
  overflow: hidden;
}
.accordion-enter-from,
.accordion-leave-to {
  opacity: 0;
  max-height: 0;
  padding-top: 0;
  padding-bottom: 0;
}
.accordion-enter-to,
.accordion-leave-from {
  opacity: 1;
  max-height: 500px;
}

.variant-detail {
  background: rgba(255, 255, 255, 0.03);
  border: 1px solid rgba(71, 85, 105, 0.2);
  border-radius: 8px;
  padding: 0.875rem 1rem;
}

/* ════ Comparison grid ════ */
.comparison-grid {
  display: flex;
  flex-wrap: wrap;
  gap: 0;
}

/* ════ Comparison cards ════ */
.comparison-card {
  min-width: 320px;
  flex: 1 1 320px;
  overflow: hidden;
}
.comparison-card--divider {
  border-left: 1px solid rgba(71, 85, 105, 0.2);
}

.card-header {
  padding: 0.875rem 1.25rem;
  border-bottom: 1px solid rgba(71, 85, 105, 0.15);
  display: flex;
  align-items: center;
  justify-content: space-between;
}

.edit-listing-btn {
  display: inline-block;
  padding: 0.375rem 0.875rem;
  font-size: 0.75rem;
  font-weight: 500;
  border: 1px solid rgba(139, 92, 246, 0.35);
  color: #a78bfa;
  transition: all 0.15s ease;
}
.edit-listing-btn:hover {
  background: rgba(139, 92, 246, 0.1);
  border-color: rgba(139, 92, 246, 0.5);
  color: #c4b5fd;
}

.desc-box {
  font-size: 0.75rem;
  color: rgba(226, 232, 240, 0.75);
  background: rgba(15, 23, 42, 0.4);
  padding: 0.75rem;
  max-height: 12rem;
  overflow-y: auto;
  line-height: 1.6;
}

/* ════ Buttons ════ */
.btn-primary {
  height: 2.75rem;
  padding: 0 1.5rem;
  font-size: 0.8125rem;
  font-weight: 500;
  letter-spacing: 0.02em;
  background: #8b5cf6;
  color: white;
  transition: background 0.15s ease;
}
.btn-primary:hover { background: #7c3aed; }

.btn-outline {
  height: 2.75rem;
  padding: 0 1.25rem;
  font-size: 0.8125rem;
  font-weight: 500;
  border: 1px solid rgba(71, 85, 105, 0.4);
  color: #94a3b8;
  transition: all 0.15s ease;
}
.btn-outline:hover {
  color: #e2e8f0;
  border-color: rgba(71, 85, 105, 0.7);
}

.btn-sm {
  font-size: 0.75rem;
  font-weight: 500;
  transition: color 0.15s ease;
  white-space: nowrap;
}

/* ════ Inputs ════ */
.input-field {
  width: 100%;
  background: rgba(15, 23, 42, 0.6);
  border: 1px solid rgba(71, 85, 105, 0.35);
  padding: 0.625rem 0.75rem;
  font-size: 0.875rem;
  color: #e2e8f0;
  transition: border-color 0.15s ease;
  outline: none;
}
.input-field::placeholder { color: rgba(148, 163, 184, 0.3); }
.input-field:focus { border-color: #8b5cf6; }

/* ════ Transitions ════ */
.modal-enter-active { transition: opacity 0.2s ease; }
.modal-leave-active { transition: opacity 0.15s ease; }
.modal-enter-from,
.modal-leave-to { opacity: 0; }

/* ════ Prose (HTML descriptions) ════ */
:deep(.prose-listing) { font-size: 0.75rem; line-height: 1.6; }
:deep(.prose-listing h1),
:deep(.prose-listing h2),
:deep(.prose-listing h3),
:deep(.prose-listing h4) { font-weight: 600; margin-top: 0.75em; margin-bottom: 0.25em; }
:deep(.prose-listing h1) { font-size: 1.1em; }
:deep(.prose-listing h2) { font-size: 1em; }
:deep(.prose-listing h3) { font-size: 0.95em; }
:deep(.prose-listing p) { margin-bottom: 0.5em; }
:deep(.prose-listing ul),
:deep(.prose-listing ol) { padding-left: 1.25em; margin-bottom: 0.5em; }
:deep(.prose-listing ul) { list-style: disc; }
:deep(.prose-listing ol) { list-style: decimal; }
:deep(.prose-listing li) { margin-bottom: 0.15em; }
:deep(.prose-listing a) { text-decoration: underline; }
:deep(.prose-listing img) { max-width: 100%; margin: 0.5em 0; }
</style>
