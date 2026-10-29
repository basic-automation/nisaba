<script setup lang="ts">
// Settings live in the main window. No command returns a secret to any webview
// (stored secrets come back masked, and the company secret is only shown in a
// native dialog), so they need no window of their own (N-2.7).
type SettingsTab = 'general' | 'platforms' | 'plugins' | 'company' | 'about'

const route = useRoute()
const { activeCompany } = useCompanyContext()

const isAdmin = computed(() => activeCompany.value?.role === 'admin')

const tabs = computed(() => [
  { label: 'General', value: 'general' },
  ...(isAdmin.value ? [{ label: 'Platforms', value: 'platforms' }] : []),
  { label: 'Plugins', value: 'plugins' },
  { label: 'Company', value: 'company' },
  { label: 'About', value: 'about' },
])

// The tab rides in the query (?tab=platforms) so other views can deep-link.
const activeTab = computed<SettingsTab>({
  get() {
    const wanted = route.query.tab as string | undefined
    return (tabs.value.some(t => t.value === wanted) ? wanted : 'general') as SettingsTab
  },
  set(tab) {
    navigateTo({ query: { ...route.query, tab } }, { replace: true })
  },
})

// General, Platforms and Plugins share one panel so unsaved edits survive
// switching between tabs.
const configTab = computed(() => ({
  general: 'local',
  platforms: 'company',
  plugins: 'plugins',
}[activeTab.value as 'general' | 'platforms' | 'plugins'] ?? 'local') as 'local' | 'company' | 'plugins')
const showConfig = computed(() => ['general', 'platforms', 'plugins'].includes(activeTab.value))
</script>

<template>
  <div class="flex flex-col h-full">
    <div class="shrink-0 mb-6 max-w-2xl">
      <h2 class="text-sm font-medium tracking-widest uppercase text-muted/60 mb-5">Settings</h2>
      <TabBar v-model="activeTab" :items="tabs" />
    </div>

    <div class="flex-1 min-h-0">
      <ConfigSettings v-show="showConfig" :tab="configTab" />
      <CompanySettings v-if="activeTab === 'company'" />
      <AboutSettings v-else-if="activeTab === 'about'" />
    </div>
  </div>
</template>
