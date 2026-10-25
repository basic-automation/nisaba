<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'
import type { PlatformCapabilities } from '~/types'

const { activeCompanyId, activeCompany, companiesInitialized } = useCompanyContext()
const route = useRoute()

// N-2.7: the dedicated settings window (label `nisaba-settings`) is the only
// context allowed to invoke secret-returning Tauri commands. It hosts just the
// settings views; the target view arrives via the URL hash (#settings:config).
const inSettingsWindow = isSettingsWindow()
if (inSettingsWindow) {
  const { view, query } = parseSettingsHash()
  navigateTo({ path: view === 'setup' ? '/setup' : `/${view}`, query })
}

// Pre-load image cache manifest immediately so cached images resolve instantly
// on all pages without waiting for per-page IPC calls.
const { loadManifest } = useImageCache()
loadManifest()

// Force full destroy + recreate of all page components on company switch
// or backend init by toggling NuxtPage out of the DOM and back in.
const showPage = ref(true)

// Track which companies the user has dismissed the setup prompt for this session
const dismissedSetupIds = new Set<string>()

async function remountPages() {
  showPage.value = false
  await nextTick()
  showPage.value = true
}

/** Redirect to setup wizard if active company has no platforms configured. */
async function checkPlatformConfig() {
  // The settings window never nudges anywhere — the user opened it deliberately.
  if (inSettingsWindow) return
  const id = activeCompanyId.value
  const company = activeCompany.value
  if (!id || !company) return
  if (dismissedSetupIds.has(id)) return
  // Don't redirect if already on the setup page
  if (route.path === '/setup') return
  // Claim this company synchronously, BEFORE the async capability check
  // below: the two startup watchers (activeCompanyId, companiesInitialized)
  // can both fire in one tick, and without this the await lets both calls
  // through to open a duplicate settings window (which also breaks
  // WebDriver screenshots in CI). If capabilities turn out non-empty no
  // wizard is needed anyway, so claiming early changes nothing.
  dismissedSetupIds.add(id)
  try {
    const caps = await invoke<Record<string, PlatformCapabilities>>('get_platform_capabilities')
    if (Object.keys(caps).length === 0) {
      // N-11.1: completing or skipping the wizard is persisted per company —
      // don't auto-open it again for a company whose wizard already ran.
      try {
        if (await invoke<boolean>('get_setup_wizard_completed')) return
      } catch {
        // ignore; fall through to opening the wizard
      }
      // Platform credentials are secrets: configuring them happens in the
      // dedicated settings window, the only context allowed to read them.
      await openSettingsWindow('setup', { companyId: id, companyName: company.name })
    }
  } catch {
    // ignore
  }
}

watch(activeCompanyId, async (newId, oldId) => {
  // Clear dismissal when switching to a different company
  if (oldId && oldId !== newId) {
    dismissedSetupIds.delete(oldId)
  }
  await remountPages()
  if (companiesInitialized.value) {
    await nextTick()
    await checkPlatformConfig()
  }
})

watch(companiesInitialized, async (ready) => {
  if (ready) {
    await remountPages()
    await nextTick()
    await checkPlatformConfig()
  }
})
</script>

<template>
  <NuxtLayout :name="inSettingsWindow ? 'settings' : 'default'">
    <NuxtPage v-if="showPage" />
  </NuxtLayout>
</template>

<style>
html, body {
  margin: 0;
  padding: 0;
  overflow: hidden;
  height: 100%;
  background-color: #1e293b;
  color: #e2e8f0;
  font-family: 'clother', ui-sans-serif, system-ui, -apple-system, 'Segoe UI', Roboto, sans-serif;
}

/* Glass scrollbar */
::-webkit-scrollbar {
  width: 6px;
  height: 6px;
}
::-webkit-scrollbar-track {
  background: transparent;
}
::-webkit-scrollbar-thumb {
  background: rgba(139, 92, 246, 0.18);
  border-radius: 3px;
  border: 1px solid rgba(255, 255, 255, 0.06);
}
::-webkit-scrollbar-thumb:hover {
  background: rgba(139, 92, 246, 0.35);
}
::-webkit-scrollbar-corner {
  background: transparent;
}

</style>
