<script setup lang="ts">
const { isRunning, fetchStatus, triggerSync, pauseSync, resumeSync, startListening } = useSync()

startListening()

onMounted(async () => {
  try {
    await fetchStatus()
  } catch {
    // Company context may not be ready yet; will refresh when companies-ready fires
  }
})

async function toggleSync() {
  if (isRunning.value) {
    await pauseSync()
  } else {
    await resumeSync()
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
        :class="isRunning ? 'bg-green-400' : 'bg-yellow-400'"
      />
      <span class="tooltip">{{ isRunning ? 'Pause sync' : 'Resume sync' }}</span>
    </button>

    <!-- Sync now -->
    <button
      class="group relative flex items-center justify-center w-8 h-8 rounded text-muted/40 hover:text-muted transition-colors hover:bg-surface/30"
      @click="triggerSync"
    >
      <svg class="w-5 h-5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5">
        <path d="M17 10a7 7 0 0 1-12.9 3.7" />
        <path d="M3 10a7 7 0 0 1 12.9-3.7" />
        <path d="M17 4v4h-4" />
        <path d="M3 16v-4h4" />
      </svg>
      <span class="tooltip">Sync now</span>
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
