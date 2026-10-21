<script setup lang="ts">
import type { VendorPluginInfo } from '~/types'

// Per-plugin auto-sync schedule (N-7.5). Without an interval the plugin runs
// on every global sync cycle; with one, the auto-sync path runs it at most
// once per interval. Manual runs (Sync / Fetch) are never gated by it.
const props = defineProps<{
  plugin: Pick<VendorPluginInfo, 'id' | 'sync_interval_minutes'>
}>()

const { setPluginSyncInterval, startSync } = useVendors()
const error = ref('')
const running = ref(false)

// Value 0 in the select means "every global cycle" (stored as null).
const CHOICES: { minutes: number; label: string }[] = [
  { minutes: 0, label: 'Every cycle' },
  { minutes: 15, label: '15 min' },
  { minutes: 30, label: '30 min' },
  { minutes: 60, label: '1 h' },
  { minutes: 360, label: '6 h' },
  { minutes: 720, label: '12 h' },
  { minutes: 1440, label: '24 h' },
  { minutes: 10080, label: '7 days' },
]

const options = computed(() => {
  const current = props.plugin.sync_interval_minutes ?? 0
  const values = new Map(CHOICES.map(c => [c.minutes, c]))
  if (!values.has(current)) {
    values.set(current, { minutes: current, label: `${current} min` })
  }
  return [...values.values()].sort((a, b) => a.minutes - b.minutes)
})

async function onChange(event: Event) {
  const minutes = Number((event.target as HTMLSelectElement).value)
  error.value = ''
  try {
    await setPluginSyncInterval(props.plugin.id, minutes === 0 ? null : minutes)
  } catch (e: any) {
    error.value = e?.toString() || 'Could not save the sync interval'
  }
}

async function onRunNow() {
  error.value = ''
  running.value = true
  try {
    await startSync(props.plugin.id)
  } catch (e: any) {
    error.value = e?.toString() || 'Could not start a run'
  } finally {
    running.value = false
  }
}
</script>

<template>
  <div class="text-[11px] text-muted/30 flex items-center gap-1.5">
    <label :for="`interval-${plugin.id}`" class="shrink-0">Sync every:</label>
    <select
      :id="`interval-${plugin.id}`"
      :value="plugin.sync_interval_minutes ?? 0"
      class="bg-transparent text-muted/60 border border-white/10 rounded px-1 py-0.5 focus:outline-none focus:ring-1 focus:ring-accent/50"
      @change="onChange"
    >
      <option v-for="o in options" :key="o.minutes" :value="o.minutes" class="bg-surface text-foreground">
        {{ o.label }}
      </option>
    </select>
    <button
      class="text-muted/50 hover:text-accent transition-colors disabled:opacity-50"
      :disabled="running"
      @click="onRunNow"
    >
      {{ running ? 'Starting…' : 'Run now' }}
    </button>
    <span v-if="error" class="text-red-400/80">{{ error }}</span>
  </div>
</template>
