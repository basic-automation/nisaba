<script setup lang="ts">
const {
  updateAvailable,
  updateVersion,
  downloading,
  downloadProgress,
  error,
  downloadAndInstall,
  dismiss,
  startAutoCheck,
  stopAutoCheck,
} = useUpdater()

onMounted(() => {
  startAutoCheck()
})

onUnmounted(() => {
  stopAutoCheck()
})
</script>

<template>
  <Transition name="banner">
    <div
      v-if="updateAvailable"
      class="update-banner flex items-center gap-4 px-4 py-2 text-sm shrink-0"
    >
      <div class="flex-1 flex items-center gap-3">
        <!-- Icon -->
        <svg class="w-4 h-4 text-accent shrink-0" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.5">
          <path d="M10 3v10M6 9l4 4 4-4" />
          <path d="M3 15v2h14v-2" />
        </svg>

        <span v-if="!downloading" class="text-foreground/80">
          Update <span class="text-accent font-medium">v{{ updateVersion }}</span> available
        </span>
        <span v-else class="text-foreground/80">
          Downloading... {{ downloadProgress }}%
        </span>
      </div>

      <!-- Progress bar -->
      <div v-if="downloading" class="w-32 h-1.5 rounded-full bg-surface/30 overflow-hidden">
        <div
          class="h-full bg-accent rounded-full transition-all duration-300"
          :style="{ width: `${downloadProgress}%` }"
        />
      </div>

      <!-- Error -->
      <span v-if="error" class="text-red-400/80 text-xs">{{ error }}</span>

      <!-- Actions -->
      <button
        v-if="!downloading"
        class="text-xs font-medium text-accent hover:text-accent/80 transition-colors"
        @click="downloadAndInstall"
      >
        Install
      </button>
      <button
        v-if="!downloading"
        class="text-xs text-muted/30 hover:text-muted/50 transition-colors"
        @click="dismiss"
      >
        Later
      </button>
    </div>
  </Transition>
</template>

<style scoped>
.update-banner {
  background: rgba(139, 92, 246, 0.06);
  border-bottom: 1px solid rgba(139, 92, 246, 0.12);
}

.banner-enter-active,
.banner-leave-active {
  transition: all 0.3s ease;
}

.banner-enter-from,
.banner-leave-to {
  opacity: 0;
  max-height: 0;
  padding-top: 0;
  padding-bottom: 0;
}
</style>
