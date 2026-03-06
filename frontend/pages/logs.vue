<script setup lang="ts">
const { filteredLines, levelFilter, searchQuery, autoScroll, startListening, clear } = useLogs()

startListening()

const logContainer = ref<HTMLElement | null>(null)

const levels = ['ERROR', 'WARN', 'INFO', 'DEBUG', 'TRACE']

// Cap visible lines to avoid rendering thousands of DOM nodes
const MAX_VISIBLE = 500
const showAll = ref(false)

const visibleLines = computed(() => {
  const all = filteredLines.value
  if (showAll.value || all.length <= MAX_VISIBLE) return all
  return all.slice(-MAX_VISIBLE)
})

const hiddenCount = computed(() => {
  if (showAll.value) return 0
  return Math.max(0, filteredLines.value.length - MAX_VISIBLE)
})

function levelColor(level: string) {
  switch (level) {
    case 'ERROR': return 'text-red-400/90'
    case 'WARN': return 'text-warning/80'
    case 'INFO': return 'text-info/80'
    case 'DEBUG': return 'text-muted/50'
    case 'TRACE': return 'text-muted/30'
    default: return 'text-muted/40'
  }
}

// Auto-scroll to bottom when new lines arrive
watch(visibleLines, () => {
  if (autoScroll.value && logContainer.value) {
    nextTick(() => {
      logContainer.value!.scrollTop = logContainer.value!.scrollHeight
    })
  }
}, { deep: false })
</script>

<template>
  <div class="flex flex-col h-full">
    <!-- Header -->
    <div class="flex items-end justify-between mb-6 shrink-0">
      <h2 class="text-sm font-medium tracking-widest uppercase text-muted/60">Logs</h2>
      <div class="flex items-center gap-6">
        <!-- Level filter -->
        <nav class="flex items-center gap-3">
          <button
            class="text-xs pb-0.5 transition-colors"
            :class="!levelFilter ? 'text-foreground border-b border-accent' : 'text-muted/30 hover:text-muted'"
            @click="levelFilter = ''"
          >
            All
          </button>
          <button
            v-for="l in levels"
            :key="l"
            class="text-xs pb-0.5 transition-colors"
            :class="levelFilter === l ? 'text-foreground border-b border-accent' : 'text-muted/30 hover:text-muted'"
            @click="levelFilter = l"
          >
            {{ l }}
          </button>
        </nav>

        <input
          v-model="searchQuery"
          type="text"
          placeholder="Filter..."
          class="w-48 bg-transparent border-b border-border/30 px-0 py-0.5 text-xs text-foreground placeholder-muted/20 focus:outline-none focus:border-accent/50 transition-colors"
        >

        <button
          class="text-xs transition-colors"
          :class="autoScroll ? 'text-accent' : 'text-muted/30 hover:text-muted'"
          @click="autoScroll = !autoScroll"
        >
          Auto-scroll
        </button>

        <button
          class="text-xs text-muted/30 hover:text-muted transition-colors"
          @click="clear"
        >
          Clear
        </button>
      </div>
    </div>

    <!-- Log output -->
    <div
      ref="logContainer"
      class="flex-1 overflow-auto min-h-0 -mr-6 pr-6 font-mono text-xs leading-relaxed"
    >
      <!-- Show older lines prompt -->
      <button
        v-if="hiddenCount > 0"
        class="w-full py-2 text-center text-[11px] text-muted/30 hover:text-muted/50 transition-colors"
        @click="showAll = true"
      >
        {{ hiddenCount }} older lines hidden — click to show all
      </button>

      <div
        v-for="(line, i) in visibleLines"
        :key="i"
        class="log-line flex gap-3 py-px hover:bg-white/[0.02]"
      >
        <span class="text-muted/20 select-none shrink-0 w-20 text-right tabular-nums">{{ line.timestamp }}</span>
        <span class="shrink-0 w-12 text-right" :class="levelColor(line.level)">{{ line.level }}</span>
        <span class="text-muted/25 shrink-0 truncate max-w-[180px]" :title="line.target">{{ line.target }}</span>
        <span class="text-foreground/70 break-all">{{ line.message }}</span>
      </div>
      <p v-if="filteredLines.length === 0" class="text-muted/30 py-16">
        No log output yet.
      </p>
    </div>
  </div>
</template>

<style scoped>
.log-line {
  content-visibility: auto;
  contain-intrinsic-size: auto 20px;
}
</style>
