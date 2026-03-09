<script setup lang="ts">
import type { VendorPluginInfo, VendorListing, VendorImportItem, VendorSyncEvent, VendorSyncBatchEvent } from '~/types'

const {
  plugins,
  loading,
  listings,
  listingLoading,
  listingError,
  syncStatus,
  installedPlugins,
  fetchPlugins,
  loadCachedListings,
  fetchSyncStatus,
  startSync,
  fetchVendorListings,
  importVendorListings,
  loadVariantSkus,
  isVendorItemImported,
} = useVendors()

const { products, fetchProducts, createVariant } = useProducts()
const { companiesInitialized } = useCompanyContext()

const activePlugin = ref<string>('')
const search = ref('')
const selectedIds = ref<Set<string>>(new Set())
const expandedGroups = ref<Set<string>>(new Set())
const collapsingGroups = ref<Set<string>>(new Set())
const importingState = ref(false)

const { notify } = useNotifications()
const { resolveThumb, ensureCached } = useImageCache()

// Filters
const hideImported = ref(false)
const hideZeroQty = ref(false)
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

// Link to existing product dialog
const showLinkDialog = ref(false)
const linkTarget = ref<{ mode: 'group' | 'variant'; group: ListingGroup; variant?: VendorListing } | null>(null)
const linkProductId = ref('')
const linkSearch = ref('')

const tabItems = computed(() =>
  installedPlugins.value.map(p => ({
    label: p.display_name,
    value: p.id,
  }))
)

interface ListingGroup {
  key: string
  groupKey: string | null
  title: string
  imageUrl: string | null
  variants: VendorListing[]
  totalQty: number
}

const filteredListings = computed(() => {
  if (!activePlugin.value) return []
  let all = listings.value[activePlugin.value] || []
  if (hideImported.value) {
    all = all.filter(l => !isVendorItemImported(l))
  }
  if (hideZeroQty.value) {
    all = all.filter(l => (l.quantity ?? 0) > 0)
  }
  if (search.value) {
    const q = search.value.toLowerCase()
    all = all.filter(l =>
      l.title.toLowerCase().includes(q) ||
      l.vendor_item_id.toLowerCase().includes(q) ||
      (l.sku && l.sku.toLowerCase().includes(q)) ||
      (l.group_key && l.group_key.toLowerCase().includes(q)) ||
      (l.variant_attributes && Object.values(l.variant_attributes).some(v => v.toLowerCase().includes(q)))
    )
  }
  if (sortMode.value && sortMode.value !== 'default') {
    all = [...all].sort((a, b) => {
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
  return all
})

const groupedListings = computed<ListingGroup[]>(() => {
  const items = filteredListings.value
  const groupMap = new Map<string, VendorListing[]>()
  const standalones: VendorListing[] = []
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
      title: first.title.replace(/\s*-\s*[^-]*$/, '') || first.title,
      imageUrl: first.image_url,
      variants,
      totalQty: variants.reduce((sum, v) => sum + (v.quantity ?? 0), 0),
    })
  }

  for (const item of standalones) {
    groups.push({
      key: item.vendor_item_id,
      groupKey: null,
      title: item.title,
      imageUrl: item.image_url,
      variants: [item],
      totalQty: item.quantity ?? 0,
    })
  }

  return groups
})

const totalVariantCount = computed(() => filteredListings.value.length)

// Cache vendor listing images to local disk whenever listings change
watch(filteredListings, (items) => {
  const urls = items.map(l => l.image_url).filter(Boolean)
  if (urls.length > 0) ensureCached(urls)
})

const selectedCount = computed(() => selectedIds.value.size)

function isGroupExpanded(group: ListingGroup): boolean {
  return expandedGroups.value.has(group.key)
}

/** True while the collapse animation is still playing */
function isGroupCollapsing(group: ListingGroup): boolean {
  return collapsingGroups.value.has(group.key)
}

function toggleExpand(group: ListingGroup) {
  const s = new Set(expandedGroups.value)
  if (s.has(group.key)) {
    s.delete(group.key)
    // Mark as collapsing so grid-column stays wide during leave animation
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

function groupSelectionState(group: ListingGroup): 'none' | 'partial' | 'all' {
  const ids = group.variants.map(v => v.vendor_item_id)
  const count = ids.filter(id => selectedIds.value.has(id)).length
  if (count === 0) return 'none'
  if (count === ids.length) return 'all'
  return 'partial'
}

function toggleGroupSelection(group: ListingGroup) {
  const s = new Set(selectedIds.value)
  const state = groupSelectionState(group)
  if (state === 'all') {
    for (const v of group.variants) s.delete(v.vendor_item_id)
  } else {
    for (const v of group.variants) s.add(v.vendor_item_id)
  }
  selectedIds.value = s
}

function toggleSelection(vendorItemId: string) {
  const s = new Set(selectedIds.value)
  if (s.has(vendorItemId)) {
    s.delete(vendorItemId)
  } else {
    s.add(vendorItemId)
  }
  selectedIds.value = s
}

function toggleSelectAll() {
  if (selectedIds.value.size === filteredListings.value.length) {
    selectedIds.value = new Set()
  } else {
    selectedIds.value = new Set(filteredListings.value.map(l => l.vendor_item_id))
  }
}

function formatVariantAttrs(listing: VendorListing): string {
  if (!listing.variant_attributes || Object.keys(listing.variant_attributes).length === 0) return ''
  return Object.values(listing.variant_attributes).join(' / ')
}

function priceRange(group: ListingGroup): string {
  const prices = group.variants.map(v => v.price).filter((p): p is number => p !== null && p !== undefined)
  if (prices.length === 0) return ''
  const min = Math.min(...prices)
  const max = Math.max(...prices)
  const currency = group.variants[0].currency
  if (min === max) return formatPrice(min, currency)
  return `${formatPrice(min, currency)} - ${formatPrice(max, currency)}`
}

const importPreview = computed(() => {
  if (selectedIds.value.size === 0) return null
  const all = listings.value[activePlugin.value] || []
  const selected = all.filter(l => selectedIds.value.has(l.vendor_item_id))
  const groups = new Map<string, VendorListing[]>()
  let standalone = 0
  for (const l of selected) {
    if (l.group_key) {
      const existing = groups.get(l.group_key) || []
      existing.push(l)
      groups.set(l.group_key, existing)
    } else {
      standalone++
    }
  }
  return { groups: groups.size, variants: selected.length - standalone, standalone, total: selected.length }
})

const linkableProducts = computed(() => {
  if (!linkSearch.value) return products.value
  const q = linkSearch.value.toLowerCase()
  return products.value.filter(p =>
    p.name.toLowerCase().includes(q) || p.canonical_sku.toLowerCase().includes(q)
  )
})

function openLinkGroupDialog(group: ListingGroup) {
  linkTarget.value = { mode: 'group', group }
  linkProductId.value = ''
  linkSearch.value = ''
  showLinkDialog.value = true
}

function openLinkVariantDialog(group: ListingGroup, variant: VendorListing) {
  linkTarget.value = { mode: 'variant', group, variant }
  linkProductId.value = ''
  linkSearch.value = ''
  showLinkDialog.value = true
}

async function handleLinkToProduct() {
  if (!linkProductId.value || !linkTarget.value) return
  const items = linkTarget.value.mode === 'variant' && linkTarget.value.variant
    ? [linkTarget.value.variant]
    : linkTarget.value.group.variants
  let linked = 0
  let skipped = 0
  for (const item of items) {
    try {
      await createVariant(
        linkProductId.value,
        item.sku || item.vendor_item_id,
        item.title,
        item.variant_attributes || {},
        item.quantity ?? 0,
        item.image_url ?? undefined,
      )
      linked++
    } catch (e: any) {
      // Skip duplicates (UNIQUE constraint), report other errors
      if (String(e).includes('UNIQUE constraint')) {
        skipped++
      } else {
        notify({ type: 'error', title: 'Link failed', message: String(e), detail: String(e), source: 'vendors' })
        return
      }
    }
  }
  showLinkDialog.value = false
  linkTarget.value = null
  await loadVariantSkus()
  notify({ type: 'success', title: 'Variants linked', message: `${linked} linked, ${skipped} already existed`, source: 'vendors' })
}

function groupImportState(group: ListingGroup): 'none' | 'partial' | 'all' {
  const count = group.variants.filter(v => isVendorItemImported(v)).length
  if (count === 0) return 'none'
  if (count === group.variants.length) return 'all'
  return 'partial'
}

const currentSyncStatus = computed(() => {
  if (!activePlugin.value) return null
  return syncStatus.value[activePlugin.value] || null
})

const isSyncing = computed(() => currentSyncStatus.value?.syncing ?? false)

const lastSyncedLabel = computed(() => {
  const status = currentSyncStatus.value
  if (!status?.last_synced_at) return null
  const ts = new Date(status.last_synced_at + 'Z')
  const now = new Date()
  const diffMs = now.getTime() - ts.getTime()
  const diffMin = Math.floor(diffMs / 60000)
  if (diffMin < 1) return 'just now'
  if (diffMin < 60) return `${diffMin}m ago`
  const diffHrs = Math.floor(diffMin / 60)
  if (diffHrs < 24) return `${diffHrs}h ago`
  const diffDays = Math.floor(diffHrs / 24)
  return `${diffDays}d ago`
})

// Listen for sync events from backend
useTauriEvent<VendorSyncEvent>('vendor-sync', (event) => {
  if (event.status === 'completed' || event.status === 'failed') {
    fetchSyncStatus(event.plugin_id)
  }
  if (event.status === 'completed' && event.plugin_id === activePlugin.value) {
    loadCachedListings(event.plugin_id)
  }
})

// Listen for progressive batch events during sync
useTauriEvent<VendorSyncBatchEvent>('vendor-sync-batch', (event) => {
  if (event.plugin_id !== activePlugin.value) return
  try {
    const batch: VendorListing[] = JSON.parse(event.listings_json)
    listings.value[event.plugin_id] = batch
  } catch { /* ignore parse errors */ }
})

async function initVendors() {
  if (!companiesInitialized.value) return
  // Only fetch plugins if not already loaded (state persists across navigations)
  if (plugins.value.length === 0) {
    await fetchPlugins()
  }
  await Promise.all([loadVariantSkus(), fetchProducts()])
  if (installedPlugins.value.length > 0 && !activePlugin.value) {
    activePlugin.value = installedPlugins.value[0].id
  }
}

onMounted(initVendors)
watch(companiesInitialized, initVendors)

watch(activePlugin, (id) => {
  search.value = ''
  selectedIds.value = new Set()
  expandedGroups.value = new Set()
  if (id) {
    // Only load from DB if not already in memory
    if (!listings.value[id]) {
      loadCachedListings(id)
    }
    if (!syncStatus.value[id]) {
      fetchSyncStatus(id)
    }
  }
})

async function handleSync() {
  if (!activePlugin.value || isSyncing.value) return
  await startSync(activePlugin.value)
  // Optimistically update status
  syncStatus.value[activePlugin.value] = {
    ...syncStatus.value[activePlugin.value],
    syncing: true,
    last_synced_at: syncStatus.value[activePlugin.value]?.last_synced_at ?? null,
    cached_count: syncStatus.value[activePlugin.value]?.cached_count ?? 0,
  }
}

async function handleImport() {
  if (!activePlugin.value || selectedIds.value.size === 0) return
  importingState.value = true
  try {
    const all = listings.value[activePlugin.value] || []
    const items: VendorImportItem[] = all
      .filter(l => selectedIds.value.has(l.vendor_item_id))
      .map(l => ({
        vendor_item_id: l.vendor_item_id,
        group_key: l.group_key ?? null,
        title: l.title,
        sku: l.sku || l.vendor_item_id,
        quantity: l.quantity ?? 0,
        price: l.price ?? null,
        currency: l.currency ?? null,
        image_url: l.image_url,
        url: l.url ?? null,
        extras: l.extras || {},
        variant_attributes: l.variant_attributes || {},
      }))
    const result = await importVendorListings(activePlugin.value, items)
    notify({ type: 'success', title: 'Import complete', message: `${result.products_created} products, ${result.variants_created} variants`, source: 'vendors' })
    selectedIds.value = new Set()
    await loadVariantSkus()
  } catch (e: any) {
    notify({ type: 'error', title: 'Import failed', message: String(e), detail: String(e), source: 'vendors' })
  } finally {
    importingState.value = false
  }
}

function formatPrice(price: number | null, currency: string | null): string {
  if (price === null || price === undefined) return ''
  const sym = currency === 'USD' || !currency ? '$' : currency + ' '
  return `${sym}${price.toFixed(2)}`
}

function openExternal(url: string) {
  window.open(url, '_blank')
}
</script>

<template>
  <CompanyRequired>
  <div class="flex flex-col h-full">
    <!-- Header -->
    <div class="flex items-end justify-between mb-10 shrink-0">
      <h2 class="text-sm font-medium tracking-widest uppercase text-muted/60">Vendors</h2>
      <div class="flex items-center gap-3">
        <Button
          v-if="selectedCount > 0"
          variant="solid"
          color="accent"
          size="sm"
          :disabled="importingState"
          @click="handleImport"
        >
          {{ importingState ? 'Importing...' : `Import ${selectedCount} selected` }}
        </Button>
        <NuxtLink to="/marketplace">
          <Button variant="solid" color="accent" size="sm">Plugin Marketplace</Button>
        </NuxtLink>
      </div>
    </div>

    <!-- Import preview -->
    <div v-if="importPreview && selectedCount > 0" class="text-[11px] text-muted/40 mb-4 shrink-0">
      {{ importPreview.groups }} product lines, {{ importPreview.variants }} variants, {{ importPreview.standalone }} standalone
    </div>

    <!-- No installed plugins -->
    <div v-if="!loading && installedPlugins.length === 0" class="flex flex-col items-center justify-center flex-1 text-center">
      <p class="text-sm text-muted/40 mb-4">No vendor plugins installed yet.</p>
      <NuxtLink to="/marketplace">
        <Button variant="solid" color="accent" size="sm">Browse Marketplace</Button>
      </NuxtLink>
    </div>

    <template v-else-if="installedPlugins.length > 0">
      <!-- Plugin tabs + search + sync -->
      <div class="flex items-center gap-6 mb-4 shrink-0">
        <TabBar v-model="activePlugin" :items="tabItems" />

        <Splatter trigger="focus" :opacity-ranges="[[0.08, 0.18], [0.08, 0.18], [0.03, 0.15]]" class="flex-1">
          <input
            v-model="search"
            type="text"
            placeholder="Search vendor listings..."
            class="relative z-[1] w-full bg-transparent border-b border-border/30 px-2 py-1.5 text-sm text-foreground placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
          >
        </Splatter>

        <Button
          variant="solid"
          color="default"
          size="sm"
          :disabled="isSyncing"
          @click="handleSync"
        >
          <span v-if="isSyncing" class="flex items-center gap-1.5">
            <svg class="w-3.5 h-3.5 animate-spin" fill="none" viewBox="0 0 24 24">
              <circle class="opacity-25" cx="12" cy="12" r="10" stroke="currentColor" stroke-width="4" />
              <path class="opacity-75" fill="currentColor" d="M4 12a8 8 0 018-8V0C5.373 0 0 5.373 0 12h4z" />
            </svg>
            Syncing...
          </span>
          <span v-else>Sync</span>
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

      <!-- Status + select all -->
      <div class="flex items-center gap-4 text-[11px] text-muted/30 mb-6 shrink-0">
        <template v-if="activePlugin && listings[activePlugin]">
          <span>{{ groupedListings.length }} product lines ({{ totalVariantCount }} variants)</span>
          <span v-if="search"> matching "{{ search }}"</span>
          <button class="text-accent/40 hover:text-accent transition-colors" @click="toggleSelectAll">
            {{ selectedIds.size === filteredListings.length && filteredListings.length > 0 ? 'Deselect all' : 'Select all' }}
          </button>
        </template>
        <span v-if="currentSyncStatus?.cached_count" class="text-muted/20">
          {{ currentSyncStatus.cached_count.toLocaleString() }} cached
        </span>
        <span v-if="lastSyncedLabel" class="text-muted/20">
          synced {{ lastSyncedLabel }}
        </span>
      </div>

      <!-- Content area -->
      <div class="flex-1 overflow-auto min-h-0 -mr-6 pr-6">
        <!-- Loading -->
        <div v-if="listingLoading[activePlugin]" class="py-16 text-sm text-muted/40">
          Loading vendor listings...
        </div>

        <!-- Error -->
        <div v-else-if="listingError[activePlugin]" class="py-8">
          <p class="text-sm text-red-400/80">{{ listingError[activePlugin] }}</p>
          <button class="text-xs text-accent mt-2 hover:text-accent/80" @click="handleSync">Retry sync</button>
        </div>

        <!-- Grouped listing cards -->
        <div v-else-if="groupedListings.length > 0" class="vendor-grid">
          <template v-for="group in groupedListings" :key="group.key">
            <!-- Standalone item (no group_key, single variant) — render as before -->
            <GroundGlass
              v-if="!group.groupKey"
              class="vendor-card group flex flex-col cursor-pointer"
              :class="{ 'ring-1 ring-accent/40': selectedIds.has(group.variants[0].vendor_item_id) }"
              @click="toggleSelection(group.variants[0].vendor_item_id)"
            >
              <div class="card-content">
                <div class="absolute top-2 right-2 z-10">
                  <div
                    class="w-5 h-5 border flex items-center justify-center transition-colors"
                    :class="selectedIds.has(group.variants[0].vendor_item_id)
                      ? 'border-accent bg-accent/20 text-accent'
                      : 'border-border/30 text-transparent group-hover:border-border/50'"
                  >
                    <svg v-if="selectedIds.has(group.variants[0].vendor_item_id)" class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                      <path stroke-linecap="round" stroke-linejoin="round" stroke-width="3" d="M5 13l4 4L19 7" />
                    </svg>
                  </div>
                </div>

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
                </div>

                <h4 :title="group.variants[0].title" class="text-sm text-foreground leading-snug line-clamp-2 mb-1">
                  {{ group.variants[0].title }}
                </h4>

                <div class="flex items-center gap-3 text-xs text-muted/40 mb-1">
                  <span v-if="group.variants[0].price !== null" class="text-foreground/70">
                    {{ formatPrice(group.variants[0].price, group.variants[0].currency) }}
                  </span>
                  <span v-if="group.variants[0].quantity !== null">Qty: {{ group.variants[0].quantity }}</span>
                </div>

                <p v-if="group.variants[0].sku" class="text-[11px] text-muted/20 font-mono truncate mb-1">
                  {{ group.variants[0].sku }}
                </p>

                <div class="mt-auto flex items-center gap-3">
                  <button
                    v-if="group.variants[0].url"
                    class="text-xs text-accent hover:text-accent/80 transition-colors"
                    @click.stop="openExternal(group.variants[0].url!)"
                  >
                    View source
                  </button>
                  <button
                    class="text-xs text-muted/40 hover:text-muted transition-colors"
                    @click.stop="openLinkGroupDialog(group)"
                  >
                    Link
                  </button>
                  <span
                    v-if="isVendorItemImported(group.variants[0])"
                    class="text-[10px] text-green-400/80 uppercase tracking-wider ml-auto"
                  >Imported</span>
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
                'ring-1 ring-accent/40': groupSelectionState(group) === 'all',
                'ring-1 ring-accent/20': groupSelectionState(group) === 'partial',
              }"
            >
              <div
                class="vendor-card-inner group flex flex-col cursor-pointer"
                @click="toggleExpand(group)"
              >
                <div class="card-content">
                  <!-- Selection checkbox -->
                  <div class="absolute top-2 right-2 z-10">
                    <div
                      class="w-5 h-5 border flex items-center justify-center transition-colors cursor-pointer"
                      :class="{
                        'border-accent bg-accent/20 text-accent': groupSelectionState(group) === 'all',
                        'border-accent/50 bg-accent/10 text-accent/60': groupSelectionState(group) === 'partial',
                        'border-border/30 text-transparent group-hover:border-border/50': groupSelectionState(group) === 'none',
                      }"
                      @click.stop="toggleGroupSelection(group)"
                    >
                      <svg v-if="groupSelectionState(group) === 'all'" class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="3" d="M5 13l4 4L19 7" />
                      </svg>
                      <svg v-else-if="groupSelectionState(group) === 'partial'" class="w-3 h-3" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                        <path stroke-linecap="round" stroke-width="3" d="M6 12h12" />
                      </svg>
                    </div>
                  </div>

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
                    <span>Qty: {{ group.totalQty.toLocaleString() }}</span>
                  </div>

                  <span class="inline-block text-[10px] text-accent/50 bg-accent/5 px-1.5 py-0.5 mb-2">
                    {{ group.groupKey }}
                  </span>

                  <!-- Import state badge -->
                  <span
                    v-if="groupImportState(group) === 'all'"
                    class="text-[10px] text-green-400/80 uppercase tracking-wider"
                  >Imported</span>
                  <span
                    v-else-if="groupImportState(group) === 'partial'"
                    class="text-[10px] text-yellow-400/80 uppercase tracking-wider"
                  >Partial</span>

                  <!-- Actions -->
                  <div class="mt-auto flex items-center gap-3 text-[11px] text-muted/30">
                    <button
                      class="text-xs text-muted/40 hover:text-muted transition-colors"
                      @click.stop="openLinkGroupDialog(group)"
                    >
                      Link
                    </button>
                    <div class="flex-1" />
                    <template v-if="!isGroupExpanded(group)">
                      <svg
                        class="w-3.5 h-3.5"
                        fill="none" stroke="currentColor" viewBox="0 0 24 24"
                      >
                        <path stroke-linecap="round" stroke-linejoin="round" stroke-width="2" d="M19 9l-7 7-7-7" />
                      </svg>
                      <span>Expand</span>
                    </template>
                  </div>
                </div>
              </div>

              <!-- Expanded variant list + collapse -->
              <Transition name="expand-panel" @after-leave="onCollapseAfterLeave(group)">
                <div v-if="isGroupExpanded(group)" class="expand-content">
                  <div class="variant-panel">
                    <div
                      v-for="variant in group.variants"
                      :key="variant.vendor_item_id"
                      class="variant-row cursor-pointer"
                      :class="{ 'bg-accent/5': selectedIds.has(variant.vendor_item_id) }"
                      @click="toggleSelection(variant.vendor_item_id)"
                    >
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

                      <div class="flex-1 min-w-0">
                        <p v-if="formatVariantAttrs(variant)" class="text-xs text-foreground/80 truncate">
                          {{ formatVariantAttrs(variant) }}
                        </p>
                        <p v-else class="text-xs text-foreground/80 truncate">{{ variant.title }}</p>
                        <div class="flex items-center gap-3 text-[11px] text-muted/30">
                          <span v-if="variant.sku" class="font-mono truncate">{{ variant.sku }}</span>
                          <span v-if="variant.price !== null">{{ formatPrice(variant.price, variant.currency) }}</span>
                          <span v-if="variant.quantity !== null">Qty: {{ variant.quantity }}</span>
                        </div>
                      </div>

                      <span
                        v-if="isVendorItemImported(variant)"
                        class="text-[10px] text-green-400/80 uppercase tracking-wider shrink-0"
                      >Imported</span>
                      <button
                        v-else
                        class="text-[10px] text-muted/40 hover:text-muted transition-colors shrink-0"
                        @click.stop="openLinkVariantDialog(group, variant)"
                      >Link</button>

                      <div
                        class="w-4 h-4 border flex items-center justify-center shrink-0 transition-colors"
                        :class="selectedIds.has(variant.vendor_item_id)
                          ? 'border-accent bg-accent/20 text-accent'
                          : 'border-border/30 text-transparent'"
                      >
                        <svg v-if="selectedIds.has(variant.vendor_item_id)" class="w-2.5 h-2.5" fill="none" stroke="currentColor" viewBox="0 0 24 24">
                          <path stroke-linecap="round" stroke-linejoin="round" stroke-width="3" d="M5 13l4 4L19 7" />
                        </svg>
                      </div>
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

        <!-- Empty state with sync prompt -->
        <div v-else-if="activePlugin && listings[activePlugin] && !isSyncing" class="flex flex-col items-center justify-center py-20">
          <p class="text-sm text-muted/40 mb-4">No cached listings. Click Sync to fetch from vendor.</p>
          <Button variant="solid" color="accent" size="sm" @click="handleSync">Sync Now</Button>
        </div>

        <!-- Empty during initial load (no cache yet, not loading) -->
        <div v-else-if="activePlugin && !listings[activePlugin] && !listingLoading[activePlugin] && !isSyncing" class="flex flex-col items-center justify-center py-20">
          <p class="text-sm text-muted/40 mb-4">No cached listings. Click Sync to fetch from vendor.</p>
          <Button variant="solid" color="accent" size="sm" @click="handleSync">Sync Now</Button>
        </div>
      </div>
    </template>
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
              {{ linkTarget.mode === 'variant' ? '1 variant' : `${linkTarget.group.variants.length} variant(s)` }} will be added to the selected product
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
.vendor-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(240px, 1fr));
  gap: 1rem;
  align-content: start;
  align-items: stretch;
}

.vendor-card {
  padding: 0.875rem;
  padding-bottom: 1rem;
  height: 100%;
}

.card-content {
  position: relative;
  z-index: 1;
  display: flex;
  flex-direction: column;
  flex: 1;
}

/* Inner card for grouped items (inside GroundGlass wrapper) */
.vendor-card-inner {
  padding: 0.875rem;
  padding-bottom: 1rem;
  height: 100%;
}

/* Wrapper for grouped cards (GroundGlass) */
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

.group-expanded .vendor-card-inner,
.group-collapsing .vendor-card-inner {
  width: 240px;
  flex-shrink: 0;
  height: auto;
}

.group-expanded .vendor-card-inner .aspect-square,
.group-collapsing .vendor-card-inner .aspect-square {
  max-height: 200px;
  aspect-ratio: auto;
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
  max-height: 420px;
  overflow-y: auto;
  flex: 1;
  min-width: 0;
  padding: 0.25rem 0;
}

.collapse-sidebar {
  display: flex;
  flex-direction: column;
  align-items: center;
  justify-content: center;
  gap: 0.375rem;
  width: 1.5rem;
  flex-shrink: 0;
  border-left: 1px solid hsl(var(--border) / 0.1);
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
