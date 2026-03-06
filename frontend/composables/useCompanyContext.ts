import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { CompanyRegistryEntry } from '~/types'

// Preserve global reactive state across HMR (hot reload re-evaluates module scope,
// resetting all refs — but the Tauri backend doesn't re-emit 'companies-ready',
// leaving companiesInitialized stuck at false and the UI stuck on "Loading...")
const _hmr: Record<string, any> = (import.meta as any).hot?.data ?? {}

const activeCompanyId: Ref<string | null> = _hmr.activeCompanyId ?? (_hmr.activeCompanyId = ref<string | null>(null))
const companies: Ref<CompanyRegistryEntry[]> = _hmr.companies ?? (_hmr.companies = ref<CompanyRegistryEntry[]>([]))
const backendReady: Ref<boolean> = _hmr.backendReady ?? (_hmr.backendReady = ref(false))
const companiesInitialized: Ref<boolean> = _hmr.companiesInitialized ?? (_hmr.companiesInitialized = ref(false))

// Event hooks for company switch notifications
const switchCallbacks: (() => void)[] = _hmr.switchCallbacks ?? (_hmr.switchCallbacks = [] as (() => void)[])

// Listen for backend "companies-ready" event (emitted after slow init completes)
let companiesReadyListenerSetup: boolean = _hmr.companiesReadyListenerSetup ?? false

export function useCompanyContext() {
  async function fetchCompanies(opts?: { skipActiveId?: boolean }) {
    try {
      companies.value = await invoke<CompanyRegistryEntry[]>('list_companies')
      if (!opts?.skipActiveId) {
        const id = await invoke<string | null>('get_active_company_id')
        activeCompanyId.value = id
        // If backend returned an active company, it's fully initialized
        // (active_company_id is set right before the companies-ready event).
        // This recovers from HMR full-page reloads where the one-time event was missed.
        if (id && !companiesInitialized.value) {
          companiesInitialized.value = true
        }
      }
      backendReady.value = true
    } catch {
      companies.value = []
      if (!opts?.skipActiveId) {
        activeCompanyId.value = null
      }
    }
  }

  /** Retry fetchCompanies until backend state is managed. */
  async function waitForBackend(maxRetries = 40, intervalMs = 500) {
    for (let i = 0; i < maxRetries; i++) {
      await fetchCompanies()
      if (backendReady.value) return
      await new Promise(r => setTimeout(r, intervalMs))
    }
    // Final fallback: set backendReady anyway so UI isn't stuck forever
    backendReady.value = true
  }

  // Set up a one-time listener for the companies-ready event.
  // When backend finishes initializing all companies (adapters, sync, P2P),
  // re-fetch companies and notify switch listeners so data pages refresh.
  if (!companiesReadyListenerSetup) {
    companiesReadyListenerSetup = true
    _hmr.companiesReadyListenerSetup = true
    listen('companies-ready', async () => {
      await fetchCompanies()
      companiesInitialized.value = true
      for (const cb of switchCallbacks) {
        cb()
      }
    })
  }

  async function switchCompany(id: string) {
    try {
      await invoke('switch_company', { id })
    } catch (e) {
      console.error('Failed to switch company:', e)
      return
    }
    activeCompanyId.value = id
    // Notify all listeners to refetch their data
    for (const cb of switchCallbacks) {
      cb()
    }
  }

  function onCompanySwitch(callback: () => void) {
    switchCallbacks.push(callback)
    // Return cleanup function
    return () => {
      const idx = switchCallbacks.indexOf(callback)
      if (idx >= 0) switchCallbacks.splice(idx, 1)
    }
  }

  const activeCompany = computed(() =>
    companies.value.find(c => c.id === activeCompanyId.value) ?? null
  )

  return {
    activeCompanyId: readonly(activeCompanyId),
    companies: readonly(companies),
    activeCompany,
    backendReady: readonly(backendReady),
    companiesInitialized: readonly(companiesInitialized),
    fetchCompanies,
    waitForBackend,
    switchCompany,
    onCompanySwitch,
  }
}
