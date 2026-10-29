<script setup lang="ts">
import type { AppConfig, BackupInfo } from '~/types'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

const { config, loading, fetchConfig, saveConfig, fetchCompanyConfig, saveCompanyConfig } = useConfig()
const { activeCompanyId, activeCompany } = useCompanyContext()
const { installedPlugins, fetchPlugins, setPluginConfig, getPluginConfig } = useVendors()

// The Settings page owns the tab strip; this panel keeps its edit state (and
// unsaved changes) across all three of its tabs.
const props = defineProps<{ tab: 'local' | 'company' | 'plugins' }>()
const activeTab = computed(() => props.tab)
const saving = ref(false)

const { notify } = useNotifications()

// N-11.3: restoring a backup replaces the whole database — confirm through
// the single shared <ConfirmDialog> component.
const { dialog, confirming, requestConfirm, confirmDialogAction, cancelDialog } = useConfirmDialog()

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
  await fetchBackups()
})

// ── N-8.1: database backups ──────────────────────────────────────────────
// The backup schedule settings (backup_enabled / backup_keep_last /
// backup_interval_hours) are part of editConfig.database and save through
// the normal config save flow. The list below is the app database's backups.
const backups = ref<BackupInfo[]>([])
const backingUp = ref(false)
const restoringPath = ref<string | null>(null)
const backupMessage = ref('')

async function fetchBackups() {
  try {
    backups.value = await invoke<BackupInfo[]>('list_backups')
  } catch {
    backups.value = []
  }
}

function formatBackupSize(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`
  if (bytes < 1024 * 1024) return `${(bytes / 1024).toFixed(1)} KB`
  return `${(bytes / (1024 * 1024)).toFixed(1)} MB`
}

function formatBackupDate(iso: string): string {
  const d = new Date(iso)
  return isNaN(d.getTime()) ? iso : d.toLocaleString()
}

function formatBackupReason(reason: string): string {
  return reason.split('-').map(w => w.charAt(0).toUpperCase() + w.slice(1)).join(' ')
}

async function runBackupNow() {
  backingUp.value = true
  backupMessage.value = ''
  try {
    const path = await invoke<string>('backup_now')
    backupMessage.value = `Backup written: ${path}`
    notify({ type: 'success', title: 'Backup complete', message: path, source: 'config' })
    await fetchBackups()
  } catch (e) {
    const msg = String(e)
    backupMessage.value = msg
    notify({ type: 'error', title: 'Backup failed', message: msg, detail: msg, source: 'config' })
  } finally {
    backingUp.value = false
  }
}

function restoreBackup(b: BackupInfo) {
  requestConfirm({
    title: 'Restore backup',
    message:
      `Restore the database from backup "${b.name}" (${formatBackupDate(b.created_at)})?\n\n` +
      'Everything written after this backup will be lost. A safety backup of the current database ' +
      'is taken first. The app will restart after the restore.',
    confirmLabel: 'Restore',
    action: async () => {
      restoringPath.value = b.path
      try {
        await invoke('restore_backup', { backup_path: b.path })
        notify({ type: 'info', title: 'Restore complete', message: 'Restarting the app…', source: 'config' })
        // The database file was swapped on disk; open connections still cache
        // the old file, so the app must restart to see the restored state.
        await invoke('plugin:process|restart')
      } catch (e) {
        const msg = String(e)
        notify({ type: 'error', title: 'Restore failed', message: msg, detail: msg, source: 'config' })
        restoringPath.value = null
      }
    },
  })
}

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

// Sync interval picker. The backend only supports schedules of the form
// `0 */N * * * *` (every N minutes) and rejects anything else on save —
// including */0, which used to kill the sync engine — so the UI offers a
// fixed set of intervals instead of a free-form cron field.
const SYNC_INTERVAL_MINUTE_OPTIONS = [1, 5, 10, 15, 30, 60, 120, 240, 480, 720, 1440]

function parseScheduleMinutes(schedule: string): number | null {
  const match = schedule.trim().match(/^0 \*\/(\d+) \* \* \* \*$/)
  if (!match) return null
  const minutes = Number.parseInt(match[1], 10)
  return minutes >= 1 ? minutes : null
}

function formatIntervalMinutes(minutes: number): string {
  if (minutes < 60) return minutes === 1 ? '1 minute' : `${minutes} minutes`
  if (minutes % 60 === 0) {
    const hours = minutes / 60
    return hours === 1 ? '1 hour' : `${hours} hours`
  }
  return `${minutes} minutes`
}

const syncIntervalOptions = computed(() => {
  const options = [...SYNC_INTERVAL_MINUTE_OPTIONS]
  const current = editConfig.value
    ? parseScheduleMinutes(editConfig.value.general.sync_schedule)
    : null
  // Keep an off-list stored value selectable so opening the page never
  // silently rewrites it; it is replaced only when the user picks another.
  if (current !== null && !options.includes(current)) {
    options.push(current)
    options.sort((a, b) => a - b)
  }
  return options
})

const syncIntervalMinutes = computed({
  get: () => {
    if (!editConfig.value) return 5
    return parseScheduleMinutes(editConfig.value.general.sync_schedule) ?? 5
  },
  set: (minutes: number) => {
    if (!editConfig.value || !Number.isFinite(minutes) || minutes < 1) return
    editConfig.value.general.sync_schedule = `0 */${Math.trunc(minutes)} * * * *`
  },
})

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
  const raw = ebayAuthCode.value.trim()
  if (!raw) return
  // The backend verifies the OAuth `state` issued with the authorize URL, so
  // the manual fallback needs the full redirect URL (code + state), not just
  // a bare code.
  let code = raw
  let returnedState: string | null = null
  try {
    const parsed = new URL(raw)
    const urlCode = parsed.searchParams.get('code')
    if (urlCode) {
      code = urlCode
      returnedState = parsed.searchParams.get('state')
    }
  } catch {
    // Not a URL; a bare code is passed through and rejected by the
    // backend's state check.
  }
  ebayAuthMessage.value = ''
  try {
    await invoke('complete_ebay_auth', { code, returned_state: returnedState })
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

// N-11.1: re-enter the first-run setup wizard from settings.
// The onBeforeRouteLeave guard above still prompts on unsaved changes.
function openSetupWizard() {
  navigateTo({
    path: '/setup',
    query: {
      companyId: activeCompanyId.value ?? '',
      companyName: activeCompany.value?.name ?? '',
      from: 'settings',
    },
  })
}

async function handleSaveCompany() {  if (!editCompanyConfig.value) return
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
    <div class="flex items-end justify-end mb-6 shrink-0 max-w-2xl min-h-4">
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

    <div class="flex-1 overflow-auto min-h-0 -mr-6 pr-6">
    <div v-if="loading" class="py-16 text-sm text-muted/40">Loading config...</div>

    <!-- LOCAL TAB -->
    <div v-else-if="activeTab === 'local' && editConfig" class="space-y-14 max-w-2xl">
      <!-- General -->
      <section>
        <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">General</h3>
        <div class="grid grid-cols-3 gap-x-8 gap-y-5">
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Sync interval</label>
            <select v-model.number="syncIntervalMinutes" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors appearance-none">
              <option v-for="opt in syncIntervalOptions" :key="opt" :value="opt" class="bg-background">Every {{ formatIntervalMinutes(opt) }}</option>
            </select>
            <p class="mt-1.5 text-[11px] text-muted/40">Applies on save, no restart needed.</p>
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Max retries</label>
            <input v-model.number="editConfig.general.max_retries" type="number" min="0" max="10" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
        </div>
      </section>

      <!-- Database backups (N-8.1) -->
      <section>
        <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Database backups</h3>
        <p class="text-xs text-muted/50 mb-5 max-w-xl">
          A timestamped backup is written next to the database before every migration and on the
          schedule below. Backups never leave this machine. Restoring replaces the database file
          and restarts the app.
        </p>
        <div class="grid grid-cols-3 gap-x-8 gap-y-5">
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Automatic backups</label>
            <select v-model="editConfig.database.backup_enabled" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors appearance-none">
              <option :value="true" class="bg-background">Enabled</option>
              <option :value="false" class="bg-background">Disabled</option>
            </select>
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Keep last N</label>
            <input v-model.number="editConfig.database.backup_keep_last" type="number" min="1" max="100" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Backup every (hours)</label>
            <input v-model.number="editConfig.database.backup_interval_hours" type="number" min="0" max="720" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
            <p class="mt-1.5 text-[11px] text-muted/40">0 disables the schedule. Applies on save, no restart needed.</p>
          </div>
        </div>
        <div class="mt-6 flex items-center gap-3">
          <button
            @click="runBackupNow"
            :disabled="backingUp"
            class="text-xs font-medium px-4 py-2 rounded border border-border/40 text-foreground/80 hover:border-accent/50 hover:text-foreground transition-colors disabled:opacity-40"
          >
            {{ backingUp ? 'Backing up…' : 'Back up now' }}
          </button>
          <span v-if="backupMessage" class="text-xs text-muted/50 truncate max-w-md">{{ backupMessage }}</span>
        </div>
        <div class="mt-6">
          <h4 class="text-[11px] text-muted/40 uppercase tracking-wider mb-3">Backups</h4>
          <div v-if="backups.length === 0" class="text-xs text-muted/40 py-2">No backups yet.</div>
          <div v-else class="divide-y divide-white/[0.06]">
            <div v-for="b in backups" :key="b.path" class="flex items-center gap-4 py-2.5">
              <div class="flex-1 min-w-0">
                <div class="text-sm text-foreground/90 truncate">{{ b.name }}</div>
                <div class="text-[11px] text-muted/40">
                  {{ formatBackupDate(b.created_at) }} · {{ formatBackupSize(b.size_bytes) }} · {{ formatBackupReason(b.reason) }}
                </div>
              </div>
              <button
                @click="restoreBackup(b)"
                :disabled="restoringPath !== null"
                class="text-xs font-medium px-3 py-1.5 rounded border border-red-500/30 text-red-400/80 hover:border-red-500/60 hover:text-red-300 transition-colors disabled:opacity-40 shrink-0"
              >
                {{ restoringPath === b.path ? 'Restoring…' : 'Restore' }}
              </button>
            </div>
          </div>
        </div>
      </section>

      <!-- Updates (N-10.1) -->
      <section>
        <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Updates</h3>
        <div class="grid grid-cols-3 gap-x-8 gap-y-5">
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Check for updates</label>
            <select v-model="editConfig.updates.check_for_updates" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors appearance-none">
              <option :value="true" class="bg-background">Enabled</option>
              <option :value="false" class="bg-background">Disabled</option>
            </select>
            <p class="mt-1.5 text-[11px] text-muted/40">When enabled, Nisaba checks for a new release at launch and every 6 hours. Applies on save; takes effect at the next launch.</p>
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Install updates automatically</label>
            <select v-model="editConfig.updates.auto_install" :disabled="!editConfig.updates.check_for_updates" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors appearance-none disabled:opacity-30">
              <option :value="true" class="bg-background">Enabled</option>
              <option :value="false" class="bg-background">Disabled</option>
            </select>
            <p class="mt-1.5 text-[11px] text-muted/40">When enabled, an update found at launch is installed straight away and Nisaba restarts into it. Updates found later while you work only show the banner. Needs update checks on.</p>
          </div>
        </div>
      </section>
    </div>

    <!-- COMPANY TAB -->
    <div v-else-if="activeTab === 'company' && editCompanyConfig" class="space-y-14 max-w-2xl">
      <!-- N-11.1: re-enter the first-run setup wizard -->
      <section>
        <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Platform setup</h3>
        <p class="text-xs text-muted/50 mb-4 max-w-xl">
          Re-run the first-run setup wizard to add platforms or step through each platform's
          credentials again, with per-platform connection testing.
        </p>
        <button
          @click="openSetupWizard"
          class="text-xs font-medium px-4 py-2 rounded border border-border/40 text-foreground/80 hover:border-accent/50 hover:text-foreground transition-colors"
        >
          Open setup wizard
        </button>
      </section>

      <!-- N-11.2: credential health — per-platform expiry state with advance
           warnings, each linking back to the wizard step that renews it. -->
      <section>
        <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Credential health</h3>
        <p class="text-xs text-muted/50 mb-4 max-w-xl">
          When each platform's token or keys expire, with a warning ahead of time so a
          mid-sync refresh never fails silently. Renewals happen in the setup wizard.
        </p>
        <CredentialHealthPanel mode="full" />
      </section>

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
                  {{ showManualCode ? 'Hide manual entry' : 'Paste redirect URL manually' }}
                </button>

                <div v-if="showManualCode" class="mt-3 space-y-2">
                  <p class="text-[11px] text-muted/30">Paste the full eBay redirect URL (it carries the authorization code and the state for verification):</p>
                  <div class="flex items-center gap-3">
                    <input
                      v-model="ebayAuthCode"
                      type="text"
                      placeholder="https://...?code=...&state=..."
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

  <ConfirmDialog
    :open="dialog.open"
    :title="dialog.title"
    :message="dialog.message"
    :confirm-label="dialog.confirmLabel"
    :danger="dialog.danger"
    :confirming="confirming"
    @confirm="confirmDialogAction"
    @cancel="cancelDialog"
  />
</template>
