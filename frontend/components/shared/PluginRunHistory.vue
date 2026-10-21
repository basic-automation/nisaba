<script setup lang="ts">
import type { PluginRunInfo, VendorPluginInfo } from '~/types'

// Run history for one plugin (N-7.5): each recorded run's start/end,
// listings emitted, error, and the plugin's own log lines. Loaded lazily
// when the user expands the section on the plugin's card.
const props = defineProps<{
  plugin: Pick<VendorPluginInfo, 'id'>
}>()

const { fetchPluginRunHistory } = useVendors()
const open = ref(false)
const loading = ref(false)
const error = ref('')
const runs = ref<PluginRunInfo[]>([])
const expandedRun = ref<number | null>(null)

async function toggle() {
  open.value = !open.value
  if (!open.value || runs.value.length > 0) return
  loading.value = true
  error.value = ''
  try {
    runs.value = await fetchPluginRunHistory(props.plugin.id, 20)
  } catch (e: any) {
    error.value = e?.toString() || 'Could not load run history'
  } finally {
    loading.value = false
  }
}

function toggleLogs(runId: number) {
  expandedRun.value = expandedRun.value === runId ? null : runId
}

function statusClass(status: string): string {
  switch (status) {
    case 'completed': return 'text-green-300'
    case 'failed': return 'text-red-300'
    default: return 'text-amber-300'
  }
}
</script>

<template>
  <div class="text-[11px] text-muted/30">
    <button class="hover:text-muted/60 transition-colors" @click="toggle">
      {{ open ? 'Hide run history' : 'Run history' }}
    </button>
    <div v-if="open" class="mt-2">
      <p v-if="loading" class="text-muted/40">Loading…</p>
      <p v-else-if="error" class="text-red-400/80">{{ error }}</p>
      <p v-else-if="runs.length === 0" class="text-muted/40">No runs recorded yet.</p>
      <ul v-else class="space-y-1.5">
        <li v-for="run in runs" :key="run.id" class="border border-white/5 rounded px-2 py-1.5">
          <div class="flex items-center gap-2 flex-wrap">
            <span :class="statusClass(run.status)">{{ run.status }}</span>
            <span>{{ run.started_at }}</span>
            <span v-if="run.finished_at">→ {{ run.finished_at }}</span>
            <span>{{ run.listings_emitted }} listings</span>
            <span class="text-muted/20">{{ run.trigger_kind }}</span>
            <button
              v-if="run.log_lines.length > 0"
              class="ml-auto hover:text-muted/60 transition-colors"
              @click="toggleLogs(run.id)"
            >
              {{ expandedRun === run.id ? 'Hide logs' : `Logs (${run.log_lines.length})` }}
            </button>
          </div>
          <p v-if="run.error" class="text-red-400/80 mt-1 break-words">{{ run.error }}</p>
          <pre
            v-if="expandedRun === run.id"
            class="mt-1.5 text-[10px] text-muted/50 font-mono whitespace-pre-wrap max-h-40 overflow-auto"
          >{{ run.log_lines.join('\n') }}</pre>
        </li>
      </ul>
    </div>
  </div>
</template>
