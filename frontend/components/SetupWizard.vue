<script setup lang="ts">
import type { Platform } from '~/types'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

const props = defineProps<{
  /** When provided, skip step 1 and jump straight to platform selection for this existing company. */
  existingCompanyId?: string
  existingCompanyName?: string
  /**
   * N-11.2: when re-entering for an existing company, jump straight to this
   * platform's step (deep-linked from credential-health warnings).
   */
  initialPlatform?: string
}>()

const emit = defineEmits<{
  complete: []
  cancel: []
}>()

const { company, createCompany } = useCompany()
const { fetchCompanies } = useCompanyContext()
const { fetchCompanyConfig, saveCompanyConfig } = useConfig()

// Wizard state — start at step 2 if configuring an existing company
const isExisting = !!props.existingCompanyId
const step = ref(isExisting ? 2 : 1)
const companyName = ref(props.existingCompanyName ?? '')
const creating = ref(false)
const createError = ref('')
const createdCompanyId = ref<string | null>(props.existingCompanyId ?? null)

const allPlatforms: Platform[] = ['ebay', 'squarespace', 'xmrbazaar', 'amazon']
const selectedPlatforms = ref<Set<Platform>>(new Set())
const platformIndex = ref(0)
const saving = ref(false)

// Platform descriptions for selection cards
const platformDescriptions: Record<Platform, string> = {
  ebay: 'Sync inventory with eBay listings via the Trading API.',
  squarespace: 'Sync inventory with your Squarespace Commerce store.',
  xmrbazaar: 'Sync inventory with XMR Bazaar listings.',
  amazon: 'Sync inventory with your Amazon Seller Central listings.',
}

// Platform config mirrors company config shape
const platformConfig = ref<Record<string, any>>({
  ebay: {
    enabled: false,
    client_id: '',
    client_secret: '',
    redirect_uri: '',
    environment: 'production',
  },
  squarespace: {
    enabled: false,
    api_key: '',
    user_agent: '',
  },
  xmrbazaar: {
    enabled: false,
    username: '',
    password: '',
    base_url: 'https://xmrbazaar.com',
  },
  amazon: {
    enabled: false,
    client_id: '',
    client_secret: '',
    refresh_token: '',
    seller_id: '',
    region: 'NA',
    marketplace_ids: ['ATVPDKIKX0DER'],
  },
  alerts: {
    default_low_stock_threshold: 5,
  },
})

// One-click eBay sign-in: offered when this build ships a hosted broker. The user's
// own developer keys stay available behind "Use my own keys".
const ebayBrokerUrl = ref('')
const brokerAvailable = computed(() => ebayBrokerUrl.value !== '')
const useOwnEbayKeys = ref(true)

function switchToEbayBroker() {
  // Broker mode is the absence of own keys, so they are cleared, not just hidden.
  const ebay = platformConfig.value.ebay
  ebay.client_id = ''
  ebay.client_secret = ''
  ebay.redirect_uri = ''
  ebay.environment = 'production'
  useOwnEbayKeys.value = false
}

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

// Guide instruction visibility per platform
const showInstructions = ref<Record<Platform, boolean>>({
  ebay: false,
  squarespace: false,
  xmrbazaar: false,
  amazon: false,
})

// N-11.1: per-platform Test connection state
type TestConnState = 'idle' | 'testing' | 'ok' | 'error'
const testConnection = ref<Record<Platform, { state: TestConnState; message: string }>>({
  ebay: { state: 'idle', message: '' },
  squarespace: { state: 'idle', message: '' },
  xmrbazaar: { state: 'idle', message: '' },
  amazon: { state: 'idle', message: '' },
})

// N-11.1: the exact eBay RuName (redirect URL) to register, per environment.
// Desktop apps use eBay's auth-accepted page as the redirect target; the
// in-app auth window captures the navigation, so the registered value only
// needs to match the Redirect URI field below character-for-character.
const EBAY_RUNAME_PROD =
  'https://signin.ebay.com/ws/eBayISAPI.dll?ThirdPartyAuthSucessFailure&isAuthSuccessful=true'
const EBAY_RUNAME_SANDBOX =
  'https://signin.sandbox.ebay.com/ws/eBayISAPI.dll?ThirdPartyAuthSucessFailure&isAuthSuccessful=true'
const ebayRecommendedRuName = computed(() =>
  platformConfig.value.ebay.environment === 'production' ? EBAY_RUNAME_PROD : EBAY_RUNAME_SANDBOX
)
const copiedRuName = ref(false)

function prefillEbayRuName() {
  if (!platformConfig.value.ebay.redirect_uri.trim()) {
    platformConfig.value.ebay.redirect_uri = ebayRecommendedRuName.value
  }
}

async function copyText(text: string) {
  try {
    await navigator.clipboard.writeText(text)
    copiedRuName.value = true
    setTimeout(() => { copiedRuName.value = false }, 2000)
  } catch {
    // Clipboard unavailable — the value is still visible for manual copy.
  }
}

// N-11.1: stage the platform's current fields (adapters are built from the
// saved company config, so the backend must see what the user typed), then
// probe the platform with a read-only request and report the result.
async function stagePlatformConfig(platform: Platform) {
  const existing = (await fetchCompanyConfig()) ?? {}
  await saveCompanyConfig({
    ...existing,
    [platform]: { ...platformConfig.value[platform], enabled: true },
  })
}

async function handleTestConnection(platform: Platform) {
  const t = testConnection.value[platform]
  t.state = 'testing'
  t.message = ''
  try {
    await stagePlatformConfig(platform)
    const summary = await invoke<string>('test_platform_connection', { platform })
    t.state = 'ok'
    t.message = summary
  } catch (e: any) {
    t.state = 'error'
    t.message = String(e)
  }
}

// Computed helpers
const selectedPlatformsList = computed(() =>
  allPlatforms.filter(p => selectedPlatforms.value.has(p))
)

const currentPlatform = computed(() =>
  selectedPlatformsList.value[platformIndex.value] ?? null
)

const isLastPlatform = computed(() =>
  platformIndex.value >= selectedPlatformsList.value.length - 1
)

// Step 1: Create company
async function handleCreate() {
  const name = companyName.value.trim()
  if (!name) return
  creating.value = true
  createError.value = ''
  try {
    await createCompany(name)
    // createCompany sets company.value with the new CompanyInfo (which has .id).
    // The backend already set this as the active company, so invoke() calls
    // in subsequent steps will target this company's DB. We intentionally
    // don't call switchCompany/fetchCompanies here to avoid triggering
    // app.vue's page re-mount which would destroy the wizard.
    if (company.value?.id) {
      createdCompanyId.value = company.value.id
    }
    step.value = 2
  } catch (e: any) {
    createError.value = e?.toString() || 'Failed to create company'
  } finally {
    creating.value = false
  }
}

// Step 2: Toggle platform selection
function togglePlatform(platform: Platform) {
  const s = new Set(selectedPlatforms.value)
  if (s.has(platform)) {
    s.delete(platform)
  } else {
    s.add(platform)
  }
  selectedPlatforms.value = s
}

function continueFromPlatformSelect() {
  if (selectedPlatforms.value.size === 0) {
    // Skip straight to done
    step.value = 4
    return
  }
  // Mark selected platforms as enabled
  for (const p of selectedPlatforms.value) {
    platformConfig.value[p].enabled = true
  }
  platformIndex.value = 0
  step.value = 3
}

// Step 3: Navigate between platform sub-steps
function nextPlatform() {
  if (isLastPlatform.value) {
    step.value = 4
  } else {
    platformIndex.value++
  }
}

function prevPlatform() {
  if (platformIndex.value > 0) {
    platformIndex.value--
  } else {
    step.value = 2
  }
}

// Step 4: Save and finish
async function handleFinish() {
  saving.value = true
  try {
    // Merge with existing company config if any. N-11.1: completing (or
    // skipping — the Skip button lands here too) is persisted so the main
    // window stops auto-opening the wizard for this company.
    const existing = await fetchCompanyConfig()
    const merged = { ...existing, ...platformConfig.value, setup_wizard_completed: true }
    await saveCompanyConfig(merged)
  } catch (e) {
    console.error('Failed to save config:', e)
  } finally {
    saving.value = false
  }
  if (!isExisting) {
    // Sync the frontend active company ID with the backend for newly created companies.
    // This will trigger app.vue's page re-mount, but the wizard is done
    // and we're about to navigate away.
    await fetchCompanies()
  }
  emit('complete')
}

// eBay OAuth
async function handleEbayCancel() {
  try {
    await invoke('cancel_ebay_auth')
  } finally {
    ebayAuthLoading.value = false
    ebayAuthMessage.value = ''
  }
}

async function handleEbayAuth() {
  ebayAuthLoading.value = true
  ebayAuthMessage.value = ''
  showManualCode.value = false
  try {
    // The auth window is driven by the adapter built from the *saved* config, so what
    // is on screen (broker mode, or the typed keys) must be saved first.
    await stagePlatformConfig('ebay')
    await invoke('start_ebay_auth')
    // One-click sign-in opens your own browser; your own keys use an eBay window here.
    ebayAuthMessage.value = useOwnEbayKeys.value
      ? 'Sign in on the eBay window...'
      : 'Finish signing in in your browser, then come back here.'
  } catch (e: any) {
    ebayAuthMessage.value = `Error: ${e}`
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
    ebayAuthMessage.value = 'Connected'
    showManualCode.value = false
    ebayAuthCode.value = ''
    setTimeout(() => { ebayAuthMessage.value = '' }, 5000)
  } catch (e: any) {
    ebayAuthMessage.value = `Error: ${e}`
  }
}

// Amazon OAuth
async function handleAmazonAuth() {
  amazonAuthLoading.value = true
  amazonAuthMessage.value = ''
  showAmazonManualCode.value = false
  try {
    await invoke('start_amazon_auth')
    amazonAuthMessage.value = 'Sign in on the Amazon window...'
  } catch (e: any) {
    amazonAuthMessage.value = `Error: ${e}`
    amazonAuthLoading.value = false
  }
}

async function handleAmazonManualComplete() {
  if (!amazonAuthCode.value.trim()) return
  amazonAuthMessage.value = ''
  try {
    await invoke('complete_amazon_auth', { code: amazonAuthCode.value.trim() })
    amazonAuthenticated.value = true
    amazonAuthMessage.value = 'Connected'
    showAmazonManualCode.value = false
    amazonAuthCode.value = ''
    setTimeout(() => { amazonAuthMessage.value = '' }, 5000)
  } catch (e: any) {
    amazonAuthMessage.value = `Error: ${e}`
  }
}

// Listen for eBay auth events
const unlisteners: (() => void)[] = []

onMounted(async () => {
  // N-11.1: when reconfiguring an existing company (settings re-entry), start
  // from the saved values so Test connection works against real credentials.
  // Runs in the settings window, the only context allowed to read secrets.
  if (isExisting) {
    try {
      const saved = await fetchCompanyConfig()
      if (saved) {
        for (const p of allPlatforms) {
          if (saved[p] && typeof saved[p] === 'object') {
            platformConfig.value[p] = { ...platformConfig.value[p], ...saved[p] }
          }
        }
      }
    } catch {
      // Secret commands are denied outside the settings window; the wizard
      // still works with blank fields there.
    }
  }

  try {
    ebayBrokerUrl.value = await invoke<string>('get_ebay_broker_url')
  } catch {
    ebayBrokerUrl.value = ''
  }
  // Saved own keys keep the manual form; otherwise one-click is the default when offered.
  useOwnEbayKeys.value = !brokerAvailable.value || !!platformConfig.value.ebay.client_id
  if (!useOwnEbayKeys.value) switchToEbayBroker()

  unlisteners.push(await listen<boolean>('ebay-auth-complete', () => {
    ebayAuthLoading.value = false
    ebayAuthenticated.value = true
    ebayAuthMessage.value = 'Connected'
    showManualCode.value = false
    ebayAuthCode.value = ''
    setTimeout(() => { ebayAuthMessage.value = '' }, 5000)
  }))

  unlisteners.push(await listen<string>('ebay-auth-error', (event) => {
    ebayAuthLoading.value = false
    ebayAuthMessage.value = `Error: ${event.payload}`
  }))

  unlisteners.push(await listen<boolean>('amazon-auth-complete', () => {
    amazonAuthLoading.value = false
    amazonAuthenticated.value = true
    amazonAuthMessage.value = 'Connected'
    showAmazonManualCode.value = false
    amazonAuthCode.value = ''
    setTimeout(() => { amazonAuthMessage.value = '' }, 5000)
  }))

  unlisteners.push(await listen<string>('amazon-auth-error', (event) => {
    amazonAuthLoading.value = false
    amazonAuthMessage.value = `Error: ${event.payload}`
  }))

  // N-11.2: deep-link from a credential-health warning straight to the
  // platform step that renews the credential (existing company only). The
  // saved config was pre-filled above, so jumping here loses nothing.
  if (isExisting && props.initialPlatform && (allPlatforms as string[]).includes(props.initialPlatform)) {
    const p = props.initialPlatform as Platform
    selectedPlatforms.value = new Set([p])
    platformConfig.value[p].enabled = true
    platformIndex.value = 0
    step.value = 3
  }
})

onUnmounted(() => {
  unlisteners.forEach(fn => fn())
})
</script>

<template>
  <div class="max-w-2xl">
    <!-- Step indicator -->
    <div class="flex items-center gap-3 mb-10">
      <template v-for="s in (isExisting ? [2, 3, 4] : [1, 2, 3, 4])" :key="s">
        <div
          class="w-2 h-2 rounded-full transition-colors"
          :class="s === step ? 'bg-accent' : s < step ? 'bg-accent/40' : 'bg-border/30'"
        />
        <div v-if="s < 4" class="w-8 h-px" :class="s < step ? 'bg-accent/30' : 'bg-border/20'" />
      </template>
    </div>

    <!-- STEP 1: Company Name -->
    <div v-if="step === 1" class="space-y-5">
      <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Create Company</h3>
      <div>
        <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Company Name</label>
        <input
          v-model="companyName"
          placeholder="Enter name"
          class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
          :disabled="creating"
          @keyup.enter="handleCreate"
        />
      </div>
      <p v-if="createError" class="text-xs text-red-400/80">{{ createError }}</p>
      <div class="flex gap-3 pt-3">
        <button
          class="px-4 py-1.5 text-xs rounded border border-accent/60 text-accent hover:bg-accent/10 transition-colors disabled:opacity-30"
          :disabled="creating || !companyName.trim()"
          @click="handleCreate"
        >
          {{ creating ? 'Creating...' : 'Create' }}
        </button>
        <button
          class="px-4 py-1.5 text-xs rounded border border-border/40 text-muted/60 hover:border-border/60 hover:text-muted/80 transition-colors"
          @click="emit('cancel')"
        >
          Cancel
        </button>
      </div>
    </div>

    <!-- STEP 2: Select Platforms -->
    <div v-else-if="step === 2" class="space-y-6">
      <div>
        <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-2">Select Platforms</h3>
        <p class="text-xs text-muted/30">Choose which platforms to configure. You can always add more later.</p>
      </div>

      <div class="space-y-2">
        <button
          v-for="platform in allPlatforms"
          :key="platform"
          class="w-full flex items-center gap-4 px-4 py-3.5 rounded-lg border transition-colors text-left"
          :class="selectedPlatforms.has(platform)
            ? 'bg-accent/[0.07] border-accent/20'
            : 'border-border/20 hover:bg-white/[0.03]'"
          @click="togglePlatform(platform)"
        >
          <!-- Checkbox -->
          <div
            class="w-4 h-4 rounded border flex items-center justify-center shrink-0 transition-colors"
            :class="selectedPlatforms.has(platform)
              ? 'bg-accent/20 border-accent/50'
              : 'border-border/30'"
          >
            <svg
              v-if="selectedPlatforms.has(platform)"
              class="w-3 h-3 text-accent"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="2.5"
            >
              <path d="M3 8l4 4 6-6" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          </div>

          <!-- Platform badge + description -->
          <div class="flex-1 min-w-0">
            <div class="flex items-center gap-2 mb-0.5">
              <PlatformBadge :platform="platform" />
            </div>
            <p class="text-xs text-muted/30">{{ platformDescriptions[platform] }}</p>
          </div>
        </button>
      </div>

      <div class="flex gap-3 pt-3">
        <button
          class="px-4 py-1.5 text-xs rounded border border-accent/60 text-accent hover:bg-accent/10 transition-colors"
          @click="continueFromPlatformSelect"
        >
          {{ selectedPlatforms.size > 0 ? 'Continue' : 'Skip' }}
        </button>
        <button
          class="px-4 py-1.5 text-xs rounded border border-border/40 text-muted/60 hover:border-border/60 hover:text-muted/80 transition-colors"
          @click="emit('cancel')"
        >
          Cancel
        </button>
      </div>
    </div>

    <!-- STEP 3: Configure Platforms -->
    <div v-else-if="step === 3 && currentPlatform" class="space-y-6">
      <div>
        <div class="flex items-center gap-3 mb-2">
          <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60">Configure</h3>
          <PlatformBadge :platform="currentPlatform" />
          <span class="text-[10px] text-muted/30">{{ platformIndex + 1 }} / {{ selectedPlatformsList.length }}</span>
        </div>
      </div>

      <!-- eBay Configuration -->
      <template v-if="currentPlatform === 'ebay'">
        <template v-if="useOwnEbayKeys">
        <!-- Instructions toggle -->
        <div>
          <button
            class="text-[11px] text-muted/30 hover:text-muted/50 transition-colors"
            @click="showInstructions.ebay = !showInstructions.ebay"
          >
            {{ showInstructions.ebay ? 'Hide instructions' : 'Show setup instructions' }}
          </button>
          <div v-if="showInstructions.ebay" class="mt-3 space-y-2 text-xs text-muted/50 leading-relaxed">
            <p>1. Go to <a href="https://developer.ebay.com" target="_blank" class="text-accent/70 hover:text-accent">developer.ebay.com</a>, sign in, and open <strong class="text-muted/70">Application Access Keys</strong>.</p>
            <p>2. Pick the keyset matching the <strong class="text-muted/70">Environment</strong> below (Production for real selling) and copy the <strong class="text-muted/70">App ID (Client ID)</strong> and <strong class="text-muted/70">Cert ID (Client Secret)</strong> into the fields below.</p>
            <p>3. Under <strong class="text-muted/70">OAuth &gt; eBay Redirect URL (RuName)</strong>, register exactly this value — then paste the same value into the Redirect URI field below. It must match character-for-character:</p>
            <div class="flex items-center gap-2">
              <code class="flex-1 min-w-0 text-[11px] font-mono text-muted/70 bg-white/[0.03] border border-border/20 rounded px-2 py-1.5 break-all">{{ ebayRecommendedRuName }}</code>
              <button
                class="text-[11px] text-accent hover:text-accent/80 transition-colors shrink-0"
                @click="copyText(ebayRecommendedRuName)"
              >
                {{ copiedRuName ? 'Copied' : 'Copy' }}
              </button>
            </div>
            <p>4. Use <strong class="text-muted/70">Authorize with eBay</strong> below to complete OAuth, then <strong class="text-muted/70">Test connection</strong> to verify everything works.</p>
            <img
              :src="'/guide/ebay-developer-console.png'"
              alt=""
              class="rounded border border-border/20 mt-2 max-w-md"
              @error="($event.target as HTMLImageElement).style.display = 'none'"
            />
            <img
              :src="'/guide/ebay-app-keys.png'"
              alt=""
              class="rounded border border-border/20 mt-2 max-w-md"
              @error="($event.target as HTMLImageElement).style.display = 'none'"
            />
          </div>
        </div>

        <!-- Credential fields -->
        <div class="grid grid-cols-2 gap-x-8 gap-y-5">
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Client ID</label>
            <input v-model="platformConfig.ebay.client_id" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Client Secret</label>
            <input v-model="platformConfig.ebay.client_secret" type="password" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Redirect URI (RuName)</label>
            <input v-model="platformConfig.ebay.redirect_uri" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
            <button
              class="text-[11px] text-accent/70 hover:text-accent transition-colors mt-1"
              @click="prefillEbayRuName"
            >
              Fill with the recommended RuName
            </button>
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Environment</label>
            <select v-model="platformConfig.ebay.environment" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors appearance-none">
              <option value="sandbox" class="bg-background">Sandbox</option>
              <option value="production" class="bg-background">Production</option>
            </select>
          </div>
        </div>

        <button
          v-if="brokerAvailable"
          class="text-[11px] text-accent/70 hover:text-accent transition-colors"
          @click="switchToEbayBroker"
        >
          Use one-click sign-in instead (clears these keys)
        </button>
        </template>

        <div v-else class="space-y-1.5">
          <p class="text-xs text-muted/50 leading-relaxed">
            Sign in with your eBay account. No developer account or keys needed.
          </p>
          <button
            class="text-[11px] text-muted/30 hover:text-muted/50 transition-colors"
            @click="useOwnEbayKeys = true"
          >
            Use my own eBay developer keys instead
          </button>
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
            <button
              v-if="ebayAuthLoading"
              class="ml-4 text-[11px] text-muted/40 hover:text-muted/60 transition-colors"
              @click="handleEbayCancel"
            >
              Cancel
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
      </template>

      <!-- Squarespace Configuration -->
      <template v-else-if="currentPlatform === 'squarespace'">
        <!-- Instructions toggle -->
        <div>
          <button
            class="text-[11px] text-muted/30 hover:text-muted/50 transition-colors"
            @click="showInstructions.squarespace = !showInstructions.squarespace"
          >
            {{ showInstructions.squarespace ? 'Hide instructions' : 'Show setup instructions' }}
          </button>
          <div v-if="showInstructions.squarespace" class="mt-3 space-y-2 text-xs text-muted/50 leading-relaxed">
            <p>1. In your Squarespace site, go to <strong class="text-muted/70">Settings &gt; Advanced &gt; Developer &gt; API Keys</strong>. (API keys are created inside your site — not in the <a href="https://developers.squarespace.com" target="_blank" class="text-accent/70 hover:text-accent">developer portal</a>, which hosts the API docs.)</p>
            <p>2. Click <strong class="text-muted/70">Generate Key</strong> and give it a descriptive name (e.g. "Nisaba Sync").</p>
            <p>3. Ensure the key has <strong class="text-muted/70">Commerce Read</strong> and <strong class="text-muted/70">Commerce Write</strong> scopes.</p>
            <p>4. Copy the API key and paste it below, then use <strong class="text-muted/70">Test connection</strong> to verify it. The User Agent can be any identifier for your app.</p>
            <img
              :src="'/guide/squarespace-api-keys.png'"
              alt=""
              class="rounded border border-border/20 mt-2 max-w-md"
              @error="($event.target as HTMLImageElement).style.display = 'none'"
            />
          </div>
        </div>

        <!-- Credential fields -->
        <div class="grid grid-cols-2 gap-x-8 gap-y-5">
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">API Key</label>
            <input v-model="platformConfig.squarespace.api_key" type="password" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">User Agent</label>
            <input v-model="platformConfig.squarespace.user_agent" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
        </div>
      </template>

      <!-- XMR Bazaar Configuration -->
      <template v-else-if="currentPlatform === 'xmrbazaar'">
        <!-- Instructions toggle -->
        <div>
          <button
            class="text-[11px] text-muted/30 hover:text-muted/50 transition-colors"
            @click="showInstructions.xmrbazaar = !showInstructions.xmrbazaar"
          >
            {{ showInstructions.xmrbazaar ? 'Hide instructions' : 'Show setup instructions' }}
          </button>
          <div v-if="showInstructions.xmrbazaar" class="mt-3 space-y-2 text-xs text-muted/50 leading-relaxed">
            <p>1. Use your account credentials from <a href="https://xmrbazaar.com" target="_blank" class="text-accent/70 hover:text-accent">xmrbazaar.com</a>.</p>
            <p>2. Enter the same username and password you use to log in to XMR Bazaar.</p>
            <p>3. The Base URL defaults to <code class="text-muted/60">https://xmrbazaar.com</code>. Only change this if you use a custom instance.</p>
            <p>4. Use <strong class="text-muted/70">Test connection</strong> below to verify the login works.</p>
            <img
              :src="'/guide/xmrbazaar-login.png'"
              alt=""
              class="rounded border border-border/20 mt-2 max-w-md"
              @error="($event.target as HTMLImageElement).style.display = 'none'"
            />
          </div>
        </div>

        <!-- Credential fields -->
        <div class="grid grid-cols-2 gap-x-8 gap-y-5">
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Username</label>
            <input v-model="platformConfig.xmrbazaar.username" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Password</label>
            <input v-model="platformConfig.xmrbazaar.password" type="password" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
          <div class="col-span-2">
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Base URL</label>
            <input v-model="platformConfig.xmrbazaar.base_url" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
        </div>
      </template>

      <!-- Amazon Configuration -->
      <template v-else-if="currentPlatform === 'amazon'">
        <!-- Instructions toggle -->
        <div>
          <button
            class="text-[11px] text-muted/30 hover:text-muted/50 transition-colors"
            @click="showInstructions.amazon = !showInstructions.amazon"
          >
            {{ showInstructions.amazon ? 'Hide instructions' : 'Show setup instructions' }}
          </button>
          <div v-if="showInstructions.amazon" class="mt-3 space-y-2 text-xs text-muted/50 leading-relaxed">
            <p><strong class="text-muted/70">Self-authorized private app:</strong> Nisaba connects to Amazon as a private Selling Partner API (SP-API) app that you authorize for your own seller account — it is never listed publicly. You register once as a developer, create the app, and authorize it yourself below.</p>
            <p>1. Register as a developer via the <a href="https://developer-docs.amazon.com/sp-api/" target="_blank" class="text-accent/70 hover:text-accent">SP-API developer docs</a>, then in <a href="https://sellercentral.amazon.com" target="_blank" class="text-accent/70 hover:text-accent">Seller Central &gt; Apps &amp; Services &gt; Develop Apps</a> create a new app.</p>
            <p>2. Under the app's <strong class="text-muted/70">LWA credentials</strong>, copy the <strong class="text-muted/70">Client ID</strong> and <strong class="text-muted/70">Client Secret</strong> into the fields below. Your <strong class="text-muted/70">Seller ID</strong> is on the Seller Central <strong class="text-muted/70">Account Info</strong> page.</p>
            <p>3. When Amazon asks which SP-API <strong class="text-muted/70">roles</strong> the app needs, select <strong class="text-muted/70">Product Listing</strong> — the only role Nisaba uses (it reads and updates your own listings and inventory through the Listings Items API). The Test connection check below needs no role at all.</p>
            <p>4. <strong class="text-muted/70">Approval caveat:</strong> Amazon must approve your developer registration before the app works, and any restricted role (anything touching buyer personal data) needs separate Amazon approval — that decision is Amazon's and can take several days. Product Listing is a direct, unrestricted role for your own private app: it is available as soon as your developer registration completes. Nisaba never requests or touches buyer PII.</p>
            <p>5. Select your region (NA, EU, or FE) and the marketplace IDs you sell in. Leave <strong class="text-muted/70">Refresh token</strong> blank — after you use <strong class="text-muted/70">Authorize with Amazon</strong> below, Nisaba stores the tokens itself. Finish with <strong class="text-muted/70">Test connection</strong>.</p>
          </div>
        </div>

        <!-- Credential fields -->
        <div class="grid grid-cols-2 gap-x-8 gap-y-5">
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Client ID (LWA)</label>
            <input v-model="platformConfig.amazon.client_id" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Client Secret</label>
            <input v-model="platformConfig.amazon.client_secret" type="password" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Seller ID</label>
            <input v-model="platformConfig.amazon.seller_id" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors">
          </div>
          <div>
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Region</label>
            <select v-model="platformConfig.amazon.region" class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground focus:outline-none focus:border-accent/50 transition-colors appearance-none">
              <option value="NA" class="bg-background">North America (NA)</option>
              <option value="EU" class="bg-background">Europe (EU)</option>
              <option value="FE" class="bg-background">Far East (FE)</option>
            </select>
          </div>
          <div class="col-span-2">
            <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Marketplace IDs (comma-separated)</label>
            <input
              :value="platformConfig.amazon.marketplace_ids.join(', ')"
              class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground font-mono focus:outline-none focus:border-accent/50 transition-colors"
              @input="platformConfig.amazon.marketplace_ids = ($event.target as HTMLInputElement).value.split(',').map((s: string) => s.trim()).filter(Boolean)"
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
      </template>

      <!-- N-11.1: Test connection (per platform) -->
      <div class="border-t border-border/20 pt-5">
        <div class="flex items-center gap-3">
          <button
            class="px-4 py-1.5 text-xs rounded border border-accent/60 text-accent hover:bg-accent/10 transition-colors disabled:opacity-30"
            :disabled="testConnection[currentPlatform].state === 'testing'"
            @click="handleTestConnection(currentPlatform)"
          >
            {{ testConnection[currentPlatform].state === 'testing' ? 'Testing…' : 'Test connection' }}
          </button>
          <span
            v-if="testConnection[currentPlatform].message"
            class="text-xs"
            :class="testConnection[currentPlatform].state === 'ok' ? 'text-green-400/80' : 'text-red-400/80'"
          >{{ testConnection[currentPlatform].message }}</span>
        </div>
        <p class="text-[11px] text-muted/30 mt-2">
          Saves the fields above, then makes a single read-only request to {{ currentPlatform }} to verify the credentials.
        </p>
      </div>

      <!-- Navigation -->
      <div class="flex gap-3 pt-3">
        <button
          class="px-4 py-1.5 text-xs rounded border border-accent/60 text-accent hover:bg-accent/10 transition-colors"
          @click="nextPlatform"
        >
          {{ isLastPlatform ? 'Finish' : 'Next' }}
        </button>
        <button
          class="px-4 py-1.5 text-xs rounded border border-border/40 text-muted/60 hover:border-border/60 hover:text-muted/80 transition-colors"
          @click="prevPlatform"
        >
          Back
        </button>
      </div>
    </div>

    <!-- STEP 4: Done / Summary -->
    <div v-else-if="step === 4" class="space-y-6">
      <div>
        <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-2">Setup Complete</h3>
        <p class="text-xs text-muted/30">Your company is ready to go.</p>
      </div>

      <div class="space-y-3">
        <div class="flex items-center gap-3">
          <span class="text-[11px] text-muted/40 uppercase tracking-wider w-20">Company</span>
          <span class="text-sm text-foreground">{{ companyName }}</span>
        </div>
        <div class="flex items-center gap-3">
          <span class="text-[11px] text-muted/40 uppercase tracking-wider w-20">Platforms</span>
          <div v-if="selectedPlatformsList.length > 0" class="flex gap-2">
            <PlatformBadge v-for="p in selectedPlatformsList" :key="p" :platform="p" />
          </div>
          <span v-else class="text-xs text-muted/30">None configured</span>
        </div>
      </div>

      <div class="flex gap-3 pt-3">
        <button
          class="px-4 py-1.5 text-xs rounded border border-accent/60 text-accent hover:bg-accent/10 transition-colors disabled:opacity-30"
          :disabled="saving"
          @click="handleFinish"
        >
          {{ saving ? 'Saving...' : 'Go to Dashboard' }}
        </button>
        <button
          v-if="selectedPlatformsList.length > 0"
          class="px-4 py-1.5 text-xs rounded border border-border/40 text-muted/60 hover:border-border/60 hover:text-muted/80 transition-colors"
          @click="step = 3; platformIndex = 0"
        >
          Back to platforms
        </button>
      </div>
    </div>
  </div>
</template>
