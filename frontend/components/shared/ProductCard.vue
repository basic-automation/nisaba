<script setup lang="ts">
import type { Platform } from '~/types'

const { resolveThumb } = useImageCache()

const props = defineProps<{
  name: string
  sku: string
  quantity: number
  lowStockThreshold?: number | null
  imageUrl?: string | null
  platformInfo: { platform: Platform; price: number | null }[]
  variantCount?: number
  lastRefreshed?: string | null
}>()

function formatPrice(price: number | null): string {
  if (price === null || price === undefined) return ''
  return `$${price.toFixed(2)}`
}

const freshnessLabel = computed(() => {
  if (!props.lastRefreshed) return null
  const now = Date.now()
  const then = new Date(props.lastRefreshed + 'Z').getTime()
  if (isNaN(then)) return null
  const diffMs = now - then
  const hours = diffMs / (1000 * 60 * 60)
  if (hours < 1) return null // fresh, no indicator
  if (hours < 24) return `${Math.floor(hours)}h ago`
  const days = Math.floor(hours / 24)
  if (days < 7) return `${days}d ago`
  return `${days}d ago`
})

const freshnessClass = computed(() => {
  if (!props.lastRefreshed) return ''
  const now = Date.now()
  const then = new Date(props.lastRefreshed + 'Z').getTime()
  if (isNaN(then)) return ''
  const hours = (now - then) / (1000 * 60 * 60)
  if (hours < 1) return ''
  if (hours < 24) return 'text-muted/30'
  if (hours < 168) return 'text-yellow-500/50'
  return 'text-orange-400/60'
})
</script>

<template>
  <GroundGlass class="group flex flex-col flex-1 product-card">
    <div class="card-content">
      <!-- Photo -->
      <div class="aspect-square rounded-md overflow-hidden bg-surface/30 mb-3">
        <img
          v-if="imageUrl"
          :src="resolveThumb(imageUrl) ?? imageUrl"
          :alt="name"
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

      <!-- Info -->
      <h4 :title="name" class="text-sm text-foreground leading-snug line-clamp-2 mb-1 group-hover:text-accent transition-colors">
        {{ name }}
      </h4>

      <!-- Platform badges + prices -->
      <div class="flex flex-wrap gap-x-3 text-xs text-muted/60 mb-1 min-h-[1.25rem]">
        <span
          v-for="info in platformInfo"
          :key="info.platform"
          class="flex items-center gap-1"
        >
          <PlatformBadge :platform="info.platform" icon-only />
          <span v-if="info.price != null">{{ formatPrice(info.price) }}</span>
        </span>
      </div>

      <div class="flex items-center justify-between text-xs mt-auto">
        <div class="flex items-center gap-1.5 min-w-0">
          <span class="font-mono text-muted/30 truncate max-w-[100px]">{{ sku }}</span>
          <span
            v-if="variantCount && variantCount > 0"
            class="text-[10px] text-accent/60 bg-accent/10 px-1.5 py-0.5 shrink-0"
          >{{ variantCount }} var</span>
        </div>
        <div class="flex items-center gap-1.5">
          <span v-if="freshnessLabel" :class="['text-[10px]', freshnessClass]">{{ freshnessLabel }}</span>
          <QuantityBadge :quantity="quantity" :threshold="lowStockThreshold" />
        </div>
      </div>
    </div>
  </GroundGlass>
</template>

<style scoped>
.product-card {
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
</style>
