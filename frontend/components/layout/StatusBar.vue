<script setup lang="ts">
const { isRunning, fetchStatus, triggerSync, pauseSync, resumeSync, startListening } = useSync()
const { notify } = useNotifications()

// N-11.3: "Sync now" gave no progress or error feedback — track the in-flight
// trigger and surface failures as notifications.
const syncingNow = ref(false)

startListening()

onMounted(async () => {
  try {
    await fetchStatus()
  } catch {
    // Company context may not be ready yet; will refresh when companies-ready fires
  }
})

async function toggleSync() {
  try {
    if (isRunning.value === null) {
      await fetchStatus()
    } else if (isRunning.value) {
      await pauseSync()
    } else {
      await resumeSync()
    }
  } catch (e: any) {
    notify({ type: 'error', title: 'Sync toggle failed', message: String(e), source: 'statusbar' })
  }
}

async function handleSyncNow() {
  if (syncingNow.value) return
  syncingNow.value = true
  try {
    await triggerSync()
    notify({ type: 'success', title: 'Sync triggered', message: 'A sync cycle has started.', source: 'statusbar' })
  } catch (e: any) {
    notify({ type: 'error', title: 'Sync failed to start', message: String(e), source: 'statusbar' })
  } finally {
    syncingNow.value = false
  }
}
</script>

<template>
  <div class="flex flex-col items-center gap-2">
    <!-- Sync status dot -->
    <button
      class="group relative flex items-center justify-center w-8 h-8 rounded transition-colors hover:bg-surface/30"
      @click="toggleSync"
    >
      <span
        class="block w-2.5 h-2.5 rounded-full transition-colors"
        :class="isRunning === null ? 'bg-muted/30' : isRunning ? 'bg-green-400' : 'bg-yellow-400'"
      />
      <span class="tooltip">{{ isRunning === null ? 'Loading…' : isRunning ? 'Pause sync' : 'Resume sync' }}</span>
    </button>

    <!-- Sync now -->
    <button
      class="group relative flex items-center justify-center w-8 h-8 rounded text-muted/40 hover:text-muted transition-colors hover:bg-surface/30 disabled:opacity-50"
      :disabled="syncingNow"
      @click="handleSyncNow"
    >
      <svg
        class="w-5 h-5"
        :class="{ 'animate-spin': syncingNow }"
        viewBox="0 0 20 20"
        fill="none"
        stroke="currentColor"
        stroke-width="1.5"
      >
        <path d="M17 10a7 7 0 0 1-12.9 3.7" />
        <path d="M3 10a7 7 0 0 1 12.9-3.7" />
        <path d="M17 4v4h-4" />
        <path d="M3 16v-4h4" />
      </svg>
      <span class="tooltip">{{ syncingNow ? 'Syncing…' : 'Sync now' }}</span>
    </button>
  </div>
</template>

<style scoped>
.tooltip {
  position: absolute;
  left: 100%;
  top: 50%;
  transform: translateY(-50%);
  margin-left: 8px;
  padding: 4px 10px;
  background: #1a1a2e;
  color: #e2e8f0;
  font-size: 12px;
  white-space: nowrap;
  border-radius: 4px;
  pointer-events: none;
  opacity: 0;
  transition: opacity 0.15s ease;
  z-index: 50;
}

.group:hover .tooltip {
  opacity: 1;
}
</style>
