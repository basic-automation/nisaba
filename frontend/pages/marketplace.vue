<script setup lang="ts">
import type { VendorPluginInfo, VendorConfigField } from '~/types'

const {
  plugins,
  loading,
  approvedPlugins,
  installedPlugins,
  pendingPlugins,
  fetchPlugins,
  submitPlugin,
  importPlugin,
  approvePlugin,
  rejectPlugin,
  setPluginConfig,
  removeRegistryPlugin,
  installPlugin,
  uninstallPlugin,
} = useVendors()

const { companiesInitialized } = useCompanyContext()

// Tab state
type MarketTab = 'available' | 'installed' | 'pending'
const activeTab = ref<MarketTab>('available')
const tabItems = computed(() => {
  const items = [
    { label: 'Available', value: 'available' as MarketTab },
    { label: 'Installed', value: 'installed' as MarketTab },
  ]
  if (isAdmin.value) {
    const count = pendingPlugins.value.length
    items.push({
      label: count > 0 ? `Pending (${count})` : 'Pending',
      value: 'pending' as MarketTab,
    })
  }
  return items
})

// Role check
const isAdmin = ref(false)
async function initMarketplace() {
  if (!companiesInitialized.value) return
  await fetchPlugins()
  try {
    const { invoke } = await import('@tauri-apps/api/core')
    const company = await invoke<any>('get_company')
    isAdmin.value = company?.role === 'admin'
  } catch { /* ignore */ }
}

onMounted(initMarketplace)
watch(companiesInitialized, initMarketplace)

// Available = approved but not installed
const availablePlugins = computed(() =>
  approvedPlugins.value.filter(p => !p.installed)
)

// Submit dialog
const showSubmitDialog = ref(false)
const submitUrl = ref('')
const submitting = ref(false)
const submitError = ref('')
const submitPreview = ref<VendorPluginInfo | null>(null)

async function handleSubmitFromUrl() {
  if (!submitUrl.value) return
  submitting.value = true
  submitError.value = ''
  try {
    const result = await submitPlugin(submitUrl.value)
    submitPreview.value = result
    showSubmitDialog.value = false
    submitUrl.value = ''
  } catch (e: any) {
    submitError.value = e?.toString() || 'Failed to submit plugin'
  } finally {
    submitting.value = false
  }
}

async function handleSubmitFromFile() {
  try {
    const { open } = await import('@tauri-apps/plugin-dialog')
    const selected = await open({
      multiple: false,
      filters: [{ name: 'TypeScript', extensions: ['ts'] }],
    })
    if (!selected) return
    const path = typeof selected === 'string' ? selected : selected
    submitting.value = true
    submitError.value = ''
    const result = await importPlugin(path as string)
    submitPreview.value = result
    showSubmitDialog.value = false
  } catch (e: any) {
    submitError.value = e?.toString() || 'Failed to import plugin'
  } finally {
    submitting.value = false
  }
}

async function handleSubmitFromFolder() {
  try {
    const { open } = await import('@tauri-apps/plugin-dialog')
    const selected = await open({
      directory: true,
    })
    if (!selected) return
    submitting.value = true
    submitError.value = ''
    const result = await importPlugin(selected as string)
    submitPreview.value = result
    showSubmitDialog.value = false
  } catch (e: any) {
    submitError.value = e?.toString() || 'Failed to import plugin'
  } finally {
    submitting.value = false
  }
}

// Configure dialog
const showConfigDialog = ref(false)
const configPlugin = ref<VendorPluginInfo | null>(null)
const configValues = ref<Record<string, string>>({})
const configSaving = ref(false)

function openConfigDialog(plugin: VendorPluginInfo) {
  configPlugin.value = plugin
  configValues.value = {}
  plugin.config_fields.forEach((f: VendorConfigField) => {
    configValues.value[f.key] = ''
  })
  showConfigDialog.value = true
}

async function handleSaveConfig() {
  if (!configPlugin.value) return
  configSaving.value = true
  try {
    await setPluginConfig(configPlugin.value.id, configValues.value)
    showConfigDialog.value = false
  } finally {
    configSaving.value = false
  }
}

// Expand code
const expandedCode = ref<string | null>(null)
function toggleCode(pluginId: string) {
  expandedCode.value = expandedCode.value === pluginId ? null : pluginId
}

async function handleToggleEnabled(plugin: VendorPluginInfo) {
  // Toggle by uninstalling/installing
  if (plugin.enabled) {
    await uninstallPlugin(plugin.id)
  } else {
    await installPlugin(plugin.id)
  }
}

async function handleRemovePlugin(plugin: VendorPluginInfo) {
  if (!confirm(`Remove "${plugin.display_name}" from the marketplace?`)) return
  try {
    await removeRegistryPlugin(plugin.id)
  } catch (e: any) {
    submitError.value = e?.toString() || 'Failed to remove plugin'
  }
}

// Category badge color
function categoryColor(cat: string): string {
  switch (cat) {
    case 'vendor': return 'bg-blue-500/20 text-blue-300'
    case 'analytics': return 'bg-purple-500/20 text-purple-300'
    case 'shipping': return 'bg-green-500/20 text-green-300'
    case 'payment': return 'bg-amber-500/20 text-amber-300'
    default: return 'bg-muted/20 text-muted/60'
  }
}
</script>

<template>
  <CompanyRequired>
  <div class="flex flex-col h-full">
    <!-- Header -->
    <div class="flex items-end justify-between mb-10 shrink-0">
      <h2 class="text-sm font-medium tracking-widest uppercase text-muted/60">Plugin Marketplace</h2>
      <Button variant="solid" color="accent" size="sm" @click="showSubmitDialog = true">
        Submit Plugin
      </Button>
    </div>

    <!-- Tabs -->
    <div class="mb-6 shrink-0">
      <TabBar v-model="activeTab" :items="tabItems" />
    </div>

    <!-- Content area -->
    <div class="flex-1 overflow-auto min-h-0 -mr-6 pr-6">
      <!-- Loading -->
      <div v-if="loading" class="py-16 text-sm text-muted/40">Loading plugins...</div>

      <!-- Available Tab -->
      <template v-else-if="activeTab === 'available'">
        <div v-if="availablePlugins.length === 0" class="py-16 text-sm text-muted/40 text-center">
          No plugins available to install.
        </div>
        <div v-else class="marketplace-grid">
          <GroundGlass
            v-for="plugin in availablePlugins"
            :key="plugin.id"
            class="plugin-card"
          >
            <div class="relative z-[1] flex flex-col h-full p-5">
              <div class="flex items-start justify-between mb-2">
                <div class="flex items-center gap-2">
                  <img
                    v-if="plugin.icon"
                    :src="plugin.icon"
                    :alt="plugin.display_name"
                    class="w-6 h-6 rounded shrink-0"
                  />
                  <h3 class="text-sm font-medium text-foreground">{{ plugin.display_name }}</h3>
                </div>
                <div class="flex items-center gap-2">
                  <span
                    class="text-[10px] px-1.5 py-0.5 rounded"
                    :class="categoryColor(plugin.category)"
                  >{{ plugin.category }}</span>
                  <span class="text-[10px] text-muted/30 bg-surface/20 px-2 py-0.5 rounded">v{{ plugin.version }}</span>
                </div>
              </div>
              <p class="text-xs text-muted/40 line-clamp-2 mb-3">{{ plugin.description }}</p>

              <div v-if="plugin.config_fields.length > 0" class="text-[11px] text-muted/20 mb-4">
                Requires: {{ plugin.config_fields.map((f: VendorConfigField) => f.label).join(', ') }}
              </div>

              <div class="mt-auto flex items-center gap-3">
                <Button variant="solid" color="accent" size="xs" @click="installPlugin(plugin.id)">
                  Install
                </Button>
                <button
                  v-if="isAdmin && plugin.config_fields.length > 0 && !plugin.has_config"
                  class="text-xs text-muted/40 hover:text-accent transition-colors"
                  @click="openConfigDialog(plugin)"
                >
                  Configure
                </button>
                <button
                  v-if="isAdmin"
                  class="text-xs text-muted/40 hover:text-red-400 transition-colors ml-auto"
                  @click="handleRemovePlugin(plugin)"
                >
                  Remove
                </button>
              </div>
            </div>
          </GroundGlass>
        </div>
      </template>

      <!-- Installed Tab -->
      <template v-else-if="activeTab === 'installed'">
        <div v-if="installedPlugins.length === 0" class="py-16 text-sm text-muted/40 text-center">
          No plugins installed.
        </div>
        <div v-else class="marketplace-grid">
          <GroundGlass
            v-for="plugin in installedPlugins"
            :key="plugin.id"
            class="plugin-card"
          >
            <div class="relative z-[1] flex flex-col h-full p-5">
              <div class="flex items-start justify-between mb-2">
                <div class="flex items-center gap-2">
                  <img
                    v-if="plugin.icon"
                    :src="plugin.icon"
                    :alt="plugin.display_name"
                    class="w-6 h-6 rounded shrink-0"
                  />
                  <span
                    class="w-2 h-2 rounded-full shrink-0"
                    :class="plugin.enabled ? 'bg-green-400' : 'bg-muted/20'"
                  />
                  <h3 class="text-sm font-medium text-foreground">{{ plugin.display_name }}</h3>
                </div>
                <div class="flex items-center gap-2">
                  <span
                    class="text-[10px] px-1.5 py-0.5 rounded"
                    :class="categoryColor(plugin.category)"
                  >{{ plugin.category }}</span>
                  <span class="text-[10px] text-muted/30 bg-surface/20 px-2 py-0.5 rounded">v{{ plugin.version }}</span>
                </div>
              </div>
              <p class="text-xs text-muted/40 line-clamp-2 mb-4">{{ plugin.description }}</p>

              <div class="mt-auto flex items-center gap-3">
                <Button
                  variant="solid"
                  color="muted"
                  size="xs"
                  @click="uninstallPlugin(plugin.id)"
                >
                  Uninstall
                </Button>
                <button
                  v-if="isAdmin && plugin.config_fields.length > 0"
                  class="text-xs text-muted/40 hover:text-accent transition-colors"
                  @click="openConfigDialog(plugin)"
                >
                  Configure
                </button>
                <button
                  v-if="isAdmin"
                  class="text-xs text-muted/40 hover:text-red-400 transition-colors ml-auto"
                  @click="handleRemovePlugin(plugin)"
                >
                  Remove
                </button>
              </div>
            </div>
          </GroundGlass>
        </div>
      </template>

      <!-- Pending Tab (admin only) -->
      <template v-else-if="activeTab === 'pending'">
        <div v-if="pendingPlugins.length === 0" class="py-16 text-sm text-muted/40 text-center">
          No plugins pending review.
        </div>
        <div v-else class="space-y-4">
          <GroundGlass
            v-for="plugin in pendingPlugins"
            :key="plugin.id"
            class="p-5"
          >
            <div class="relative z-[1]">
              <div class="flex items-start justify-between mb-2">
                <div class="flex items-center gap-2">
                  <img
                    v-if="plugin.icon"
                    :src="plugin.icon"
                    :alt="plugin.display_name"
                    class="w-6 h-6 rounded shrink-0"
                  />
                  <div>
                    <h3 class="text-sm font-medium text-foreground">{{ plugin.display_name }}</h3>
                    <p class="text-[11px] text-muted/20 mt-0.5">
                      v{{ plugin.version }}
                      <span v-if="plugin.submitted_by"> &middot; from {{ plugin.submitted_by }}</span>
                    </p>
                  </div>
                </div>
                <div class="flex items-center gap-2">
                  <span
                    class="text-[10px] px-1.5 py-0.5 rounded"
                    :class="categoryColor(plugin.category)"
                  >{{ plugin.category }}</span>
                  <Button variant="solid" color="accent" size="xs" @click="approvePlugin(plugin.id)">
                    Approve
                  </Button>
                  <Button variant="solid" color="muted" size="xs" @click="rejectPlugin(plugin.id)">
                    Reject
                  </Button>
                  <Button variant="solid" color="muted" size="xs" @click="handleRemovePlugin(plugin)">
                    Remove
                  </Button>
                </div>
              </div>
              <p class="text-xs text-muted/40 mb-3">{{ plugin.description }}</p>

              <button
                class="text-xs text-muted/30 hover:text-muted/50 transition-colors"
                @click="toggleCode(plugin.id)"
              >
                {{ expandedCode === plugin.id ? 'Hide Code' : 'View Code' }}
              </button>

              <div v-if="expandedCode === plugin.id" class="mt-3 p-3 bg-surface/10 rounded-md overflow-auto max-h-64">
                <pre class="text-[11px] text-muted/40 font-mono whitespace-pre-wrap">{{ plugin.plugin_file }}</pre>
              </div>
            </div>
          </GroundGlass>
        </div>
      </template>
    </div>

    <!-- Submit Plugin Dialog -->
    <Teleport to="body">
      <div v-if="showSubmitDialog" class="fixed inset-0 z-50 flex items-center justify-center bg-black/35" @click.self="showSubmitDialog = false">
        <GroundGlass :opacity="3" :blur="10" :sizes="['70%', '70%', '65%']" class="p-8 w-full max-w-md" style="background: rgba(30, 41, 59, 0.30)">
          <div class="relative z-[1]">
            <h3 class="text-sm font-medium tracking-widest uppercase text-foreground/70 mb-6">Submit Plugin</h3>

            <!-- URL input -->
            <div class="mb-4">
              <label class="text-xs text-muted/30 mb-1.5 block">Install from URL</label>
              <GlassInput v-model="submitUrl" placeholder="https://example.com/plugin.ts or .zip" />
            </div>

            <div class="flex items-center gap-4 mb-6">
              <Button
                variant="solid"
                color="accent"
                size="sm"
                :disabled="!submitUrl || submitting"
                @click="handleSubmitFromUrl"
              >
                {{ submitting ? 'Loading...' : 'Submit from URL' }}
              </Button>

              <span class="text-xs text-muted/20">or</span>

              <Button
                variant="solid"
                color="default"
                size="sm"
                :disabled="submitting"
                @click="handleSubmitFromFile"
              >
                Choose .ts file
              </Button>

              <Button
                variant="solid"
                color="default"
                size="sm"
                :disabled="submitting"
                @click="handleSubmitFromFolder"
              >
                Choose Folder
              </Button>
            </div>

            <p v-if="submitError" class="text-xs text-red-400/80 mb-4">{{ submitError }}</p>

            <div class="flex justify-end">
              <Button variant="solid" color="muted" size="sm" @click="showSubmitDialog = false">Cancel</Button>
            </div>
          </div>
        </GroundGlass>
      </div>
    </Teleport>

    <!-- Configure Credentials Dialog -->
    <Teleport to="body">
      <div v-if="showConfigDialog && configPlugin" class="fixed inset-0 z-50 flex items-center justify-center bg-black/35" @click.self="showConfigDialog = false">
        <GroundGlass :opacity="3" :blur="10" :sizes="['70%', '70%', '65%']" class="p-8 w-full max-w-md" style="background: rgba(30, 41, 59, 0.30)">
          <div class="relative z-[1]">
            <h3 class="text-sm font-medium tracking-widest uppercase text-foreground/70 mb-2">Configure Credentials</h3>
            <p class="text-xs text-muted/30 mb-6">{{ configPlugin.display_name }} &middot; Company-wide settings</p>

            <div class="space-y-4 mb-8">
              <div v-for="field in (configPlugin.config_fields as VendorConfigField[])" :key="field.key">
                <label class="text-xs text-muted/40 mb-1.5 block">
                  {{ field.label }}
                  <span v-if="field.required" class="text-accent">*</span>
                </label>
                <GlassInput
                  v-model="configValues[field.key]"
                  :type="field.secret ? 'password' : 'text'"
                  :placeholder="field.placeholder || ''"
                />
              </div>
            </div>

            <div class="flex justify-end gap-4">
              <Button variant="solid" color="muted" size="sm" @click="showConfigDialog = false">Cancel</Button>
              <Button
                variant="solid"
                color="accent"
                size="sm"
                :disabled="configSaving"
                @click="handleSaveConfig"
              >
                {{ configSaving ? 'Saving...' : 'Save' }}
              </Button>
            </div>
          </div>
        </GroundGlass>
      </div>
    </Teleport>
  </div>
  </CompanyRequired>
</template>

<style scoped>
.marketplace-grid {
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(280px, 1fr));
  gap: 1rem;
  align-content: start;
}

.plugin-card {
  min-height: 160px;
}
</style>
