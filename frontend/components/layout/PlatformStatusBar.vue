<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { Platform, PlatformHealth, SyncEngineEvent } from '~/types'

const { activeCompanyId, companiesInitialized } = useCompanyContext()

const health = ref<PlatformHealth[]>([])
const lastError = ref<Record<string, string>>({})

const labels: Record<Platform, string> = {
  squarespace: 'Squarespace',
  ebay: 'eBay',
  xmrbazaar: 'XMR Bazaar',
  amazon: 'Amazon',
}

async function fetchHealth() {
  try {
    health.value = await invoke<PlatformHealth[]>('get_platform_health')
  } catch {
    health.value = []
  }
}

function statusDot(p: PlatformHealth): string {
  if (!p.enabled) return 'bg-white/20'
  if (lastError.value[p.platform]) return 'bg-red-400'
  if (p.authenticated) return 'bg-green-400'
  return 'bg-yellow-400'
}

function statusLabel(p: PlatformHealth): string {
  if (!p.enabled) return `${labels[p.platform]}: Disabled`
  if (lastError.value[p.platform]) return `${labels[p.platform]}: ${lastError.value[p.platform]}`
  if (p.authenticated) return `${labels[p.platform]}: Connected`
  return `${labels[p.platform]}: Not authenticated`
}

onMounted(async () => {
  if (companiesInitialized.value) {
    await fetchHealth()
  }

  await listen<SyncEngineEvent>('sync-event', (event) => {
    const e = event.payload
    if ('PlatformError' in e) {
      lastError.value[e.PlatformError.platform] = e.PlatformError.message
      const plat = e.PlatformError.platform
      setTimeout(() => { delete lastError.value[plat] }, 60000)
    } else if ('AuthExpired' in e) {
      const plat = e.AuthExpired.platform
      const h = health.value.find(p => p.platform === plat)
      if (h) h.authenticated = false
    } else if ('CycleComplete' in e) {
      lastError.value = {}
      fetchHealth()
    }
  })
})

watch(companiesInitialized, async (ready) => {
  if (ready) await fetchHealth()
})

watch(activeCompanyId, async () => {
  if (companiesInitialized.value) {
    lastError.value = {}
    await fetchHealth()
  }
})
</script>

<template>
  <div v-if="health.length > 0" class="flex items-center gap-3 px-6 py-1.5 shrink-0">
    <div
      v-for="p in health"
      :key="p.platform"
      class="group relative flex items-center gap-1.5"
      :class="p.enabled ? '' : 'opacity-40'"
    >
      <span
        class="block w-1.5 h-1.5 rounded-full shrink-0"
        :class="statusDot(p)"
      />
      <PlatformBadge :platform="p.platform" icon-only />
      <span class="status-tooltip">{{ statusLabel(p) }}</span>
    </div>
  </div>
</template>

<style scoped>
.status-tooltip {
  position: absolute;
  bottom: 100%;
  left: 50%;
  transform: translateX(-50%);
  margin-bottom: 6px;
  padding: 4px 10px;
  background: #1a1a2e;
  color: #e2e8f0;
  font-size: 11px;
  white-space: nowrap;
  border-radius: 4px;
  pointer-events: none;
  opacity: 0;
  transition: opacity 0.15s ease;
  z-index: 50;
}

.group:hover .status-tooltip {
  opacity: 1;
}
</style>
