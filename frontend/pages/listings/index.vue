<script setup lang="ts">
import type { Platform, PlatformListing, PlatformMapping, ProductVariant } from '~/types'
import { invoke } from '@tauri-apps/api/core'

const { products, fetchProducts, createProduct, createVariant, listVariants, getMappings, createMapping } = useProducts()
const { resolveThumb, ensureCached } = useImageCache()

const platforms: { key: Platform; label: string }[] = [
  { key: 'squarespace', label: 'Squarespace' },
  { key: 'ebay', label: 'eBay' },
  { key: 'xmrbazaar', label: 'XMR Bazaar' },
  { key: 'amazon', label: 'Amazon' },
]

const activePlatform = ref<Platform>('squarespace')

const platformTabItems = platforms.map(p => ({ label: p.label, value: p.key }))
const platformListings = ref<Record<Platform, PlatformListing[]>>({} as any)
const platformLoading = ref<Record<Platform, boolean>>({} as any)
const platformError = ref<Record<Platform, string>>({} as any)
const platformFetched = ref<Record<Platform, boolean>>({} as any)
const importSearch = ref('')
const hideImported = ref(true)
const hideZeroQty = ref(false)

// Sort
type SortMode = 'default' | 'qty-desc' | 'qty-asc' | 'price-desc' | 'price-asc' | 'alpha-asc' | 'alpha-desc'
const sortMode = ref<SortMode>('default')

const sortOptions = [
  { value: 'default', label: 'Default' },
  { value: 'qty-desc', label: 'Qty \u2193' },
  { value: 'qty-asc', label: 'Qty \u2191' },
  { value: 'price-desc', label: 'Price \u2193' },
  { value: 'price-asc', label: 'Price \u2191' },
  { value: 'alpha-asc', label: 'A\u2013Z' },
  { value: 'alpha-desc', label: 'Z\u2013A' },
]

// Mapping data (for imported detection)
const productMappings = ref<Record<string, PlatformMapping[]>>({})

// Grouping + expand/collapse animation
const expandedGroups = ref<Set<string>>(new Set())
const collapsingGroups = ref<Set<string>>(new Set())

interface ListingGroup {
  key: string
  groupKey: string | null
  title: string
  imageUrl: string | null
  variants: PlatformListing[]
  totalQty: number
}

// Link to existing product dialog
const showLinkDialog = ref(false)
const linkTarget = ref<{ mode: 'group' | 'single'; group: ListingGroup; listing?: PlatformListing } | null>(null)
const linkProductId = ref('')
const linkSearch = ref('')

const { notify } = useNotifications()
const { companiesInitialized } = useCompanyContext()

async function initListings() {
  if (!companiesInitialized.value) return
  await fetchProducts()
  // Load mappings and cached listings in parallel (instant, no API)
  await Promise.all([
    loadMappings(),
    loadCachedPlatformListings(activePlatform.value),
  ])
  // Then refresh from live API in background (non-blocking)
  fetchPlatformListings(activePlatform.value)
}

onMounted(initListings)
watch(companiesInitialized, initListings)

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

const mappedItemIds = computed(() => {
  const ids = new Set<string>()
  for (const mappings of Object.values(productMappings.value)) {
    for (const m of mappings) {
      ids.add(`${m.platform}:${m.platform_item_id}`)
    }
  }
  return ids
})

function isImported(platform: Platform, itemId: string) {
  return mappedItemIds.value.has(`${platform}:${itemId}`)
}

const filteredPlatformListings = computed(() => {
  let listings = platformListings.value[activePlatform.value] || []
  if (hideImported.value) {
    listings = listings.filter(l => !isImported(activePlatform.value, l.platform_item_id))
  }
  if (hideZeroQty.value) {
    listings = listings.filter(l => (l.quantity ?? 0) > 0)
  }
  if (importSearch.value) {
    const q = importSearch.value.toLowerCase()
    listings = listings.filter(l =>
      l.title.toLowerCase().includes(q) ||
      l.platform_item_id.toLowerCase().includes(q) ||
      (l.sku && l.sku.toLowerCase().includes(q))
    )
  }
  if (sortMode.value && sortMode.value !== 'default') {
    listings = [...listings].sort((a, b) => {
      switch (sortMode.value) {
        case 'qty-desc': return (b.quantity ?? 0) - (a.quantity ?? 0)
        case 'qty-asc': return (a.quantity ?? 0) - (b.quantity ?? 0)
        case 'price-desc': return (b.price ?? 0) - (a.price ?? 0)
        case 'price-asc': return (a.price ?? 0) - (b.price ?? 0)
        case 'alpha-asc': return a.title.localeCompare(b.title)
        case 'alpha-desc': return b.title.localeCompare(a.title)
        default: return 0
      }
    })
  }
  return listings
})

const groupedListings = computed<ListingGroup[]>(() => {
  const items = filteredPlatformListings.value
  const groupMap = new Map<string, PlatformListing[]>()
  const standalones: PlatformListing[] = []
  const groupOrder: string[] = []

  for (const item of items) {
    if (item.group_key) {
      if (!groupMap.has(item.group_key)) {
        groupMap.set(item.group_key, [])
        groupOrder.push(item.group_key)
      }
      groupMap.get(item.group_key)!.push(item)
    } else {
      standalones.push(item)
    }
  }

  const groups: ListingGroup[] = []

  for (const gk of groupOrder) {
    const variants = groupMap.get(gk)!
    const first = variants[0]
    groups.push({
      key: gk,
      groupKey: gk,
      title: first.title.replace(/\s*\[.*?\]\s*$/, '') || first.title,
      imageUrl: first.image_url,
      variants,
      totalQty: variants.reduce((sum, v) => sum + (v.quantity ?? 0), 0),
    })
  }

  for (const item of standalones) {
    groups.push({
      key: item.platform_item_id,
      groupKey: null,
      title: item.title,
      imageUrl: item.image_url,
      variants: [item],
      totalQty: item.quantity ?? 0,
    })
  }

  return groups
})

function isGroupExpanded(group: ListingGroup): boolean {
  return expandedGroups.value.has(group.key)
}

function isGroupCollapsing(group: ListingGroup): boolean {
  return collapsingGroups.value.has(group.key)
}

function toggleExpand(group: ListingGroup) {
  const s = new Set(expandedGroups.value)
  if (s.has(group.key)) {
    s.delete(group.key)
    collapsingGroups.value = new Set([...collapsingGroups.value, group.key])
  } else {
    s.add(group.key)
  }
  expandedGroups.value = s
}

function onCollapseAfterLeave(group: ListingGroup) {
  const s = new Set(collapsingGroups.value)
  s.delete(group.key)
  collapsingGroups.value = s
}

function groupImportState(group: ListingGroup): 'none' | 'partial' | 'all' {
  const count = group.variants.filter(v => isImported(activePlatform.value, v.platform_item_id)).length
  if (count === 0) return 'none'
  if (count === group.variants.length) return 'all'
  return 'partial'
}

function priceRange(group: ListingGroup): string {
  const prices = group.variants.map(v => v.price).filter((p): p is number => p !== null && p !== undefined)
  if (prices.length === 0) return ''
  const min = Math.min(...prices)
  const max = Math.max(...prices)
  if (min === max) return formatPrice(min)
  return `${formatPrice(min)} - ${formatPrice(max)}`
}

const unimportedCount = computed(() => {
  const listings = platformListings.value[activePlatform.value] || []
  return listings.filter(l => !isImported(activePlatform.value, l.platform_item_id)).length
})

const linkableProducts = computed(() => {
  if (!linkSearch.value) return products.value
  const q = linkSearch.value.toLowerCase()
  return products.value.filter(p =>
    p.name.toLowerCase().includes(q) || p.canonical_sku.toLowerCase().includes(q)
  )
})

async function loadCachedPlatformListings(platform: Platform) {
  try {
    const cached = await invoke<PlatformListing[]>('get_cached_platform_listings', { platform })
    if (cached.length > 0 && !platformFetched.value[platform]) {
      platformListings.value[platform] = cached
      platformFetched.value[platform] = true
      const urls = cached.map(l => l.image_url).filter(Boolean)
      if (urls.length > 0) ensureCached(urls)
    }
  } catch { /* no cache yet */ }
}

async function fetchPlatformListings(platform: Platform) {
  platformLoading.value[platform] = true
  platformError.value[platform] = ''
  try {
    const listings = await invoke<PlatformListing[]>('fetch_all_listings', { platform })
    platformListings.value[platform] = listings
    platformFetched.value[platform] = true
    const urls = listings.map(l => l.image_url).filter(Boolean)
    if (urls.length > 0) ensureCached(urls)
  } catch (e: any) {
    platformError.value[platform] = e?.toString() || 'Failed to fetch listings'
  } finally {
    platformLoading.value[platform] = false
  }
}

watch(activePlatform, async (platform) => {
  importSearch.value = ''
  expandedGroups.value = new Set()
  collapsingGroups.value = new Set()
  if (!platformFetched.value[platform]) {
    // Load cached first, then refresh live
    await loadCachedPlatformListings(platform)
  }
  fetchPlatformListings(platform)
})

async function importGroup(group: ListingGroup) {
  const first = group.variants[0]
  const canonicalSku = first.sku || first.platform_item_id
  try {
    // Create product + N variants + N platform_mappings
    const product = await createProduct(
      group.title,
      canonicalSku,
      group.totalQty,
    )
    const itemIds: string[] = []
    for (const variant of group.variants) {
      const newVariant = await createVariant(
        product.id,
        variant.sku || variant.platform_item_id,
        variant.title,
        variant.variant_attributes ?? {},
        variant.quantity ?? 0,
        variant.image_url ?? undefined,
      )
      await createMapping(product.id, activePlatform.value, variant.platform_item_id, newVariant.id, variant.sku ?? undefined)
      itemIds.push(variant.platform_item_id)
    }
    await Promise.all([fetchProducts(), loadMappings()])
    // Fire-and-forget: cache full listing detail for each imported variant
    for (const itemId of itemIds) {
      cacheListingDetail(product.id, activePlatform.value, itemId)
    }
    notify({ type: 'success', title: 'Group imported', message: `${group.title} — ${group.variants.length} variant(s)`, source: 'listings' })
  } catch (e: any) {
    const msg = String(e)
    const isSkuDupe = msg.includes('UNIQUE constraint failed') && msg.includes('canonical_sku')
    notify({
      type: 'error',
      title: 'Import failed',
      message: isSkuDupe
        ? `A product with SKU "${canonicalSku}" already exists. Try linking instead.`
        : msg,
      detail: msg,
      source: 'listings',
    })
  }
}

async function importSingleListing(listing: PlatformListing) {
  const canonicalSku = listing.sku || listing.platform_item_id
  try {
    const product = await createProduct(
      listing.title,
      canonicalSku,
      listing.quantity,
    )
    // Every mapping needs a variant — create one from the listing
    const variant = await createVariant(
      product.id,
      canonicalSku,
      listing.title,
      listing.variant_attributes ?? {},
      listing.quantity ?? 0,
      listing.image_url ?? undefined,
    )
    await createMapping(product.id, activePlatform.value, listing.platform_item_id, variant.id, listing.sku ?? undefined)
    await Promise.all([fetchProducts(), loadMappings()])
    // Fire-and-forget: cache full listing detail
    cacheListingDetail(product.id, activePlatform.value, listing.platform_item_id)
    notify({ type: 'success', title: 'Listing imported', message: listing.title, source: 'listings' })
  } catch (e: any) {
    const msg = String(e)
    const isSkuDupe = msg.includes('UNIQUE constraint failed') && msg.includes('canonical_sku')
    notify({
      type: 'error',
      title: 'Import failed',
      message: isSkuDupe
        ? `A product with SKU "${canonicalSku}" already exists. Try linking instead.`
        : msg,
      detail: msg,
      source: 'listings',
    })
  }
}

function cacheListingDetail(productId: string, platform: Platform, platformItemId: string) {
  invoke('cache_listing_detail', { productId, platform, platformItemId }).catch((e: unknown) => {
    console.warn('Failed to cache listing detail:', e)
  })
}

function openLinkGroupDialog(group: ListingGroup) {
  linkTarget.value = { mode: 'group', group }
  linkProductId.value = ''
  linkSearch.value = ''
  showLinkDialog.value = true
}

function openLinkSingleDialog(group: ListingGroup, listing: PlatformListing) {
  linkTarget.value = { mode: 'single', group, listing }
  linkProductId.value = ''
  linkSearch.value = ''
  showLinkDialog.value = true
}

async function handleLinkToProduct() {
  if (!linkProductId.value || !linkTarget.value) return
  const items = linkTarget.value.mode === 'single' && linkTarget.value.listing
    ? [linkTarget.value.listing]
    : linkTarget.value.group.variants

  // Fetch existing product variants to match by SKU or attributes
  const existingVariants = await listVariants(linkProductId.value)
  const variantBySku = new Map<string, ProductVariant>()
  for (const v of existingVariants) {
    variantBySku.set(v.sku.toLowerCase(), v)
  }

  function findByAttributes(attrs: Record<string, string> | undefined): ProductVariant | undefined {
    if (!attrs || Object.keys(attrs).length === 0) return undefined
    const normalized = new Map(
      Object.entries(attrs).map(([k, v]) => [k.toLowerCase().trim(), v.toLowerCase().trim()]),
    )
    return existingVariants.find(v => {
      const vAttrs = new Map(
        Object.entries(v.attributes).map(([k, val]) => [k.toLowerCase().trim(), val.toLowerCase().trim()]),
      )
      if (vAttrs.size === 0) return false
      for (const [key, val] of normalized) {
        if (vAttrs.get(key) !== val) return false
      }
      return true
    })
  }

  let linked = 0
  let created = 0
  let skipped = 0

  for (const item of items) {
    const itemSku = item.sku || item.platform_item_id
    const existing = variantBySku.get(itemSku.toLowerCase()) ?? findByAttributes(item.variant_attributes ?? undefined)

    if (existing) {
      // Match found — just create the mapping, no new variant needed
      try {
        await createMapping(linkProductId.value, activePlatform.value, item.platform_item_id, existing.id, item.sku ?? undefined)
        linked++
      } catch (e: any) {
        if (String(e).includes('UNIQUE constraint')) {
          skipped++
        } else {
          notify({ type: 'error', title: 'Link failed', message: String(e), detail: String(e), source: 'listings' })
          return
        }
      }
    } else {
      // No match — create new variant + mapping
      try {
        const newVariant = await createVariant(
          linkProductId.value,
          itemSku,
          item.title,
          item.variant_attributes ?? {},
          item.quantity ?? 0,
          item.image_url ?? undefined,
        )
        await createMapping(linkProductId.value, activePlatform.value, item.platform_item_id, newVariant.id, item.sku ?? undefined)
        created++
      } catch (e: any) {
        if (String(e).includes('UNIQUE constraint')) {
          skipped++
        } else {
          notify({ type: 'error', title: 'Link failed', message: String(e), detail: String(e), source: 'listings' })
          return
        }
      }
    }

    // Fire-and-forget: cache full listing detail
    cacheListingDetail(linkProductId.value, activePlatform.value, item.platform_item_id)
  }

  await Promise.all([fetchProducts(), loadMappings()])
  showLinkDialog.value = false
  linkTarget.value = null

  if (linked > 0 || created > 0 || skipped > 0) {
    const parts: string[] = []
    if (linked > 0) parts.push(`${linked} matched`)
    if (created > 0) parts.push(`${created} created`)
    if (skipped > 0) parts.push(`${skipped} skipped`)
    notify({ type: 'success', title: 'Listings linked', message: parts.join(', '), source: 'listings' })
  }
}

function formatPrice(price: number | null): string {
  if (price === null || price === undefined) return ''
  return `$${price.toFixed(2)}`
}
</script>

<template>
  <CompanyRequired>
  <div class="flex flex-col h-full">
    <!-- Header -->
    <div class="flex items-end justify-between mb-10 shrink-0">
      <h2 class="text-sm font-medium tracking-widest uppercase text-muted/60">Listings</h2>
    </div>

    <!-- Platform tabs + search + refresh -->
    <div class="flex items-center gap-6 mb-4 shrink-0">
      <TabBar v-model="activePlatform" :items="platformTabItems" />

      <Splatter trigger="focus" :opacity-ranges="[[0.08, 0.18], [0.08, 0.18], [0.03, 0.15]]" class="flex-1">
        <input
          v-model="importSearch"
          type="text"
          placeholder="Search listings..."
          class="relative z-[1] w-full bg-transparent border-b border-border/30 px-2 py-1.5 text-sm text-foreground placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
        >
      </Splatter>

      <Button
        variant="solid"
        color="default"
        size="sm"
        :disabled="platformLoading[activePlatform]"
        @click="fetchPlatformListings(activePlatform)"
      >
        {{ platformLoading[activePlatform] ? 'Refreshing...' : 'Refresh' }}
      </Button>
    </div>

    <!-- Filters + sort -->
    <div class="flex items-center gap-3 mb-6 shrink-0">
      <Button
        variant="solid"
        :color="hideImported ? 'accent' : 'muted'"
        size="xs"
        @click="hideImported = !hideImported"
      >
        Hide imported
      </Button>
      <Button
        variant="solid"
        :color="hideZeroQty ? 'accent' : 'muted'"
        size="xs"
        @click="hideZeroQty = !hideZeroQty"
      >
        Hide 0 qty
      </Button>

      <Select v-model="sortMode" class="ml-auto">
        <SelectTrigger class="sort-select" :clearable="sortMode !== 'default'" @clear="sortMode = 'default'">
          <span class="sort-select__label">Sort:</span>
          <SelectValue placeholder="Default" />
        </SelectTrigger>
        <SelectContent>
          <SelectItem v-for="s in sortOptions" :key="s.value" :value="s.value">{{ s.label }}</SelectItem>
        </SelectContent>
      </Select>
    </div>

    <!-- Status line -->
    <div class="flex items-center gap-3 text-[11px] text-muted/30 mb-6 shrink-0">
      <template v-if="platformFetched[activePlatform] && groupedListings.length > 0">
        <span>{{ groupedListings.length }} product lines ({{ filteredPlatformListings.length }} listings)</span>
        <span v-if="importSearch">matching "{{ importSearch }}"</span>
        <span>&middot;</span>
        <span>{{ unimportedCount }} not yet imported</span>
      </template>
    </div>

    <!-- Content area (scrollable) -->
    <div class="flex-1 overflow-auto min-h-0 -mr-6 pr-6">
      <!-- Loading (only show full-screen loader when we have NO cached listings) -->
      <div v-if="platformLoading[activePlatform] && !groupedListings.length" class="py-16 text-sm text-muted/40">
        Loading {{ platforms.find(p => p.key === activePlatform)?.label }} listings...
      </div>

      <!-- Background-refresh indicator (shown when refreshing with cached listings visible) -->
      <div v-if="platformLoading[activePlatform] && groupedListings.length > 0" class="mb-3 text-[11px] text-muted/30 animate-pulse">
        Refreshing listings...
      </div>

      <!-- Error banner (non-blocking — shown above cached listings if available) -->
      <div v-if="platformError[activePlatform]" class="mb-4 flex items-center gap-3 px-3 py-2 rounded-md bg-red-500/5 border border-red-400/15">
        <p class="text-sm text-red-400/80 flex-1">{{ platformError[activePlatform] }}</p>
        <button class="text-xs text-accent hover:text-accent/80 shrink-0" @click="fetchPlatformListings(activePlatform)">Retry</button>
        <button class="text-xs text-muted/40 hover:text-muted shrink-0" @click="platformError[activePlatform] = ''">Dismiss</button>
      </div>

      <!-- Grouped listing cards -->
      <div v-if="groupedListings.length > 0" class="listing-grid">
        <template v-for="group in groupedListings" :key="group.key">
          <!-- Standalone item (no group_key, single variant) -->
          <GroundGlass
            v-if="!group.groupKey"
            class="listing-card group flex flex-col"
            :class="isImported(activePlatform, group.variants[0].platform_item_id) ? 'opacity-40' : ''"
          >
            <div class="card-content">
              <!-- Photo -->
              <div class="aspect-square rounded-md overflow-hidden bg-surface/30 mb-3 relative">
                <img
                  v-if="group.variants[0].image_url"
                  :src="resolveThumb(group.variants[0].image_url) ?? group.variants[0].image_url"
                  :alt="group.variants[0].title"
                  class="w-full h-full object-cover opacity-90 group-hover:opacity-100 transition-opacity"
                  loading="lazy"
                  decoding="async"
                  @error="($event.target as HTMLImageElement).style.display = 'none'"
                >
                <div v-else class="w-full h-full flex items-center justify-center text-muted/15">
                  <svg class="w-10 h-10" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1" d="M4 16l4.586-4.586a2 2 0 012.828 0L16 16m-2-2l1.586-1.586a2 2 0 012.828 0L20 14m-6-6h.01M6 20h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z" />
                  </svg>
                </div>
                <span
                  v-if="isImported(activePlatform, group.variants[0].platform_item_id)"
                  class="absolute top-2 right-2 text-[10px] text-green-400/80 uppercase tracking-wider"
                >
                  Imported
                </span>
              </div>

              <!-- Info -->
              <h4 :title="group.variants[0].title" class="text-sm text-foreground leading-snug line-clamp-2 mb-1">
                {{ group.variants[0].title }}
              </h4>

              <div class="flex items-center gap-3 text-xs text-muted/40 mb-1">
                <span v-if="group.variants[0].price !== null" class="text-foreground/70">{{ formatPrice(group.variants[0].price) }}</span>
                <span>Qty: {{ group.variants[0].quantity ?? 0 }}</span>
              </div>

              <p class="text-[11px] text-muted/20 font-mono truncate mb-3">
                {{ group.variants[0].sku || group.variants[0].platform_item_id }}
              </p>

              <!-- Actions -->
              <div v-if="!isImported(activePlatform, group.variants[0].platform_item_id)" class="flex gap-3 mt-auto">
                <button
                  class="text-xs text-accent hover:text-accent/80 transition-colors"
                  @click="importSingleListing(group.variants[0])"
                >
                  Import
                </button>
                <button
                  class="text-xs text-muted/40 hover:text-muted transition-colors"
                  @click="openLinkSingleDialog(group, group.variants[0])"
                >
                  Link
                </button>
              </div>
            </div>
          </GroundGlass>

          <!-- Grouped product line card -->
          <GroundGlass
            v-else
            class="group-wrapper"
            :class="{
              'group-expanded': isGroupExpanded(group),
              'group-collapsing': isGroupCollapsing(group),
            }"
          >
            <div
              class="listing-card-inner group flex flex-col cursor-pointer"
              :class="{ 'opacity-40': groupImportState(group) === 'all' }"
              @click="toggleExpand(group)"
            >
              <div class="card-content">
                <!-- Image -->
                <div class="aspect-square rounded-md overflow-hidden bg-surface/30 mb-3 relative">
                  <img
                    v-if="group.imageUrl"
                    :src="resolveThumb(group.imageUrl) ?? group.imageUrl"
                    :alt="group.title"
                    class="w-full h-full object-cover opacity-90 group-hover:opacity-100 transition-opacity"
                    loading="lazy"
                    decoding="async"
                    @error="($event.target as HTMLImageElement).style.display = 'none'"
                  >
                  <div v-else class="w-full h-full flex items-center justify-center text-muted/15">
                    <svg class="w-10 h-10" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1" d="M4 16l4.586-4.586a2 2 0 012.828 0L16 16m-2-2l1.586-1.586a2 2 0 012.828 0L20 14m-6-6h.01M6 20h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z" />
                    </svg>
                  </div>

                  <!-- Variant count badge -->
                  <span class="absolute bottom-1.5 right-1.5 text-[10px] font-medium bg-black/60 text-white/90 px-1.5 py-0.5 rounded">
                    {{ group.variants.length }} variants
                  </span>
                </div>

                <!-- Info -->
                <h4 :title="group.title" class="text-sm text-foreground leading-snug line-clamp-2 mb-1">
                  {{ group.title }}
                </h4>

                <div class="flex items-center gap-3 text-xs text-muted/40 mb-1">
                  <span v-if="priceRange(group)" class="text-foreground/70">{{ priceRange(group) }}</span>
                  <span>Qty: {{ group.totalQty }}</span>
                </div>

                <!-- Import state badge -->
                <span
                  v-if="groupImportState(group) === 'all'"
                  class="text-[10px] text-green-400/80 uppercase tracking-wider mb-1"
                >Imported</span>
                <span
                  v-else-if="groupImportState(group) === 'partial'"
                  class="text-[10px] text-yellow-400/80 uppercase tracking-wider mb-1"
                >Partial</span>

                <!-- Actions -->
                <div class="mt-auto flex items-center gap-3 text-[11px] text-muted/30">
                  <button
                    v-if="groupImportState(group) !== 'all'"
                    class="text-xs text-accent hover:text-accent/80 transition-colors"
                    @click.stop="importGroup(group)"
                  >
                    Import
                  </button>
                  <button
                    class="text-xs text-muted/40 hover:text-muted transition-colors"
                    @click.stop="openLinkGroupDialog(group)"
                  >
                    Link
                  </button>
                </div>
              </div>
            </div>

            <!-- Expanded variant list + collapse -->
            <Transition name="expand-panel" @after-leave="onCollapseAfterLeave(group)">
              <div v-if="isGroupExpanded(group)" class="expand-content">
                <div class="variant-panel">
                  <div
                    v-for="variant in group.variants"
                    :key="variant.platform_item_id"
                    class="variant-row"
                    :class="{ 'opacity-40': isImported(activePlatform, variant.platform_item_id) }"
                  >
                    <!-- Variant thumbnail -->
                    <div class="variant-thumb">
                      <img
                        v-if="variant.image_url"
                        :src="resolveThumb(variant.image_url) ?? variant.image_url"
                        :alt="variant.title"
                        class="w-full h-full object-cover rounded-sm"
                        loading="lazy"
                        decoding="async"
                        @error="($event.target as HTMLImageElement).style.display = 'none'"
                      >
                      <div v-else class="w-full h-full flex items-center justify-center text-muted/15 rounded-sm bg-surface/30">
                        <svg class="w-4 h-4" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="1" d="M4 16l4.586-4.586a2 2 0 012.828 0L16 16m-2-2l1.586-1.586a2 2 0 012.828 0L20 14m-6-6h.01M6 20h12a2 2 0 002-2V6a2 2 0 00-2-2H6a2 2 0 00-2 2v12a2 2 0 002 2z" />
                        </svg>
                      </div>
                    </div>

                    <!-- Variant info -->
                    <div class="flex-1 min-w-0">
                      <p class="text-xs text-foreground/80 truncate">{{ variant.title }}</p>
                      <div class="flex items-center gap-3 text-[11px] text-muted/30">
                        <span v-if="variant.sku" class="font-mono truncate">{{ variant.sku }}</span>
                        <span v-if="variant.price !== null">{{ formatPrice(variant.price) }}</span>
                        <span>Qty: {{ variant.quantity ?? 0 }}</span>
                      </div>
                      <!-- Variant attributes -->
                      <div v-if="variant.variant_attributes && Object.keys(variant.variant_attributes).length > 0" class="flex flex-wrap gap-1.5 mt-1">
                        <span
                          v-for="(val, key) in variant.variant_attributes"
                          :key="key"
                          class="variant-attr-tag"
                        >{{ key }}: {{ val }}</span>
                      </div>
                    </div>

                    <!-- Imported badge / actions -->
                    <span
                      v-if="isImported(activePlatform, variant.platform_item_id)"
                      class="text-[10px] text-green-400/80 uppercase tracking-wider shrink-0"
                    >Imported</span>
                    <template v-else>
                      <button
                        class="text-[10px] text-accent hover:text-accent/80 transition-colors shrink-0"
                        @click="importSingleListing(variant)"
                      >Import</button>
                      <button
                        class="text-[10px] text-muted/40 hover:text-muted transition-colors shrink-0"
                        @click="openLinkSingleDialog(group, variant)"
                      >Link</button>
                    </template>
                  </div>
                </div>

                <button class="collapse-sidebar" @click="toggleExpand(group)">
                  <span>Collapse</span>
                  <svg class="w-3.5 h-3.5 rotate-90" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                    <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
                  </svg>
                </button>
              </div>
            </Transition>
          </GroundGlass>
        </template>
      </div>

      <!-- Empty states (only when not loading and no listings to show) -->
      <template v-if="!platformLoading[activePlatform] && groupedListings.length === 0">
        <div v-if="importSearch && (platformListings[activePlatform]?.length ?? 0) > 0" class="py-16 text-sm text-muted/40">
          No listings match "{{ importSearch }}".
        </div>
        <div v-else-if="platformFetched[activePlatform] && !platformError[activePlatform]" class="py-16 text-sm text-muted/40">
          No listings found on {{ platforms.find(p => p.key === activePlatform)?.label }}.
        </div>
      </template>
    </div>

    <!-- Link to Product Dialog -->
    <Teleport to="body">
      <div v-if="showLinkDialog && linkTarget" class="fixed inset-0 z-50 flex items-center justify-center bg-black/35" @click.self="showLinkDialog = false">
        <GroundGlass :opacity="3" :blur="10" :sizes="['70%', '70%', '65%']" class="p-8 w-full max-w-lg max-h-[80vh] flex flex-col" style="background: rgba(30, 41, 59, 0.30)">
          <div class="relative z-[1] flex flex-col flex-1 min-h-0">
            <h3 class="text-sm font-medium tracking-widest uppercase text-foreground/70 mb-2 shrink-0">Link to product</h3>
            <p class="text-sm text-muted/50 mb-1 shrink-0">
              <span class="text-foreground/80">{{ linkTarget.group.title }}</span>
            </p>
            <p class="text-[11px] text-muted/30 mb-5 shrink-0">
              {{ linkTarget.mode === 'single' ? '1 listing' : `${linkTarget.group.variants.length} variant(s)` }} will be matched by SKU or created
            </p>

            <div class="mb-4 shrink-0">
              <GlassInput v-model="linkSearch" placeholder="Search products..." />
            </div>

            <div class="overflow-auto flex-1 min-h-0 space-y-1">
              <button
                v-for="p in linkableProducts"
                :key="p.id"
                class="w-full text-left px-3 py-2.5 rounded-md text-sm transition-colors"
                :class="linkProductId === p.id ? 'bg-accent/10 text-accent' : 'hover:bg-white/[0.02] text-foreground'"
                @click="linkProductId = p.id"
              >
                <div class="text-sm">{{ p.name }}</div>
                <div class="text-[11px] text-muted/30 font-mono mt-0.5">{{ p.canonical_sku }} &middot; Qty: {{ p.quantity }}</div>
              </button>
              <div v-if="linkableProducts.length === 0" class="text-sm text-muted/30 text-center py-6">
                No products found.
              </div>
            </div>

            <div class="flex justify-end gap-4 mt-8 shrink-0">
              <Button variant="solid" color="muted" size="sm" @click="showLinkDialog = false">Cancel</Button>
              <Button variant="solid" color="accent" size="sm" :disabled="!linkProductId" @click="handleLinkToProduct">Link</Button>
            </div>
          </div>
        </GroundGlass>
      </div>
    </Teleport>
  </div>
  </CompanyRequired>
</template>

<style scoped>
.listing-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  gap: 1rem;
  align-content: start;
}

.group-wrapper {
  display: flex;
  flex-direction: column;
}

/* Expanded: spans full row, horizontal layout */
.group-expanded {
  grid-column: 1 / -1;
  flex-direction: row;
  align-items: stretch;
  animation: card-grow 0.4s cubic-bezier(0.4, 0, 0.2, 1);
}

/* Collapsing: keep full row while animating out */
.group-collapsing {
  grid-column: 1 / -1;
  flex-direction: row;
  align-items: stretch;
  animation: card-shrink 0.3s cubic-bezier(0.4, 0, 0.2, 1) forwards;
}

@keyframes card-grow {
  from { max-width: 280px; }
  to { max-width: 100%; }
}

@keyframes card-shrink {
  from { max-width: 100%; }
  to { max-width: 280px; }
}

.group-expanded .listing-card-inner,
.group-collapsing .listing-card-inner {
  width: 240px;
  flex-shrink: 0;
  height: auto;
}

.group-expanded .listing-card-inner .aspect-square,
.group-collapsing .listing-card-inner .aspect-square {
  max-height: 200px;
  aspect-ratio: auto;
}

.listing-card {
  padding: 0.875rem;
  padding-bottom: 1rem;
}

.listing-card-inner {
  padding: 0.875rem;
  padding-bottom: 1rem;
}

.card-content {
  position: relative;
  z-index: 1;
  display: flex;
  flex-direction: column;
  flex: 1;
}

/* Expand content wrapper (variant panel + collapse button) */
.expand-content {
  display: flex;
  flex: 1;
  min-width: 0;
  align-items: stretch;
  overflow: hidden;
}

/* Expand animation — grows width gradually */
.expand-panel-enter-active {
  transition: flex 0.4s cubic-bezier(0.4, 0, 0.2, 1), opacity 0.35s ease 0.05s;
}
.expand-panel-leave-active {
  transition: flex 0.3s cubic-bezier(0.4, 0, 0.2, 1), opacity 0.15s ease;
}
.expand-panel-enter-from {
  flex: 0.001;
  opacity: 0;
}
.expand-panel-enter-to {
  flex: 1;
  opacity: 1;
}
.expand-panel-leave-from {
  flex: 1;
  opacity: 1;
}
.expand-panel-leave-to {
  flex: 0.001;
  opacity: 0;
}

.variant-panel {
  display: flex;
  flex-direction: column;
  border-left: 1px solid hsl(var(--border) / 0.15);
  max-height: 400px;
  overflow-y: auto;
  flex: 1;
  min-width: 0;
  padding: 0.25rem 0;
}

.variant-row {
  display: flex;
  align-items: center;
  gap: 0.75rem;
  padding: 0.5rem 0.75rem;
  border-bottom: 1px solid hsl(var(--border) / 0.08);
  transition: background-color 0.1s;
}

.variant-row:hover {
  background-color: hsl(var(--accent) / 0.03);
}

.variant-thumb {
  width: 2rem;
  height: 2rem;
  flex-shrink: 0;
  overflow: hidden;
}

.variant-attr-tag {
  font-size: 10px;
  padding: 1px 6px;
  border-radius: 3px;
  background: hsl(var(--accent) / 0.08);
  color: hsl(var(--foreground) / 0.55);
  white-space: nowrap;
}

.collapse-sidebar {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 0.375rem;
  padding: 0 0.375rem;
  border-left: 1px solid hsl(var(--border) / 0.10);
  cursor: pointer;
  color: hsl(var(--muted) / 0.3);
  transition: color 0.15s, background-color 0.15s;
}

.collapse-sidebar:hover {
  background-color: hsl(var(--accent) / 0.05);
  color: hsl(var(--muted) / 0.6);
}

.collapse-sidebar span {
  writing-mode: vertical-lr;
  text-orientation: mixed;
  transform: rotate(180deg);
  font-size: 11px;
}

.sort-select {
  width: fit-content !important;
  gap: 6px;
  padding: 4px 12px;
  font-size: 12px;
  border: none !important;
}
.sort-select__label {
  text-transform: uppercase;
  letter-spacing: 0.05em;
  font-size: 11px;
  opacity: 0.5;
}
</style>
