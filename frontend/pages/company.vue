<script setup lang="ts">
import { confirm } from '@tauri-apps/plugin-dialog'
import type { P2PEvent } from '~/types'

const {
  company,
  loading,
  onionAddress,
  syncing,
  fetchCompany,
  leaveCompany,
  getOnionAddress,
  getCompanySecret,
  addPeer,
  removePeer,
  approvePeer,
  rejectPeer,
  setPeerRole,
  updateLogo,
  removeLogo,
  triggerSync,
  listenForEvents,
} = useCompany()

const { fetchCompanies } = useCompanyContext()

// Add peer form
const showAddPeer = ref(false)
const newPeerAddress = ref('')
const newPeerName = ref('')

// Secret visibility
const secretVisible = ref(false)
const companySecret = ref<string | null>(null)

const { notify } = useNotifications()

function showStatus(msg: string, type: 'success' | 'error' = 'success') {
  notify({
    type: type === 'error' ? 'error' : 'success',
    title: type === 'error' ? 'Error' : 'Success',
    message: msg,
    source: 'company',
  })
}

// Logo upload
const logoInput = ref<HTMLInputElement | null>(null)

function triggerLogoUpload() {
  logoInput.value?.click()
}

async function handleLogoFile(e: Event) {
  const file = (e.target as HTMLInputElement).files?.[0]
  if (!file) return

  // Validate size (max 2MB raw)
  if (file.size > 2 * 1024 * 1024) {
    showStatus('Image too large (max 2MB)', 'error')
    return
  }

  // Resize to 128x128 max via canvas
  const img = new Image()
  const reader = new FileReader()

  reader.onload = () => {
    img.onload = async () => {
      const canvas = document.createElement('canvas')
      const maxSize = 128
      let w = img.width
      let h = img.height

      if (w > maxSize || h > maxSize) {
        if (w > h) {
          h = Math.round((h / w) * maxSize)
          w = maxSize
        } else {
          w = Math.round((w / h) * maxSize)
          h = maxSize
        }
      }

      canvas.width = w
      canvas.height = h
      const ctx = canvas.getContext('2d')!
      ctx.drawImage(img, 0, 0, w, h)

      const dataUrl = canvas.toDataURL('image/png')

      // Validate encoded size (max 256KB)
      if (dataUrl.length > 256 * 1024) {
        showStatus('Compressed image too large', 'error')
        return
      }

      await updateLogo(dataUrl)
      await fetchCompanies() // refresh registry
      showStatus('Logo updated')
    }
    img.src = reader.result as string
  }
  reader.readAsDataURL(file)

  // Reset input so the same file can be selected again
  if (logoInput.value) logoInput.value.value = ''
}

async function handleRemoveLogo() {
  await removeLogo()
  await fetchCompanies()
  showStatus('Logo removed')
}

// Address helpers — strip .onion for display, add it back for internal use
function stripOnion(addr: string) {
  return addr.replace(/\.onion$/, '')
}

function ensureOnion(addr: string) {
  return addr.endsWith('.onion') ? addr : addr + '.onion'
}

function truncateAddress(addr: string) {
  const clean = stripOnion(addr)
  if (clean.length <= 20) return clean
  return clean.slice(0, 12) + '...' + clean.slice(-8)
}

// Copy to clipboard
async function copyToClipboard(text: string) {
  await navigator.clipboard.writeText(text)
  showStatus('Copied to clipboard')
}

// Copy secret (fetches from backend if not already loaded)
async function copySecret() {
  try {
    const secret = companySecret.value || await getCompanySecret()
    if (secret) {
      await navigator.clipboard.writeText(secret)
      showStatus('Secret copied to clipboard')
    } else {
      showStatus('Secret not available', 'error')
    }
  } catch (e: any) {
    showStatus(e?.toString() || 'Failed to copy secret', 'error')
  }
}

// Reveal secret
async function revealSecret() {
  if (secretVisible.value) {
    secretVisible.value = false
    companySecret.value = null
    return
  }
  try {
    const secret = await getCompanySecret()
    if (secret) {
      companySecret.value = secret
      secretVisible.value = true
    } else {
      showStatus('Secret not available', 'error')
    }
  } catch (e: any) {
    showStatus(e?.toString() || 'Failed to retrieve secret', 'error')
  }
}

// Add peer handler
async function handleAddPeer() {
  if (!newPeerAddress.value.trim()) return
  await addPeer(ensureOnion(newPeerAddress.value.trim()), newPeerName.value.trim() || 'Peer')
  showAddPeer.value = false
  newPeerAddress.value = ''
  newPeerName.value = ''
  showStatus('Peer added')
}

// Leave handler
async function handleLeave() {
  const peers = approvedPeers.value
  const hasPeers = peers.length > 0

  // Admin with peers: must ensure another admin exists
  if (isAdmin.value && hasPeers) {
    const otherAdmins = peers.filter(p => p.role === 'admin')
    if (otherAdmins.length === 0) {
      showStatus('Promote another peer to admin before leaving', 'error')
      return
    }
  }

  // Build confirmation message
  let message: string
  if (!hasPeers) {
    message = 'You are the only member. Leaving will delete all company data from this device.\n\nMake sure to export your products first if you want to keep them.'
  } else {
    message = 'Leaving will delete all company data from this device. Sync will stop and you will lose access to company products.'
  }

  const confirmed = await confirm(message, {
    title: 'Leave Company',
    kind: 'warning',
    okLabel: 'Leave',
    cancelLabel: 'Cancel',
  })
  if (!confirmed) return

  try {
    await leaveCompany()
    showStatus('Left company')
  } catch (e: any) {
    showStatus(e?.toString() || 'Failed to leave company', 'error')
  }
}

// Computed helpers
const isAdmin = computed(() => company.value?.role === 'admin')

const approvedPeers = computed(() =>
  company.value?.peers.filter(p => p.is_authorized) ?? [],
)

const pendingPeers = computed(() =>
  company.value?.peers.filter(p => !p.is_authorized) ?? [],
)

const ourDisplayAddress = computed(() => {
  const raw = onionAddress.value || company.value?.our_onion
  return raw ? stripOnion(raw) : null
})

function formatTime(ts: string | null) {
  if (!ts) return 'Never'
  const d = new Date(ts)
  return d.toLocaleTimeString()
}

// Event listener
let unlisten: (() => void) | null = null

onMounted(async () => {
  await fetchCompany()
  await getOnionAddress()
  const u = await listenForEvents((event: P2PEvent) => {
    if (event.type === 'SyncCompleted') {
      showStatus(`Synced with ${truncateAddress(event.peer)}`)
    } else if (event.type === 'SyncFailed') {
      showStatus(`Sync failed: ${event.error}`, 'error')
    } else if (event.type === 'PeerJoinRequest') {
      showStatus(`Join request from ${event.name}`)
    }
  })
  unlisten = u
})

onUnmounted(() => {
  unlisten?.()
})
</script>

<template>
  <div class="flex flex-col h-full">
    <div class="flex items-end justify-between mb-10 shrink-0 max-w-2xl">
      <h2 class="text-sm font-medium tracking-widest uppercase text-muted/60">Company</h2>
    </div>

    <div class="flex-1 overflow-auto min-h-0 -mr-6 pr-6">
    <!-- Loading -->
    <div v-if="loading" class="text-xs text-muted/40">Loading...</div>

    <!-- No Company State -->
    <template v-else-if="!company">
      <div class="space-y-14">
        <p class="text-sm text-muted/40">
          No active company.
        </p>
        <NuxtLink
          to="/companies"
          class="inline-block px-4 py-1.5 text-xs rounded border border-accent/60 text-accent hover:bg-accent/10 transition-colors"
        >
          Manage Companies
        </NuxtLink>
      </div>
    </template>

    <!-- Active Company State -->
    <template v-else>
      <div class="space-y-14">

        <!-- Company Info -->
        <section class="space-y-5">
          <div class="flex items-center gap-4">
            <!-- Logo -->
            <button
              class="relative group shrink-0 w-10 h-10 rounded-full overflow-hidden border border-border/30 hover:border-accent/40 transition-colors bg-white/[0.03] flex items-center justify-center"
              title="Click to change logo"
              @click="triggerLogoUpload"
            >
              <img
                v-if="company.logo"
                :src="company.logo"
                class="w-full h-full object-cover"
              />
              <svg v-else class="w-4 h-4 text-muted/30" viewBox="0 0 16 16" fill="currentColor">
                <path d="M2 3a1 1 0 0 1 1-1h4.586a1 1 0 0 1 .707.293l1.414 1.414a1 1 0 0 0 .707.293H13a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V3z" />
              </svg>
              <div class="absolute inset-0 bg-black/40 opacity-0 group-hover:opacity-100 transition-opacity flex items-center justify-center">
                <svg class="w-3.5 h-3.5 text-white/80" viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5">
                  <path d="M8 3v10M3 8h10" stroke-linecap="round" />
                </svg>
              </div>
            </button>
            <input
              ref="logoInput"
              type="file"
              accept="image/*"
              class="hidden"
              @change="handleLogoFile"
            />

            <div class="flex items-center gap-3">
              <span class="text-sm text-foreground">{{ company.name }}</span>
              <span class="text-[11px] text-muted/40 uppercase tracking-wider">{{ company.role }}</span>
            </div>

            <button
              v-if="company.logo && isAdmin"
              class="text-[11px] text-muted/30 hover:text-red-400/80 transition-colors"
              @click="handleRemoveLogo"
            >
              Remove logo
            </button>
          </div>

          <div class="grid grid-cols-2 gap-x-8 gap-y-5">
            <!-- Our address -->
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Address</label>
              <div v-if="ourDisplayAddress" class="flex items-center gap-3">
                <span
                  class="text-sm font-mono text-foreground truncate cursor-default"
                  :title="ourDisplayAddress"
                >{{ ourDisplayAddress }}</span>
                <button
                  class="text-[11px] text-muted/30 hover:text-muted/50 transition-colors shrink-0"
                  @click="copyToClipboard(ourDisplayAddress!)"
                >
                  Copy
                </button>
              </div>
              <span v-else class="text-sm text-muted/30 animate-pulse">Connecting...</span>
            </div>

            <!-- Company secret (admin only) -->
            <div v-if="isAdmin">
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Secret</label>
              <div class="flex items-center gap-3">
                <span v-if="secretVisible && companySecret" class="text-sm font-mono text-foreground truncate">
                  {{ companySecret }}
                </span>
                <span v-else class="text-sm text-muted/30 font-mono">••••••••••••••••</span>
                <button
                  class="text-[11px] text-muted/30 hover:text-muted/50 transition-colors shrink-0"
                  @click="revealSecret"
                >
                  {{ secretVisible ? 'Hide' : 'Reveal' }}
                </button>
                <button
                  class="text-[11px] text-muted/30 hover:text-muted/50 transition-colors shrink-0"
                  @click="copySecret"
                >
                  Copy
                </button>
              </div>
            </div>
          </div>
        </section>

        <!-- Pending Join Requests (admin only) -->
        <section v-if="isAdmin && pendingPeers.length > 0" class="space-y-5">
          <h3 class="text-xs font-medium tracking-widest uppercase text-warning/80 mb-5">
            Pending Requests ({{ pendingPeers.length }})
          </h3>

          <table class="w-full text-sm">
            <thead>
              <tr class="text-left text-[11px] text-muted/40 uppercase tracking-wider">
                <th class="pb-3 font-medium">Name</th>
                <th class="pb-3 font-medium">Address</th>
                <th class="pb-3 font-medium text-right">Actions</th>
              </tr>
            </thead>
            <tbody>
              <tr
                v-for="peer in pendingPeers"
                :key="peer.onion_address"
                class="border-t border-border/20"
              >
                <td class="py-2.5 text-foreground">{{ peer.name || 'Unknown' }}</td>
                <td class="py-2.5 font-mono text-xs text-muted/60" :title="stripOnion(peer.onion_address)">{{ truncateAddress(peer.onion_address) }}</td>
                <td class="py-2.5 text-right">
                  <div class="flex justify-end gap-2">
                    <button
                      class="px-3 py-1 text-xs rounded border border-accent/60 text-accent hover:bg-accent/10 transition-colors"
                      @click="approvePeer(peer.onion_address)"
                    >
                      Approve
                    </button>
                    <button
                      class="px-3 py-1 text-xs rounded border border-border/40 text-muted/60 hover:border-red-400/40 hover:text-red-400/80 transition-colors"
                      @click="rejectPeer(peer.onion_address)"
                    >
                      Reject
                    </button>
                  </div>
                </td>
              </tr>
            </tbody>
          </table>
        </section>

        <!-- Peers -->
        <section class="space-y-5">
          <div class="flex items-end justify-between">
            <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60">
              Peers ({{ approvedPeers.length }})
            </h3>
            <button
              v-if="isAdmin"
              class="px-3 py-1 text-xs rounded border border-accent/60 text-accent hover:bg-accent/10 transition-colors"
              @click="showAddPeer = !showAddPeer"
            >
              + Add Peer
            </button>
          </div>

          <!-- Add peer form -->
          <div v-if="showAddPeer" class="grid grid-cols-2 gap-x-8 gap-y-5">
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Name</label>
              <input
                v-model="newPeerName"
                placeholder="Optional"
                class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
              />
            </div>
            <div>
              <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Address</label>
              <input
                v-model="newPeerAddress"
                placeholder="Enter address"
                class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground font-mono placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
                @keyup.enter="handleAddPeer"
              />
            </div>
            <div class="col-span-2 flex gap-3">
              <button
                class="px-4 py-1.5 text-xs rounded border border-accent/60 text-accent hover:bg-accent/10 transition-colors"
                @click="handleAddPeer"
              >
                Add
              </button>
              <button
                class="px-4 py-1.5 text-xs rounded border border-border/40 text-muted/60 hover:border-border/60 hover:text-muted/80 transition-colors"
                @click="showAddPeer = false"
              >
                Cancel
              </button>
            </div>
          </div>

          <!-- Peer table -->
          <template v-if="approvedPeers.length > 0">
            <table class="w-full text-sm">
              <thead>
                <tr class="text-left text-[11px] text-muted/40 uppercase tracking-wider">
                  <th class="pb-3 font-medium w-5"></th>
                  <th class="pb-3 font-medium">Name</th>
                  <th class="pb-3 font-medium">Address</th>
                  <th class="pb-3 font-medium">Last Sync</th>
                  <th v-if="isAdmin" class="pb-3 font-medium text-right"></th>
                </tr>
              </thead>
              <tbody>
                <tr
                  v-for="peer in approvedPeers"
                  :key="peer.onion_address"
                  class="border-t border-border/20 hover:bg-white/[0.02] transition-colors"
                >
                  <td class="py-2.5">
                    <span
                      class="inline-block w-1.5 h-1.5 rounded-full"
                      :class="peer.is_online ? 'bg-green-400' : 'bg-muted/20'"
                    />
                  </td>
                  <td class="py-2.5">
                    <span class="text-foreground">{{ peer.name || 'Unnamed' }}</span>
                    <span v-if="peer.role === 'admin'" class="text-[10px] text-muted/30 uppercase ml-2">admin</span>
                  </td>
                  <td class="py-2.5 font-mono text-xs text-muted/60" :title="stripOnion(peer.onion_address)">{{ truncateAddress(peer.onion_address) }}</td>
                  <td class="py-2.5 text-xs text-muted/40">
                    {{ peer.sync_state?.last_synced_at ? formatTime(peer.sync_state.last_synced_at) : 'Never' }}
                  </td>
                  <td v-if="isAdmin" class="py-2.5 text-right">
                    <div class="flex justify-end gap-4">
                      <button
                        class="text-[11px] text-muted/30 hover:text-muted/50 transition-colors"
                        @click="setPeerRole(peer.onion_address, peer.role === 'admin' ? 'member' : 'admin')"
                      >
                        {{ peer.role === 'admin' ? 'Demote' : 'Make Admin' }}
                      </button>
                      <button
                        class="text-[11px] text-muted/30 hover:text-red-400/80 transition-colors"
                        @click="removePeer(peer.onion_address)"
                      >
                        Remove
                      </button>
                    </div>
                  </td>
                </tr>
              </tbody>
            </table>
          </template>

          <p v-else class="text-xs text-muted/30">
            No peers yet. Share your address and secret with others to get started.
          </p>
        </section>

        <!-- Actions -->
        <div class="border-t border-border/20 pt-5 flex gap-3">
          <button
            class="px-4 py-1.5 text-xs rounded border border-accent/60 text-accent hover:bg-accent/10 transition-colors disabled:opacity-30 disabled:hover:bg-transparent"
            :disabled="syncing"
            @click="triggerSync"
          >
            {{ syncing ? 'Syncing...' : 'Sync Now' }}
          </button>
          <button
            class="px-4 py-1.5 text-xs rounded border border-border/40 text-muted/60 hover:border-red-400/40 hover:text-red-400/80 transition-colors"
            @click="handleLeave"
          >
            Leave Company
          </button>
        </div>
      </div>
    </template>
    </div>
  </div>
</template>
