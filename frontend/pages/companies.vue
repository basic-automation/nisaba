<script setup lang="ts">
const { companies, activeCompanyId, activeCompany, switchCompany, fetchCompanies } = useCompanyContext()
const { joinCompany } = useCompany()

// Forms
const showJoinForm = ref(false)
const joinAdminAddress = ref('')
const joinSecret = ref('')
const joinName = ref('')

const { notify } = useNotifications()

function ensureOnion(addr: string) {
  return addr.endsWith('.onion') ? addr : addr + '.onion'
}

async function handleJoin() {
  if (!joinAdminAddress.value.trim() || !joinSecret.value.trim()) return
  try {
    await joinCompany(
      ensureOnion(joinAdminAddress.value.trim()),
      joinSecret.value.trim(),
      joinName.value.trim() || 'Member',
    )
    showJoinForm.value = false
    joinAdminAddress.value = ''
    joinSecret.value = ''
    joinName.value = ''
    notify({ type: 'success', title: 'Company joined', message: 'Successfully joined the company', source: 'companies' })
  } catch (e: any) {
    notify({ type: 'error', title: 'Join failed', message: e?.toString() || 'Failed to join company', source: 'companies' })
  }
}

async function handleSwitch(id: string) {
  if (id === activeCompanyId.value) return
  await switchCompany(id)
  notify({ type: 'info', title: 'Company switched', message: 'Active company changed', source: 'companies' })
}

onMounted(() => {
  fetchCompanies()
})
</script>

<template>
  <div class="flex flex-col h-full">
    <div class="flex items-end justify-between mb-10 shrink-0 max-w-2xl">
      <h2 class="text-sm font-medium tracking-widest uppercase text-muted/60">Companies</h2>
    </div>

    <div class="flex-1 overflow-auto min-h-0 -mr-6 pr-6">
    <!-- Company list -->
    <section v-if="companies.length > 0" class="mb-14 max-w-2xl">
      <div class="space-y-1">
        <button
          v-for="company in companies"
          :key="company.id"
          class="w-full flex items-center gap-4 px-4 py-3 rounded-lg transition-colors text-left"
          :class="company.id === activeCompanyId
            ? 'bg-accent/[0.07] border border-accent/20'
            : 'border border-transparent hover:bg-white/[0.03]'"
          @click="handleSwitch(company.id)"
        >
          <!-- Logo -->
          <div class="shrink-0 w-8 h-8 rounded-full overflow-hidden bg-white/[0.03] flex items-center justify-center border border-border/20">
            <img
              v-if="company.logo"
              :src="company.logo"
              class="w-full h-full object-cover"
            />
            <svg v-else class="w-3.5 h-3.5 text-muted/20" viewBox="0 0 16 16" fill="currentColor">
              <path d="M2 3a1 1 0 0 1 1-1h4.586a1 1 0 0 1 .707.293l1.414 1.414a1 1 0 0 0 .707.293H13a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V3z" />
            </svg>
          </div>

          <!-- Info -->
          <div class="flex-1 min-w-0">
            <div class="flex items-center gap-2">
              <span class="text-sm text-foreground truncate">{{ company.name }}</span>
              <span
                class="text-[10px] px-1.5 py-0.5 rounded shrink-0"
                :class="company.role === 'admin' ? 'bg-amber-500/20 text-amber-400' : 'bg-white/[0.06] text-muted/50'"
              >
                {{ company.role }}
              </span>
            </div>
          </div>

          <!-- Active indicator -->
          <div class="shrink-0">
            <svg
              v-if="company.id === activeCompanyId"
              class="w-4 h-4 text-accent"
              viewBox="0 0 16 16"
              fill="none"
              stroke="currentColor"
              stroke-width="2"
            >
              <path d="M3 8l4 4 6-6" stroke-linecap="round" stroke-linejoin="round" />
            </svg>
          </div>
        </button>
      </div>

      <!-- Link to active company peer management -->
      <NuxtLink
        v-if="activeCompany"
        to="/company"
        class="inline-block mt-4 text-xs text-muted/40 hover:text-muted/60 transition-colors"
      >
        Manage peers &amp; settings for {{ activeCompany.name }}
      </NuxtLink>
    </section>

    <div v-else class="mb-14">
      <p class="text-sm text-muted/40">No companies yet. Create one to start syncing, or join an existing one.</p>
    </div>

    <!-- Actions -->
    <div class="flex gap-3 mb-10">
      <NuxtLink
        to="/setup"
        class="px-4 py-1.5 text-xs rounded border border-accent/60 text-accent hover:bg-accent/10 transition-colors"
      >
        Create Company
      </NuxtLink>
      <button
        class="px-4 py-1.5 text-xs rounded border border-border/40 text-muted/60 hover:border-border/60 hover:text-muted/80 transition-colors"
        :class="showJoinForm ? 'bg-white/[0.03]' : ''"
        @click="showJoinForm = !showJoinForm"
      >
        Join Company
      </button>
    </div>

    <!-- Join form -->
    <div v-if="showJoinForm" class="space-y-5">
      <h3 class="text-xs font-medium tracking-widest uppercase text-muted/60 mb-5">Join Company</h3>
      <div>
        <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Your Name</label>
        <input
          v-model="joinName"
          placeholder="Optional"
          class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
        />
      </div>
      <div>
        <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Admin Address</label>
        <input
          v-model="joinAdminAddress"
          placeholder="Enter address"
          class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground font-mono placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
        />
      </div>
      <div>
        <label class="block text-[11px] text-muted/40 uppercase tracking-wider mb-1.5">Company Secret</label>
        <input
          v-model="joinSecret"
          placeholder="Enter secret"
          type="password"
          class="w-full bg-transparent border-b border-border/30 px-0 py-1.5 text-sm text-foreground font-mono placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
          @keyup.enter="handleJoin"
        />
      </div>
      <div class="flex gap-3 pt-3">
        <button
          class="px-4 py-1.5 text-xs rounded border border-accent/60 text-accent hover:bg-accent/10 transition-colors"
          @click="handleJoin"
        >
          Join
        </button>
        <button
          class="px-4 py-1.5 text-xs rounded border border-border/40 text-muted/60 hover:border-border/60 hover:text-muted/80 transition-colors"
          @click="showJoinForm = false"
        >
          Cancel
        </button>
      </div>
    </div>
    </div>
  </div>
</template>
