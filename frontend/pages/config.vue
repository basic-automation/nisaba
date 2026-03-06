<script setup lang="ts">
import type { AppConfig } from '~/types'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

const { config, loading, fetchConfig, saveConfig, fetchCompanyConfig, saveCompanyConfig } = useConfig()
const { activeCompanyId, activeCompany } = useCompanyContext()
const { installedPlugins, fetchPlugins, setPluginConfig, getPluginConfig } = useVendors()

const activeTab = ref<'local' | 'company' | 'plugins'>('local')
const saving = ref(false)

const { notify } = useNotifications()

// Plugin config state
const activePluginId = ref('')
const pluginConfigs = ref<Record<string, Record<string, string>>>({})
const cleanPluginJson = ref<Record<string, string>>({})
const pluginSaving = ref(false)

const pluginTabItems = computed(() =>
  installedPlugins.value.map(p => ({ label: p.display_name, value: p.id }))
)
const activePluginInfo = computed(() =>
  installedPlugins.value.find(p => p.id === activePluginId.value)
)
const isPluginDirty = computed(() => {
  const id = activePluginId.value
  if (!id || !pluginConfigs.value[id]) return false
  return JSON.stringify(pluginConfigs.value[id]) !== (cleanPluginJson.value[id] ?? '{}')
})

// Dirty tracking — snapshots of clean state for comparison
const cleanLocalJson = ref('')
const cleanCompanyJson = ref('')

const isLocalDirty = computed(() =>
  editConfig.value ? JSON.stringify(editConfig.value) !== cleanLocalJson.value : false
)
const isCompanyDirty = computed(() =>
  editCompanyConfig.value ? JSON.stringify(editCompanyConfig.value) !== cleanCompanyJson.value : false
)
const isDirty = computed(() => isLocalDirty.value || isCompanyDirty.value || isPluginDirty.value)

onBeforeRouteLeave(() => {
  if (isDirty.value) {
    return confirm('You have unsaved config changes. Leave without saving?')
  }
})

// eBay auth state
const ebayAuthLoading = ref(false)
const ebayAuthMessage = ref('')
const ebayAuthenticated = ref(false)
const showManualCode = ref(false)
const ebayAuthCode = ref('')

// Amazon auth state
const amazonAuthLoading = ref(false)
const amazonAuthMessage = ref('')
const amazonAuthenticated = ref(false)
const showAmazonManualCode = ref(false)
const amazonAuthCode = ref('')

// Separate edit states
const editConfig = ref<AppConfig | null>(null)
const editCompanyConfig = ref<Record<string, any> | null>(null)

const hasCompany = computed(() => !!activeCompanyId.value)
const isAdmin = computed(() => activeCompany.value?.role === 'admin')
const showCompanyTab = computed(() => hasCompany.value && isAdmin.value)

// Defaults for company platform config — ensures keys added in newer versions
// are always present when loading configs saved by older versions.
const companyConfigDefaults: Record<string, any> = {
  amazon: {
    enabled: false,
    client_id: '',
    client_secret: '',
    refresh_token: '',
    seller_id: '',
    region: 'NA',
    marketplace_ids: [],
  },
}

async function loadCompanyConfig() {
  editCompanyConfig.value = null
  if (!showCompanyTab.value) return
  const cc = await fetchCompanyConfig()
  if (cc) {
    // Merge in defaults for any keys missing from the stored config
    const merged = { ...companyConfigDefaults, ...cc }
    const json = JSON.stringify(merged)
    editCompanyConfig.value = JSON.parse(json)
    cleanCompanyJson.value = json
  }
  // Re-check platform auth for the active company
  try {
    ebayAuthenticated.value = await invoke<boolean>('check_platform_auth', { platform: 'ebay' })
  } catch {
    ebayAuthenticated.value = false
  }
  try {
    amazonAuthenticated.value = await invoke<boolean>('check_platform_auth', { platform: 'amazon' })
  } catch {
    amazonAuthenticated.value = false
  }
}

onMounted(async () => {
  await fetchConfig()
  try {
    ebayAuthenticated.value = await invoke<boolean>('check_platform_auth', { platform: 'ebay' })
  } catch { /* ignore */ }
  try {
    amazonAuthenticated.value = await invoke<boolean>('check_platform_auth', { platform: 'amazon' })
  } catch { /* ignore */ }
  await loadCompanyConfig()
  await fetchPlugins()
})

// Load plugin config when switching sub-tabs
watch(activePluginId, async (id) => {
  if (!id) return
  if (pluginConfigs.value[id]) return // already loaded
  try {
    const cfg = await getPluginConfig(id)
    pluginConfigs.value[id] = cfg
    cleanPluginJson.value[id] = JSON.stringify(cfg)
  } catch {
    pluginConfigs.value[id] = {}
    cleanPluginJson.value[id] = '{}'
  }
})

// Auto-select first installed plugin when switching to plugins tab
watch(activeTab, async (tab) => {
  if (tab === 'plugins' && !activePluginId.value && installedPlugins.value.length > 0) {
    activePluginId.value = installedPlugins.value[0].id
  }
})

// Re-fetch company config when active company changes
watch(activeCompanyId, async () => {
  // If we were on the company tab but lost access, switch back to local
  if (activeTab.value === 'company' && !showCompanyTab.value) {
    activeTab.value = 'local'
  }
  // Re-fetch plugins for the new company
  await fetchPlugins()
  activePluginId.value = ''
  pluginConfigs.value = {}
  cleanPluginJson.value = {}
  await loadCompanyConfig()
})

// Listen for auth events from the webview window
const unlisteners: (() => void)[] = []

onMounted(async () => {
  unlisteners.push(await listen<boolean>('ebay-auth-complete', (_event) => {
    ebayAuthLoading.value = false
    ebayAuthenticated.value = true
    ebayAuthMessage.value = ''
    showManualCode.value = false
    ebayAuthCode.value = ''
    notify({ type: 'success', title: 'eBay connected', message: 'OAuth authentication successful', source: 'config' })
  }))

  unlisteners.push(await listen<string>('ebay-auth-error', (event) => {
    ebayAuthLoading.value = false
    ebayAuthMessage.value = ''
    notify({ type: 'error', title: 'eBay auth failed', message: event.payload, detail: event.payload, source: 'config' })
  }))

  unlisteners.push(await listen<boolean>('amazon-auth-complete', (_event) => {
    amazonAuthLoading.value = false
    amazonAuthenticated.value = true
    amazonAuthMessage.value = ''
    showAmazonManualCode.value = false
    amazonAuthCode.value = ''
    notify({ type: 'success', title: 'Amazon connected', message: 'OAuth authentication successful', source: 'config' })
  }))

  unlisteners.push(await listen<string>('amazon-auth-error', (event) => {
    amazonAuthLoading.value = false
    amazonAuthMessage.value = ''
    notify({ type: 'error', title: 'Amazon auth failed', message: event.payload, detail: event.payload, source: 'config' })
  }))
})

onUnmounted(() => {
  unlisteners.forEach(fn => fn())
})

watch(config, (val) => {
  if (val) {
    const json = JSON.stringify(val)
    editConfig.value = JSON.parse(json)
    cleanLocalJson.value = json
  }
}, { immediate: true })

async function handleEbayAuth() {
  ebayAuthLoading.value = true
  ebayAuthMessage.value = ''
  showManualCode.value = false
  try {
    await invoke('start_ebay_auth')
    ebayAuthMessage.value = 'Sign in on the eBay window...'
  } catch (e: any) {
    ebayAuthMessage.value = ''
    notify({ type: 'error', title: 'eBay auth failed', message: String(e), detail: String(e), source: 'config' })
    ebayAuthLoading.value = false
  }
}

async function handleEbayManualComplete() {
  if (!ebayAuthCode.value.trim()) return
  ebayAuthMessage.value = ''
  try {
    await invoke('complete_ebay_auth', { code: ebayAuthCode.value.trim() })
    ebayAuthenticated.value = true
    showManualCode.value = false
    ebayAuthCode.value = ''
    notify({ type: 'success', title: 'eBay connected', message: 'Manual code authentication successful', source: 'config' })
  } catch (e: any) {
    notify({ type: 'error', title: 'eBay auth failed', message: String(e), detail: String(e), source: 'config' })
  }
}

async function handleAmazonAuth() {
  amazonAuthLoading.value = true
  amazonAuthMessage.value = ''
  showAmazonManualCode.value = false
  try {
    await invoke('start_amazon_auth')
    amazonAuthMessage.value = 'Sign in on the Amazon window...'
  } catch (e: any) {
    amazonAuthMessage.value = ''
    notify({ type: 'error', title: 'Amazon auth failed', message: String(e), detail: String(e), source: 'config' })
    amazonAuthLoading.value = false
  }
}

async function handleAmazonManualComplete() {
  if (!amazonAuthCode.value.trim()) return
  amazonAuthMessage.value = ''
  try {
    await invoke('complete_amazon_auth', { code: amazonAuthCode.value.trim() })
    amazonAuthenticated.value = true
    showAmazonManualCode.value = false
    amazonAuthCode.value = ''
    notify({ type: 'success', title: 'Amazon connected', message: 'Manual code authentication successful', source: 'config' })
  } catch (e: any) {
    notify({ type: 'error', title: 'Amazon auth failed', message: String(e), detail: String(e), source: 'config' })
  }
}

async function handleSaveLocal() {
  if (!editConfig.value) return
  saving.value = true
  try {
    await saveConfig(editConfig.value)
    cleanLocalJson.value = JSON.stringify(editConfig.value)
    notify({ type: 'success', title: 'Config saved', message: 'Local configuration updated', source: 'config' })
  } catch (e) {
    notify({ type: 'error', title: 'Save failed', message: String(e), detail: String(e), source: 'config' })
  } finally {
    saving.value = false
  }
}

async function handleSaveCompany() {
  if (!editCompanyConfig.value) return
  saving.value = true
  try {
    await saveCompanyConfig(editCompanyConfig.value)
    cleanCompanyJson.value = JSON.stringify(editCompanyConfig.value)
    notify({ type: 'success', title: 'Config saved', message: 'Company configuration updated', source: 'config' })
  } catch (e) {
    notify({ type: 'error', title: 'Save failed', message: String(e), detail: String(e), source: 'config' })
  } finally {
    saving.value = false
  }
}

async function handleSavePluginConfig() {
  const id = activePluginId.value
  if (!id || !pluginConfigs.value[id]) return
  pluginSaving.value = true
  try {
    await setPluginConfig(id, pluginConfigs.value[id])
    cleanPluginJson.value[id] = JSON.stringify(pluginConfigs.value[id])
    notify({ type: 'success', title: 'Plugin config saved', message: `${activePluginInfo.value?.display_name ?? id} settings updated`, source: 'config' })
  } catch (e) {
    notify({ type: 'error', title: 'Plugin save failed', message: String(e), detail: String(e), source: 'config' })
  } finally {
    pluginSaving.value = false
  }
}

async function handleToggleDropship(include: boolean) {
  const id = activePluginId.value
  if (!id) return
  try {
    await invoke('set_vendor_include_stock', { pluginId: id, include })
    await fetchPlugins()
  } catch (e) {
    notify({ type: 'error', title: 'Dropship toggle failed', message: String(e), source: 'config' })
  }
}
</script>

<template>
  <div class="flex flex-col h-full">
    <!-- Header -->
    <div class="flex items-end justify-between mb-10 shrink-0 max-w-2xl">
      <h2 class="text-sm font-medium tracking-widest uppercase text-muted/60">Config</h2>
      <div class="flex items-center gap-4">
        <span v-if="isDirty" class="text-xs text-yellow-400/60">Unsaved changes</span>
        <button
          v-if="activeTab === 'local'"
          class="text-xs transition-colors disabled:opacity-30"
          :class="isLocalDirty ? 'text-accent hover:text-accent/80' : 'text-muted/30'"
          :disabled="saving || !editConfig"
          @click="handleSaveLocal"
        >
          {{ saving ? 'Saving...' : 'Save' }}
        </button>
        <button
          v-if="activeTab === 'company'"
          class="text-xs transition-colors disabled:opacity-30"
          :class="isCompanyDirty ? 'text-accent hover:text-accent/80' : 'text-muted/30'"
          :disabled="saving || !editCompanyConfig"
          @click="handleSaveCompany"
        >
          {{ saving ? 'Saving...' : 'Save' }}
        </button>
        <button
          v-if="activeTab === 'plugins'"
          class="text-xs transition-colors disabled:opacity-30"
          :class="isPluginDirty ? 'text-accent hover:text-accent/80' : 'text-muted/30'"
          :disabled="pluginSaving || !activePluginId"
          @click="handleSavePluginConfig"
        >
          {{ pluginSaving ? 'Saving...' : 'Save' }}
        </button>
      </div>
    </div>

    <!-- Tabs -->
    <div class="flex gap-6 mb-10 border-b border-border/20 shrink-0 max-w-2xl">
      <button
        class="pb-2 text-xs uppercase tracking-widest transition-colors"
        :class="activeTab === 'local' ? 'text-accent border-b border-accent' : 'text-muted/40 hover:text-muted/60'"
        @click="activeTab = 'local'"
      >
        Local
      </button>
      <button
        v-if="showCompanyTab"
        class="pb-2 text-xs uppercase tracking-widest transition-colors"
        :class="activeTab === 'company' ? 'text-accent border-b border-accent' : 'text-muted/40 hover:text-muted/60'"
        @click="activeTab = 'company'"
      >
        Company
      </button>
      <button
        class="pb-2 text-xs uppercase tracking-widest transition-colors"
        :class="activeTab === 'plugins' ? 'text-accent border-b border-accent' : 'text-muted/40 hover:text-muted/60'"
        @click="activeTab = 'plugins'"
      >
        Plugins
      </button>
    </div>

    <div class="flex-1 overflow-auto min-h-0 -mr-6 pr-6">
    <div v-if="loading" class="py-16 text-sm text-muted/40">Loading config...</div>

    <!-- LOCAL TAB -->
    <div v-else-if="activeTab === 'local' && editConfig" class="space-y-14 max-w-2xl">
      <!-- General -->
      <section>
        <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">General</h3>
        <div class="grid grid-cols-3 gap-x-8 gap-y-5">
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Sync schedule</label>
            <input v-model="editConfig.general.sync_schedule" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground font-mono focus:outline-none focus:border-accent/50 transition-colors">
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Log level</label>
            <select v-model="editConfig.general.log_level" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors appearance-none">
              <option value="error" class="bg-background">Error</option>
              <option value="warn" class="bg-background">Warn</option>
              <option value="info" class="bg-background">Info</option>
              <option value="debug" class="bg-background">Debug</option>
              <option value="trace" class="bg-background">Trace</option>
            </select>
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Max retries</label>
            <input v-model.number="editConfig.general.max_retries" type="number" min="0" max="10" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
        </div>
      </section>
    </div>

    <!-- COMPANY TAB -->
    <div v-else-if="activeTab === 'company' && editCompanyConfig" class="space-y-14 max-w-2xl">
      <!-- eBay -->
      <section>
        <div class="flex items-center justify-between mb-5">
          <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60">eBay</h3>
          <label class="flex items-center gap-2 cursor-pointer">
            <input v-model="editCompanyConfig.ebay.enabled" type="checkbox" class="accent-accent">
            <span class="text-xs text-muted/40">{{ editCompanyConfig.ebay.enabled ? 'Enabled' : 'Disabled' }}</span>
          </label>
        </div>
        <div v-if="editCompanyConfig.ebay.enabled" class="space-y-6">
          <div class="grid grid-cols-2 gap-x-8 gap-y-5">
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Client ID</label>
              <input v-model="editCompanyConfig.ebay.client_id" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
            </div>
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Client secret</label>
              <input v-model="editCompanyConfig.ebay.client_secret" type="password" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
            </div>
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Redirect URI (RuName)</label>
              <input v-model="editCompanyConfig.ebay.redirect_uri" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
            </div>
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Environment</label>
              <select v-model="editCompanyConfig.ebay.environment" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors appearance-none">
                <option value="sandbox" class="bg-background">Sandbox</option>
                <option value="production" class="bg-background">Production</option>
              </select>
            </div>
          </div>

          <!-- eBay OAuth -->
          <div class="border-t border-border/20 pt-5">
            <div class="flex items-center gap-3 mb-4">
              <span class="text-[11px] text-muted/40 uppercase tracking-wider">Auth status</span>
              <span
                class="text-xs"
                :class="ebayAuthenticated ? 'text-green-400/80' : 'text-yellow-400/80'"
              >{{ ebayAuthenticated ? 'Connected' : 'Not connected' }}</span>
            </div>

            <div class="space-y-3">
              <button
                class="text-xs text-accent hover:text-accent/80 transition-colors disabled:opacity-30"
                :disabled="ebayAuthLoading"
                @click="handleEbayAuth"
              >
                {{ ebayAuthLoading ? 'Waiting for authorization...' : 'Authorize with eBay' }}
              </button>

              <!-- Manual code fallback -->
              <div v-if="!ebayAuthLoading">
                <button
                  class="text-[11px] text-muted/30 hover:text-muted/50 transition-colors"
                  @click="showManualCode = !showManualCode"
                >
                  {{ showManualCode ? 'Hide manual entry' : 'Paste code manually' }}
                </button>

                <div v-if="showManualCode" class="mt-3 space-y-2">
                  <p class="text-[11px] text-muted/30">Paste the authorization code from the eBay redirect URL:</p>
                  <div class="flex items-center gap-3">
                    <input
                      v-model="ebayAuthCode"
                      type="text"
                      placeholder="Authorization code..."
                      class="flex-1 bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground font-mono placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
                      @keydown.enter.prevent="handleEbayManualComplete"
                    >
                    <button
                      class="text-xs text-accent hover:text-accent/80 transition-colors disabled:opacity-30"
                      :disabled="!ebayAuthCode.trim()"
                      @click="handleEbayManualComplete"
                    >
                      Complete
                    </button>
                  </div>
                </div>
              </div>

              <p
                v-if="ebayAuthMessage"
                class="text-xs"
                :class="ebayAuthMessage.startsWith('Error') ? 'text-red-400/80' : 'text-muted/50'"
              >{{ ebayAuthMessage }}</p>
            </div>
          </div>
        </div>
      </section>

      <!-- Squarespace -->
      <section>
        <div class="flex items-center justify-between mb-5">
          <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60">Squarespace</h3>
          <label class="flex items-center gap-2 cursor-pointer">
            <input v-model="editCompanyConfig.squarespace.enabled" type="checkbox" class="accent-accent">
            <span class="text-xs text-muted/40">{{ editCompanyConfig.squarespace.enabled ? 'Enabled' : 'Disabled' }}</span>
          </label>
        </div>
        <div v-if="editCompanyConfig.squarespace.enabled" class="grid grid-cols-2 gap-x-8 gap-y-5">
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">API key</label>
            <input v-model="editCompanyConfig.squarespace.api_key" type="password" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">User agent</label>
            <input v-model="editCompanyConfig.squarespace.user_agent" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
        </div>
      </section>

      <!-- XMR Bazaar -->
      <section>
        <div class="flex items-center justify-between mb-5">
          <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60">XMR Bazaar</h3>
          <label class="flex items-center gap-2 cursor-pointer">
            <input v-model="editCompanyConfig.xmrbazaar.enabled" type="checkbox" class="accent-accent">
            <span class="text-xs text-muted/40">{{ editCompanyConfig.xmrbazaar.enabled ? 'Enabled' : 'Disabled' }}</span>
          </label>
        </div>
        <div v-if="editCompanyConfig.xmrbazaar.enabled" class="grid grid-cols-2 gap-x-8 gap-y-5">
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Username</label>
            <input v-model="editCompanyConfig.xmrbazaar.username" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Password</label>
            <input v-model="editCompanyConfig.xmrbazaar.password" type="password" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
          <div class="col-span-2">
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Base URL</label>
            <input v-model="editCompanyConfig.xmrbazaar.base_url" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
        </div>
      </section>

      <!-- Amazon -->
      <section>
        <div class="flex items-center justify-between mb-5">
          <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60">Amazon</h3>
          <label class="flex items-center gap-2 cursor-pointer">
            <input v-model="editCompanyConfig.amazon.enabled" type="checkbox" class="accent-accent">
            <span class="text-xs text-muted/40">{{ editCompanyConfig.amazon.enabled ? 'Enabled' : 'Disabled' }}</span>
          </label>
        </div>
        <div v-if="editCompanyConfig.amazon.enabled" class="space-y-6">
          <div class="grid grid-cols-2 gap-x-8 gap-y-5">
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Client ID (LWA)</label>
              <input v-model="editCompanyConfig.amazon.client_id" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
            </div>
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Client Secret</label>
              <input v-model="editCompanyConfig.amazon.client_secret" type="password" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
            </div>
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Seller ID</label>
              <input v-model="editCompanyConfig.amazon.seller_id" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
            </div>
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Region</label>
              <select v-model="editCompanyConfig.amazon.region" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors appearance-none">
                <option value="NA" class="bg-background">North America (NA)</option>
                <option value="EU" class="bg-background">Europe (EU)</option>
                <option value="FE" class="bg-background">Far East (FE)</option>
              </select>
            </div>
            <div class="col-span-2">
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Marketplace IDs (comma-separated)</label>
              <input
                :value="(editCompanyConfig.amazon.marketplace_ids || []).join(', ')"
                class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground font-mono focus:outline-none focus:border-accent/50 transition-colors"
                @input="editCompanyConfig.amazon.marketplace_ids = ($event.target as HTMLInputElement).value.split(',').map((s: string) => s.trim()).filter(Boolean)"
              >
              <p class="text-[10px] text-muted/20 mt-1">US: ATVPDKIKX0DER, CA: A2EUQ1WTGCTBG2, UK: A1F83G8C2ARO7P, DE: A1PA6795UKMFR9</p>
            </div>
          </div>

          <!-- Amazon OAuth -->
          <div class="border-t border-border/20 pt-5">
            <div class="flex items-center gap-3 mb-4">
              <span class="text-[11px] text-muted/40 uppercase tracking-wider">Auth status</span>
              <span
                class="text-xs"
                :class="amazonAuthenticated ? 'text-green-400/80' : 'text-yellow-400/80'"
              >{{ amazonAuthenticated ? 'Connected' : 'Not connected' }}</span>
            </div>

            <div class="space-y-3">
              <button
                class="text-xs text-accent hover:text-accent/80 transition-colors disabled:opacity-30"
                :disabled="amazonAuthLoading"
                @click="handleAmazonAuth"
              >
                {{ amazonAuthLoading ? 'Waiting for authorization...' : 'Authorize with Amazon' }}
              </button>

              <!-- Manual code fallback -->
              <div v-if="!amazonAuthLoading">
                <button
                  class="text-[11px] text-muted/30 hover:text-muted/50 transition-colors"
                  @click="showAmazonManualCode = !showAmazonManualCode"
                >
                  {{ showAmazonManualCode ? 'Hide manual entry' : 'Paste code manually' }}
                </button>

                <div v-if="showAmazonManualCode" class="mt-3 space-y-2">
                  <p class="text-[11px] text-muted/30">Paste the spapi_oauth_code from the redirect URL:</p>
                  <div class="flex items-center gap-3">
                    <input
                      v-model="amazonAuthCode"
                      type="text"
                      placeholder="spapi_oauth_code..."
                      class="flex-1 bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground font-mono placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
                      @keydown.enter.prevent="handleAmazonManualComplete"
                    >
                    <button
                      class="text-xs text-accent hover:text-accent/80 transition-colors disabled:opacity-30"
                      :disabled="!amazonAuthCode.trim()"
                      @click="handleAmazonManualComplete"
                    >
                      Complete
                    </button>
                  </div>
                </div>
              </div>

              <p
                v-if="amazonAuthMessage"
                class="text-xs"
                :class="amazonAuthMessage.startsWith('Error') ? 'text-red-400/80' : 'text-muted/50'"
              >{{ amazonAuthMessage }}</p>
            </div>
          </div>
        </div>
      </section>

      <!-- Alerts -->
      <section>
        <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Alerts</h3>
        <div>
          <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Default low stock threshold</label>
          <input v-model.number="editCompanyConfig.alerts.default_low_stock_threshold" type="number" min="0" class="w-48 bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
        </div>
      </section>
    </div>

    <!-- PLUGINS TAB -->
    <div v-else-if="activeTab === 'plugins'" class="space-y-8 max-w-2xl">
      <div v-if="installedPlugins.length === 0" class="py-16 text-sm text-muted/40">
        No plugins installed.
        <NuxtLink to="/marketplace" class="text-accent hover:text-accent/80 transition-colors">Browse Marketplace</NuxtLink>
      </div>

      <template v-else>
        <TabBar v-model="activePluginId" :items="pluginTabItems" />

        <section v-if="activePluginInfo && pluginConfigs[activePluginId]">
          <p class="text-xs text-muted/40 mb-6">{{ activePluginInfo.description }}</p>

          <div v-if="activePluginInfo.config_fields.length > 0" class="grid grid-cols-2 gap-x-8 gap-y-5">
            <div v-for="field in activePluginInfo.config_fields" :key="field.key">
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">
                {{ field.label }}
                <span v-if="field.required" class="text-accent">*</span>
              </label>
              <input
                v-model="pluginConfigs[activePluginId][field.key]"
                :type="field.secret ? 'password' : 'text'"
                :placeholder="field.placeholder || ''"
                class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors"
              >
            </div>
          </div>
          <p v-else class="text-xs text-muted/30">This plugin has no configuration fields.</p>

          <!-- Dropship toggle (system setting) -->
          <div v-if="activePluginInfo.category === 'vendor'" class="border-t border-border/20 pt-5 mt-5">
            <div class="flex items-center justify-between">
              <div>
                <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1">
                  Include vendor stock in available quantity
                </label>
                <p class="text-[10px] text-muted/20">
                  Enable for dropship vendors. Their inventory will be added to your on-hand stock.
                </p>
              </div>
              <Switch
                :checked="activePluginInfo.include_vendor_stock"
                @update:checked="handleToggleDropship"
              />
            </div>
          </div>
        </section>
      </template>
    </div>
    </div>
  </div>
</template>
