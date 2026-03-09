<script setup lang="ts">
import type { TimeWindow, ProductAnalytics } from '~/types'

const { analytics, loading, fetchAllAnalytics } = useAnalytics()
const { companiesInitialized } = useCompanyContext()
const timeWindow = ref<TimeWindow>('Week')

async function initInventory() {
  if (!companiesInitialized.value) return
  await fetchAllAnalytics(timeWindow.value)
}

onMounted(initInventory)
watch(companiesInitialized, initInventory)

async function changeWindow(window: TimeWindow) {
  timeWindow.value = window
  await fetchAllAnalytics(window)
}

const windows: TimeWindow[] = ['Day', 'Week', 'Month', 'All']
const windowLabels: Record<TimeWindow, string> = {
  Day: '24h',
  Week: '7d',
  Month: '30d',
  All: 'All',
}

const totalStock = computed(() =>
  analytics.value.reduce((sum, a) => sum + a.current_quantity, 0)
)

const avgVelocity = computed(() => {
  if (analytics.value.length === 0) return '0.0'
  return (analytics.value.reduce((sum, a) => sum + a.sales_velocity, 0) / analytics.value.length).toFixed(1)
})
</script>

<template>
  <CompanyRequired>
  <div class="flex flex-col h-full">
    <!-- Header + time window -->
    <div class="flex items-end justify-between mb-10 shrink-0 max-w-4xl">
      <h2 class="text-sm font-medium tracking-widest uppercase text-muted/60">Inventory</h2>
      <nav class="flex items-center gap-4">
        <button
          v-for="w in windows"
          :key="w"
          class="text-xs pb-0.5 transition-colors"
          :class="timeWindow === w ? 'text-foreground border-b border-accent' : 'text-muted/30 hover:text-muted'"
          @click="changeWindow(w)"
        >
          {{ windowLabels[w] }}
        </button>
      </nav>
    </div>

    <div class="flex-1 overflow-auto min-h-0 -mr-6 pr-6">
    <div v-if="loading" class="py-16 text-sm text-muted/40">Loading analytics...</div>

    <template v-else-if="analytics.length > 0">
      <!-- Metrics -->
      <div class="flex gap-16 mb-16 max-w-4xl">
        <div>
          <span class="text-5xl font-light text-foreground tabular-nums">{{ analytics.length }}</span>
          <p class="text-xs text-muted/60 mt-1 tracking-wide uppercase">Products</p>
        </div>
        <div>
          <span class="text-5xl font-light text-foreground tabular-nums">{{ totalStock }}</span>
          <p class="text-xs text-muted/60 mt-1 tracking-wide uppercase">Total stock</p>
        </div>
        <div>
          <span class="text-5xl font-light text-foreground tabular-nums">{{ avgVelocity }}</span>
          <span class="text-lg font-light text-muted/40 ml-1">/day</span>
          <p class="text-xs text-muted/60 mt-1 tracking-wide uppercase">Avg velocity</p>
        </div>
      </div>

      <!-- Analytics table -->
      <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">By product</h3>
      <table class="w-full text-sm">
        <thead>
          <tr class="text-left text-[11px] text-muted/40 uppercase tracking-wider">
            <th class="pb-3 font-medium">Product</th>
            <th class="pb-3 font-medium">Qty</th>
            <th class="pb-3 font-medium">Velocity</th>
            <th class="pb-3 font-medium">Platform sales</th>
            <th class="pb-3 font-medium">History</th>
          </tr>
        </thead>
        <tbody>
          <tr
            v-for="a in analytics"
            :key="a.product_id"
            class="border-t border-border/20"
          >
            <td class="py-2.5 text-foreground">{{ a.product_name }}</td>
            <td class="py-2.5">
              <QuantityBadge :quantity="a.current_quantity" :threshold="a.low_stock_threshold" />
            </td>
            <td class="py-2.5 text-foreground/70 font-mono text-xs tabular-nums">
              {{ a.sales_velocity.toFixed(2) }}/day
            </td>
            <td class="py-2.5">
              <div class="flex items-center gap-3">
                <span
                  v-for="[platform, sold] in a.platform_sales"
                  :key="platform"
                  class="flex items-center gap-1.5 text-xs"
                >
                  <PlatformBadge :platform="platform" icon-only />
                  <span class="text-muted/50 tabular-nums">{{ sold }}</span>
                </span>
                <span v-if="a.platform_sales.length === 0" class="text-xs text-muted/20">—</span>
              </div>
            </td>
            <td class="py-2.5 text-muted/30 text-xs tabular-nums">{{ a.history.length }}</td>
          </tr>
        </tbody>
      </table>
    </template>

    <div v-else class="py-16 text-sm text-muted/40">
      No analytics data yet. Analytics populate after sync cycles run.
    </div>
    </div>
  </div>
  </CompanyRequired>
</template>
