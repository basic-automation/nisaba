<script setup lang="ts">
// Minimal chrome for the dedicated settings window (label `nisaba-settings`).
// This window is the ONLY context allowed to invoke secret-returning Tauri
// commands, so it deliberately offers no navigation beyond the settings views.
const route = useRoute()

const tabs = [
  { label: 'Configuration', path: '/config' },
  { label: 'Company', path: '/company' },
]

// The platform-setup wizard is hosted here for existing companies; it gets no
// tabs because it is a focused flow (Close finishes it).
const showTabs = computed(() => route.path !== '/setup')

function isActive(path: string) {
  return route.path === path
}

async function closeWindow() {
  await closeCurrentWindow()
}
</script>

<template>
  <div class="settings-shell flex flex-col h-screen overflow-hidden">
    <header class="flex items-center gap-2 px-5 py-3 border-b border-white/10 shrink-0">
      <h1 class="text-sm font-medium tracking-widest uppercase text-muted/60">Settings</h1>
      <nav v-if="showTabs" class="flex items-center gap-1 ml-6">
        <NuxtLink
          v-for="t in tabs"
          :key="t.path"
          :to="t.path"
          class="px-3 py-1.5 rounded-md text-xs transition-colors"
          :class="
            isActive(t.path)
              ? 'bg-accent/15 text-foreground'
              : 'text-muted/50 hover:text-muted'
          "
        >
          {{ t.label }}
        </NuxtLink>
      </nav>
      <div class="flex-1" />
      <button
        class="px-3 py-1.5 rounded-md text-xs text-muted/50 hover:text-foreground hover:bg-white/5 transition-colors"
        @click="closeWindow"
      >
        Close
      </button>
    </header>
    <main class="flex-1 overflow-hidden p-6 min-h-0">
      <slot />
    </main>
  </div>
</template>

<style scoped>
.settings-shell {
  background: #1e293b;
}
</style>
