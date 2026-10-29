<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import type { PlatformMapping } from '~/types'

const { products, loading, fetchProducts, getMappingsBatch } = useProducts()
const {
  isRunning,
  fetchStatus,
  liveEvents,
  previewActive,
  previewRan,
  previewChanges,
  lastCycleReport,
  previewSync,
  startListening,
} = useSync()
const { capabilities, fetchCapabilities } = useConfig()
const { companiesInitialized } = useCompanyContext()

// N-11.6: "Preview sync" runs a dry-run cycle; failures to even start it
// surface inline rather than silently.
const previewError = ref<string | null>(null)

async function handlePreviewSync() {
  if (previewActive.value) return
  previewError.value = null
  try {
    await previewSync()
  } catch (e: any) {
    previewError.value = `Couldn't start the sync preview: ${e?.message ?? e ?? 'Unknown error'}`
  }
}

const capabilitiesLoaded = ref(false)
// N-11.3: the Promise.all below used to reject silently — surface load failures.
const dashboardError = ref<string | null>(null)

startListening()

async function loadDashboardData() {
  if (!companiesInitialized.value) return
  dashboardError.value = null
  try {
    await Promise.all([
      fetchProducts(),
      fetchStatus(),
      fetchCapabilities(),
    ])
  } catch (e: any) {
    dashboardError.value = String(e?.message ?? e ?? 'Unknown error')
  }
  capabilitiesLoaded.value = true
}

onMounted(loadDashboardData)
watch(companiesInitialized, loadDashboardData)

const mappingCounts = ref<Record<string, number>>({})

// N-11.7: one batch IPC call instead of a sequential per-product loop.
watch(products, async () => {
  const ids = products.value.map((p) => p.id)
  if (ids.length === 0) {
    mappingCounts.value = {}
    return
  }
  try {
    const batch = await getMappingsBatch(ids)
    const counts: Record<string, number> = {}
    for (const p of products.value) counts[p.id] = batch[p.id]?.length ?? 0
    mappingCounts.value = counts
  } catch {
    // Leave the previous counts; the mappings column shows '—' for unknowns.
  }
}, { immediate: false })

const lowStockProducts = computed(() =>
  products.value.filter(p =>
    p.low_stock_threshold && p.quantity <= p.low_stock_threshold
  )
)

const enabledPlatforms = computed(() => Object.keys(capabilities.value))

const syncStatus = computed(() => isRunning.value === null ? '…' : isRunning.value ? 'Active' : 'Paused')

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
  if ('DryRunStarted' in event) {
    return 'Sync preview started'
  }
  if ('DryRunChange' in event) {
    const e = event.DryRunChange
    return `Preview: ${e.product_name} on ${e.platform}: ${e.current_quantity} → ${e.planned_quantity} (not applied)`
  }
  if ('DryRunComplete' in event) {
    const n = event.DryRunComplete.planned_changes
    return `Sync preview complete — ${n} planned change${n === 1 ? '' : 's'}`
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
          :class="isRunning === null ? 'text-muted/40' : isRunning ? 'text-green-400' : 'text-yellow-400'"
        >{{ syncStatus }}</span>
        <p class="text-xs text-muted/60 mt-1 tracking-wide uppercase">Sync</p>
      </div>
      </div>
    </div>

    <!-- Scrollable content -->
    <div class="flex-1 overflow-auto min-h-0 -mr-6 pr-6">

    <!-- N-11.3: load failure banner (was a silent Promise.all rejection) -->
    <div
      v-if="dashboardError"
      class="mb-10 px-4 py-3 rounded-lg border border-red-500/20 bg-red-500/[0.05]"
    >
      <p class="text-xs text-red-400/80">
        Couldn't load dashboard data: {{ dashboardError }}
        <button class="text-accent hover:text-accent/80 ml-1" @click="loadDashboardData">Retry</button>
      </p>
    </div>

    <!-- No platforms configured banner -->
    <div
      v-if="capabilitiesLoaded && enabledPlatforms.length === 0"
      class="mb-10 px-4 py-3 rounded-lg border border-yellow-500/20 bg-yellow-500/[0.05]"
    >
      <p class="text-xs text-yellow-400/80">
        No platforms configured. Set up your platforms to start syncing inventory.
        <button class="text-accent hover:text-accent/80 ml-1" @click="navigateTo('/settings?tab=platforms')">Configure Platforms</button>
      </p>
    </div>

    <!-- N-11.2: credential health — advance expiry warnings and persistent
         mid-sync auth-failure banners, each linking to the wizard step that
         renews the credential. Renders nothing when everything is healthy. -->
    <div class="mb-10">
      <CredentialHealthPanel mode="alerts" />
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

    <!-- N-11.6: sync preview + per-cycle reconciliation report -->
    <div class="mb-16">
      <div class="flex items-center justify-between mb-5">
        <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60">Sync</h3>
        <button
          class="text-xs px-3 py-1.5 rounded-lg border border-border/30 text-foreground hover:bg-white/[0.04] disabled:opacity-50 transition-colors"
          :disabled="previewActive"
          @click="handlePreviewSync"
        >
          {{ previewActive ? 'Previewing…' : 'Preview sync' }}
        </button>
      </div>

      <p v-if="previewError" class="text-xs text-red-400/80 mb-3">{{ previewError }}</p>

      <!-- Dry-run preview results -->
      <div v-if="previewActive" class="py-2 text-xs text-muted/50">
        Resolving what the next sync would change…
      </div>
      <div v-else-if="previewRan && previewChanges.length === 0" class="py-2 text-xs text-muted/50">
        Preview complete — nothing would change.
      </div>
      <div v-else-if="previewChanges.length > 0" class="mb-6">
        <p class="text-xs text-muted/60 mb-3">
          Preview — {{ previewChanges.length }} planned change{{ previewChanges.length === 1 ? '' : 's' }}
          (nothing was applied)
        </p>
        <div class="space-y-0">
          <p
            v-for="(change, i) in previewChanges.slice(0, 50)"
            :key="i"
            class="text-xs text-muted/50 py-1.5 border-t border-border/10 font-mono"
          >
            {{ change.product_name }} on {{ change.platform }}: {{ change.current_quantity }} → {{ change.planned_quantity }}
          </p>
          <p v-if="previewChanges.length > 50" class="text-xs text-muted/40 py-1.5">
            …and {{ previewChanges.length - 50 }} more
          </p>
        </div>
      </div>

      <!-- Per-cycle reconciliation report for the last real cycle -->
      <div v-if="lastCycleReport" class="rounded-lg border border-border/20 bg-white/[0.02] px-4 py-3">
        <p class="text-xs font-medium text-foreground mb-2">Last sync cycle</p>
        <div class="flex flex-wrap gap-x-6 gap-y-1 text-xs text-muted/60">
          <span><span class="text-foreground tabular-nums">{{ lastCycleReport.products_synced }}</span> products</span>
          <span><span class="text-foreground tabular-nums">{{ lastCycleReport.changes_pushed }}</span> changes pushed</span>
          <span v-for="(n, platform) in lastCycleReport.changes_by_platform" :key="platform">
            {{ platform }}: <span class="text-foreground tabular-nums">{{ n }}</span>
          </span>
        </div>
        <p v-if="lastCycleReport.unmapped_listings > 0" class="text-xs text-yellow-400/80 mt-2">
          {{ lastCycleReport.unmapped_listings }} unmapped listings detected
        </p>
        <div v-if="lastCycleReport.errors.length > 0" class="mt-2 space-y-1">
          <p
            v-for="(error, i) in lastCycleReport.errors"
            :key="i"
            class="text-xs text-red-400/80 font-mono"
          >
            {{ error }}
          </p>
        </div>
      </div>
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
