<script setup lang="ts">
import type { VendorPluginInfo } from '~/types'

// How long one run of an installed plugin may take on this machine before it is stopped.
// A large catalog behind a rate-limited API can need longer than the default.
const props = defineProps<{
  plugin: Pick<VendorPluginInfo, 'id' | 'timeout_minutes' | 'timeout_is_default'>
}>()

const { setPluginTimeout } = useVendors()
const error = ref('')

const CHOICES = [15, 30, 60, 120, 240, 480, 720]

function label(minutes: number) {
  return minutes < 60 ? `${minutes} min` : `${minutes / 60} h`
}

const options = computed(() => {
  const values = new Set([...CHOICES, props.plugin.timeout_minutes])
  return [...values].sort((a, b) => a - b)
})

async function onChange(event: Event) {
  const minutes = Number((event.target as HTMLSelectElement).value)
  error.value = ''
  try {
    await setPluginTimeout(props.plugin.id, minutes)
  } catch (e: any) {
    error.value = e?.toString() || 'Could not save the time limit'
  }
}
</script>

<template>
  <div class="text-[11px] text-muted/30 flex items-center gap-1.5">
    <label :for="`timeout-${plugin.id}`" class="shrink-0">Time limit:</label>
    <select
      :id="`timeout-${plugin.id}`"
      :value="plugin.timeout_minutes"
      class="bg-transparent text-muted/60 border border-white/10 rounded px-1 py-0.5 focus:outline-none focus:ring-1 focus:ring-accent/50"
      @change="onChange"
    >
      <option v-for="m in options" :key="m" :value="m" class="bg-surface text-foreground">
        {{ label(m) }}{{ m === plugin.timeout_minutes && plugin.timeout_is_default ? ' (default)' : '' }}
      </option>
    </select>
    <span v-if="error" class="text-red-400/80">{{ error }}</span>
  </div>
</template>
