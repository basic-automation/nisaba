<script setup lang="ts">
const { activeCompany, companies, switchCompany } = useCompanyContext()

const longestName = computed(() => {
  let longest = 'No Company'
  for (const c of companies.value) {
    if (c.name.length > longest.length) longest = c.name
  }
  return longest
})

function handleSelect(value: string) {
  if (value === '__manage__') {
    navigateTo('/companies')
    return
  }
  switchCompany(value)
}
</script>

<template>
  <Select :model-value="activeCompany?.id ?? ''" @update:model-value="handleSelect">
    <SelectTrigger class="company-trigger" data-tauri-drag-region="false">
      <img
        v-if="activeCompany?.logo"
        :src="activeCompany.logo"
        class="w-4 h-4 rounded-full object-cover shrink-0"
      />
      <svg v-else class="w-3.5 h-3.5 text-muted/50 shrink-0" viewBox="0 0 16 16" fill="currentColor">
        <path d="M2 3a1 1 0 0 1 1-1h4.586a1 1 0 0 1 .707.293l1.414 1.414a1 1 0 0 0 .707.293H13a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V3z" />
      </svg>
      <span class="company-name-sizer text-xs">
        <span class="text-foreground/80">{{ activeCompany?.name || 'No Company' }}</span>
        <span class="invisible" aria-hidden="true">{{ longestName }}</span>
      </span>
    </SelectTrigger>

    <SelectContent>
      <div v-if="companies.length === 0" class="px-3 py-2 text-xs text-muted/40">
        No companies
      </div>

      <SelectItem v-for="company in companies" :key="company.id" :value="company.id">
        <span class="flex items-center gap-2 w-full">
          <img
            v-if="company.logo"
            :src="company.logo"
            class="w-4 h-4 rounded-full object-cover shrink-0"
          />
          <svg v-else class="w-3.5 h-3.5 text-muted/40 shrink-0" viewBox="0 0 16 16" fill="currentColor">
            <path d="M2 3a1 1 0 0 1 1-1h4.586a1 1 0 0 1 .707.293l1.414 1.414a1 1 0 0 0 .707.293H13a1 1 0 0 1 1 1v8a1 1 0 0 1-1 1H3a1 1 0 0 1-1-1V3z" />
          </svg>
          <span class="truncate flex-1 text-left">{{ company.name }}</span>
          <span
            class="text-[10px] px-1.5 py-0.5 rounded"
            :class="company.role === 'admin' ? 'bg-amber-500/20 text-amber-400' : 'bg-slate-500/20 text-slate-400'"
          >
            {{ company.role }}
          </span>
        </span>
      </SelectItem>

      <div v-if="companies.length > 0" class="border-t border-white/[0.06] my-1" />

      <SelectItem value="__manage__">
        <span class="flex items-center gap-2 text-muted/60">
          <svg class="w-3.5 h-3.5 shrink-0" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.5">
            <line x1="6" y1="2" x2="6" y2="10" />
            <line x1="2" y1="6" x2="10" y2="6" />
          </svg>
          Manage Companies
        </span>
      </SelectItem>
    </SelectContent>
  </Select>
</template>

<style scoped>
.company-trigger {
  display: inline-flex;
  align-items: center;
  gap: 6px;
  width: fit-content !important;
  padding: 2px 8px;
  font-size: 12px;
  color: rgba(176, 190, 201, 0.8);
  background: transparent;
  border: none;
  border-radius: 0.375rem;
  cursor: pointer;
  outline: none;
  transition: color 0.2s ease;
}
.company-trigger:hover {
  color: rgba(176, 190, 201, 1);
}

/* Grid overlay: both children share the same cell so the wider one sets the min-width */
.company-name-sizer {
  display: grid;
}
.company-name-sizer > * {
  grid-area: 1 / 1;
  white-space: nowrap;
}
</style>
