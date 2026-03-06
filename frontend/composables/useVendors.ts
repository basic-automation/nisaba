import { invoke } from '@tauri-apps/api/core'
import type { VendorPluginInfo, VendorListing, VendorImportItem, ImportResult, VendorSyncStatus, VariantSkuEntry } from '~/types'

// Module-level state — persists across navigations
const plugins = ref<VendorPluginInfo[]>([])
const loading = ref(false)
const listings = ref<Record<string, VendorListing[]>>({})
const listingLoading = ref<Record<string, boolean>>({})
const listingError = ref<Record<string, string>>({})
const variantSkuSet = ref<Set<string>>(new Set())
const syncStatus = ref<Record<string, VendorSyncStatus>>({})

export function useVendors() {
  // Computed
  const approvedPlugins = computed(() =>
    plugins.value.filter(p => p.status === 'approved')
  )
  const installedPlugins = computed(() =>
    plugins.value.filter(p => p.installed && p.enabled)
  )
  const pendingPlugins = computed(() =>
    plugins.value.filter(p => p.status === 'pending')
  )

  // Registry
  async function fetchPlugins() {
    loading.value = true
    try {
      plugins.value = await invoke<VendorPluginInfo[]>('list_registry_plugins')
    } finally {
      loading.value = false
    }
  }

  async function submitPlugin(url: string): Promise<VendorPluginInfo> {
    const result = await invoke<VendorPluginInfo>('submit_vendor_plugin', { url })
    await fetchPlugins()
    return result
  }

  async function importPlugin(path: string): Promise<VendorPluginInfo> {
    const result = await invoke<VendorPluginInfo>('import_vendor_plugin', { path })
    await fetchPlugins()
    return result
  }

  async function approvePlugin(pluginId: string) {
    await invoke('approve_vendor_plugin', { pluginId })
    await fetchPlugins()
  }

  async function rejectPlugin(pluginId: string) {
    await invoke('reject_vendor_plugin', { pluginId })
    await fetchPlugins()
  }

  async function setPluginConfig(pluginId: string, config: Record<string, string>) {
    await invoke('set_vendor_plugin_config', { pluginId, config })
    await fetchPlugins()
  }

  async function getPluginConfig(pluginId: string): Promise<Record<string, string>> {
    return invoke<Record<string, string>>('get_vendor_plugin_config', { pluginId })
  }

  async function removeRegistryPlugin(pluginId: string) {
    await invoke('remove_registry_plugin', { pluginId })
    await fetchPlugins()
  }

  // Install
  async function installPlugin(pluginId: string) {
    await invoke('install_vendor_plugin', { pluginId })
    await fetchPlugins()
  }

  async function uninstallPlugin(pluginId: string) {
    await invoke('uninstall_vendor_plugin', { pluginId })
    await fetchPlugins()
  }

  // Execution (direct — blocking, also writes to cache as side-effect)
  async function fetchVendorListings(pluginId: string) {
    listingLoading.value[pluginId] = true
    listingError.value[pluginId] = ''
    try {
      listings.value[pluginId] = await invoke<VendorListing[]>('fetch_vendor_listings', { pluginId })
    } catch (e: any) {
      listingError.value[pluginId] = e?.toString() || 'Failed to fetch listings'
    } finally {
      listingLoading.value[pluginId] = false
    }
  }

  // Cache-first loading — instant from DB
  async function loadCachedListings(pluginId: string) {
    listingLoading.value[pluginId] = true
    listingError.value[pluginId] = ''
    try {
      listings.value[pluginId] = await invoke<VendorListing[]>('get_cached_vendor_listings', { pluginId })
    } catch (e: any) {
      listingError.value[pluginId] = e?.toString() || 'Failed to load cached listings'
    } finally {
      listingLoading.value[pluginId] = false
    }
  }

  // Sync status
  async function fetchSyncStatus(pluginId: string) {
    try {
      syncStatus.value[pluginId] = await invoke<VendorSyncStatus>('get_vendor_sync_status', { pluginId })
    } catch {
      // ignore
    }
  }

  // Background sync trigger
  async function startSync(pluginId: string) {
    try {
      await invoke('sync_vendor_listings', { pluginId })
    } catch (e: any) {
      listingError.value[pluginId] = e?.toString() || 'Sync failed to start'
    }
  }

  async function importVendorListings(pluginId: string, items: VendorImportItem[]): Promise<ImportResult> {
    return invoke<ImportResult>('import_vendor_listings', { pluginId, items })
  }

  async function loadVariantSkus() {
    try {
      const entries = await invoke<VariantSkuEntry[]>('get_all_variant_skus')
      variantSkuSet.value = new Set(entries.map(e => e.sku.toLowerCase()))
    } catch {
      variantSkuSet.value = new Set()
    }
  }

  function isVendorItemImported(item: VendorListing): boolean {
    const sku = item.sku || item.vendor_item_id
    if (!sku) return false
    return variantSkuSet.value.has(sku.toLowerCase())
  }

  return {
    plugins,
    loading,
    listings,
    listingLoading,
    listingError,
    syncStatus,
    approvedPlugins,
    installedPlugins,
    pendingPlugins,
    fetchPlugins,
    submitPlugin,
    importPlugin,
    approvePlugin,
    rejectPlugin,
    setPluginConfig,
    getPluginConfig,
    removeRegistryPlugin,
    installPlugin,
    uninstallPlugin,
    fetchVendorListings,
    loadCachedListings,
    fetchSyncStatus,
    startSync,
    importVendorListings,
    variantSkuSet,
    loadVariantSkus,
    isVendorItemImported,
  }
}
