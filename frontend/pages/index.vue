<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import type { PlatformMapping } from '~/types'

const { products, loading, fetchProducts } = useProducts()
const { isRunning, fetchStatus, liveEvents, startListening } = useSync()
const { capabilities, fetchCapabilities } = useConfig()
const { companiesInitialized } = useCompanyContext()

const capabilitiesLoaded = ref(false)

startListening()

async function loadDashboardData() {
  if (!companiesInitialized.value) return
  await Promise.all([
    fetchProducts(),
    fetchStatus(),
    fetchCapabilities(),
  ])
  capabilitiesLoaded.value = true
}

onMounted(loadDashboardData)
watch(companiesInitialized, loadDashboardData)

const mappingCounts = ref<Record<string, number>>({})

watch(products, async () => {
  for (const p of products.value) {
    const mappings = await invoke<PlatformMapping[]>('get_mappings', { productId: p.id })
    mappingCounts.value[p.id] = mappings.length
  }
}, { immediate: false })

const lowStockProducts = computed(() =>
  products.value.filter(p =>
    p.low_stock_threshold && p.quantity <= p.low_stock_threshold
  )
)

const enabledPlatforms = computed(() => Object.keys(capabilities.value))

const syncStatus = computed(() => isRunning.value ? 'Active' : 'Paused')

function formatEvent(event: any): string {
  if ('CycleComplete' in event) {
    return `Sync cycle completed — ${event.CycleComplete.products_synced} products, ${event.CycleComplete.changes_pushed} changes`
  }
  if ('QuantityUpdated' in event) {
    const e = event.QuantityUpdated
    return `${e.product_name} updated on ${e.platform}: ${e.old_quantity} → ${e.new_quantity}`
  }
  if ('PlatformError' in event) {
    return `Error on ${event.PlatformError.platform}: ${event.PlatformError.message}`
  }
  if ('AuthExpired' in event) {
    return `Auth expired for ${event.AuthExpired.platform}`
  }
  if ('UnmappedListingsDetected' in event) {
    return `${event.UnmappedListingsDetected.count} unmapped listings detected`
  }
  return JSON.stringify(event)
}
</script>

<template>
  <CompanyRequired>
  <div class="flex flex-col h-full">
    <!-- Page header (pinned) -->
    <div class="shrink-0 pb-8 max-w-4xl">
      <h2 class="text-sm font-medium tracking-widest uppercase text-muted/60 mb-10">Dashboard</h2>

      <!-- Metrics -->
      <div class="flex gap-16">
      <div>
        <span class="text-5xl font-light text-foreground tabular-nums">{{ products.length }}</span>
        <p class="text-xs text-muted/60 mt-1 tracking-wide uppercase">Products</p>
      </div>
      <div>
        <span class="text-5xl font-light text-foreground tabular-nums">{{ enabledPlatforms.length }}</span>
        <p class="text-xs text-muted/60 mt-1 tracking-wide uppercase">Platforms</p>
      </div>
      <div>
        <span
          class="text-5xl font-light tabular-nums"
          :class="lowStockProducts.length > 0 ? 'text-warning' : 'text-foreground'"
        >{{ lowStockProducts.length }}</span>
        <p class="text-xs text-muted/60 mt-1 tracking-wide uppercase">Low stock</p>
      </div>
      <div>
        <span
          class="text-5xl font-light"
          :class="isRunning ? 'text-green-400' : 'text-yellow-400'"
        >{{ syncStatus }}</span>
        <p class="text-xs text-muted/60 mt-1 tracking-wide uppercase">Sync</p>
      </div>
      </div>
    </div>

    <!-- Scrollable content -->
    <div class="flex-1 overflow-auto min-h-0 -mr-6 pr-6">

    <!-- No platforms configured banner -->
    <div
      v-if="capabilitiesLoaded && enabledPlatforms.length === 0"
      class="mb-10 px-4 py-3 rounded-lg border border-yellow-500/20 bg-yellow-500/[0.05]"
    >
      <p class="text-xs text-yellow-400/80">
        No platforms configured. Set up your platforms to start syncing inventory.
        <NuxtLink to="/config" class="text-accent hover:text-accent/80 ml-1">Configure Platforms</NuxtLink>
      </p>
    </div>

    <!-- Low stock alerts -->
    <div v-if="lowStockProducts.length > 0" class="mb-16">
      <h3 class="text-xs font-medium tracking-widest uppercase text-warning/80 mb-4">Low stock</h3>
      <div class="space-y-2">
        <div
          v-for="p in lowStockProducts"
          :key="p.id"
          class="flex items-center justify-between py-1"
        >
          <span class="text-sm text-foreground">{{ p.name }}</span>
          <QuantityBadge :quantity="p.quantity" :threshold="p.low_stock_threshold" />
        </div>
      </div>
    </div>

    <!-- Products -->
    <div class="mb-16">
      <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Products</h3>

      <div v-if="loading" class="py-8 text-sm text-muted/40">Loading...</div>
      <div v-else-if="products.length === 0" class="py-8 text-sm text-muted/40">
        No products yet. Go to Products to create one.
      </div>
      <table v-else class="w-full text-sm">
        <thead>
          <tr class="text-left text-[11px] text-muted/40 uppercase tracking-wider">
            <th class="pb-3 font-medium">Name</th>
            <th class="pb-3 font-medium">SKU</th>
            <th class="pb-3 font-medium">Qty</th>
            <th class="pb-3 font-medium">Mappings</th>
            <th class="pb-3 font-medium">Updated</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="product in products"
            :key="product.id"
            class="border-t border-border/20 hover:bg-white/[0.02] cursor-pointer transition-colors"
            @click="navigateTo(`/products/${product.id}`)"
          >
            <td class="py-2.5 text-foreground">{{ product.name }}</td>
            <td class="py-2.5 text-muted/60 font-mono text-xs">{{ product.canonical_sku }}</td>
            <td class="py-2.5">
              <QuantityBadge :quantity="product.quantity" :threshold="product.low_stock_threshold" />
            </td>
            <td class="py-2.5 text-muted/60">{{ mappingCounts[product.id] ?? '—' }}</td>
            <td class="py-2.5 text-muted/40 text-xs">{{ product.updated_at }}</td>
          </tr>
        </tbody>
      </table>
    </div>

    <!-- Recent activity -->
    <div v-if="liveEvents.length > 0">
      <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Recent activity</h3>
      <div class="space-y-0">
        <p
          v-for="(event, i) in liveEvents.slice(0, 8)"
          :key="i"
          class="text-xs text-muted/50 py-1.5 border-t border-border/10 font-mono"
        >
          {{ formatEvent(event) }}
        </p>
      </div>
    </div>
    </div>
  </div>
  </CompanyRequired>
</template>
