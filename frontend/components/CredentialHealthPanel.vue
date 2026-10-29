<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { CredentialHealth, Platform, SyncEngineEvent } from '~/types'

const props = withDefaults(
  defineProps<{
    /**
     * 'alerts' (dashboard): render only platforms needing attention.
     * 'full' (settings): render every enabled platform with its expiry detail.
     */
    mode?: 'alerts' | 'full'
  }>(),
  { mode: 'alerts' },
)

const { activeCompanyId, activeCompany, companiesInitialized } = useCompanyContext()

const health = ref<CredentialHealth[]>([])
const loaded = ref(false)
// N-11.2: platforms whose refresh_auth failed mid-sync this session. Kept
// until the next health refresh clears them, so the failure is a persistent
// banner rather than a transient tooltip.
const authFailed = ref<Set<Platform>>(new Set())

const labels: Record<Platform, string> = {
  squarespace: 'Squarespace',
  ebay: 'eBay',
  xmrbazaar: 'XMR Bazaar',
  amazon: 'Amazon',
}

const visible = computed(() => {
  const list = health.value.filter(h => h.enabled)
  if (props.mode === 'full') return list
  return list.filter(h => h.status !== 'ok' || authFailed.value.has(h.platform))
})

function statusChip(h: CredentialHealth): { text: string; cls: string } {
  if (authFailed.value.has(h.platform)) {
    return { text: 'Auth failed', cls: 'text-red-400 border-red-500/30 bg-red-500/10' }
  }
  switch (h.status) {
    case 'expired':
      return { text: 'Expired', cls: 'text-red-400 border-red-500/30 bg-red-500/10' }
    case 'expiring_soon':
      return { text: 'Expiring soon', cls: 'text-amber-400 border-amber-500/30 bg-amber-500/10' }
    case 'attention':
      return { text: 'Needs attention', cls: 'text-yellow-400 border-yellow-500/30 bg-yellow-500/10' }
    default:
      return { text: 'Healthy', cls: 'text-green-400 border-green-500/30 bg-green-500/10' }
  }
}

function detailFor(h: CredentialHealth): string {
  if (authFailed.value.has(h.platform)) {
    return `Authentication failed during sync — ${labels[h.platform]} will not sync until you reconnect.`
  }
  return h.detail
}

async function fetchHealth() {
  try {
    health.value = await invoke<CredentialHealth[]>('get_credential_health')
    authFailed.value = new Set()
  } catch {
    health.value = []
  } finally {
    loaded.value = true
  }
}

// N-11.2: deep-link to the wizard step that renews this platform's credential.
function openWizardFor(platform: Platform) {
  navigateTo({
    path: '/setup',
    query: {
      companyId: activeCompanyId.value ?? '',
      companyName: activeCompany.value?.name ?? '',
      platform,
    },
  })
}

defineExpose({ refresh: fetchHealth })

onMounted(async () => {
  if (companiesInitialized.value) {
    await fetchHealth()
  }
  // Live mid-sync refresh failures surface here as a persistent banner,
  // not a silent tooltip change.
  await listen<SyncEngineEvent>('sync-event', (event) => {
    const e = event.payload
    if ('AuthExpired' in e) {
      authFailed.value = new Set(authFailed.value).add(e.AuthExpired.platform)
    } else if ('CycleComplete' in e) {
      void fetchHealth()
    }
  })
})

watch(companiesInitialized, async (ready) => {
  if (ready) await fetchHealth()
})

watch(activeCompanyId, async () => {
  if (companiesInitialized.value) await fetchHealth()
})
</script>

<template>
  <div v-if="loaded && visible.length > 0" class="space-y-3">
    <div
      v-for="h in visible"
      :key="h.platform"
      class="flex items-start gap-4 px-4 py-3 rounded-lg border border-border/30 bg-background/40"
    >
      <PlatformBadge :platform="h.platform" icon-only class="mt-0.5 shrink-0" />
      <div class="flex-1 min-w-0">
        <div class="flex items-center gap-2 flex-wrap">
          <span class="text-sm font-medium text-foreground">{{ labels[h.platform] }}</span>
          <span
            class="text-[11px] px-2 py-0.5 rounded-full border"
            :class="statusChip(h).cls"
          >{{ statusChip(h).text }}</span>
        </div>
        <p class="text-xs text-muted/70 mt-1">{{ detailFor(h) }}</p>
        <p
          v-if="props.mode === 'full' && h.refresh_token_expires_at"
          class="text-[11px] text-muted/40 mt-1 tabular-nums"
        >
          Refresh credential expires: {{ h.refresh_token_expires_at }}
        </p>
      </div>
      <button
        v-if="h.status !== 'ok' || authFailed.has(h.platform)"
        class="shrink-0 text-xs font-medium px-3 py-1.5 rounded border border-accent/40 text-accent hover:bg-accent/10 transition-colors"
        @click="openWizardFor(h.platform)"
      >
        Renew in setup wizard
      </button>
    </div>
  </div>
</template>
