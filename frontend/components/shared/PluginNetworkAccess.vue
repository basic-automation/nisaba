<script setup lang="ts">
import type { VendorPluginInfo } from '~/types'

// What a vendor plugin can reach over the network, as computed from its own files.
// Shown before install so the user sees it before the plugin ever runs.
const props = defineProps<{
  plugin: Pick<VendorPluginInfo, 'network_access' | 'allowed_hosts'>
}>()

const summary = computed(() => {
  switch (props.plugin.network_access) {
    case 'restricted':
      return props.plugin.allowed_hosts.length > 0
        ? props.plugin.allowed_hosts.join(', ')
        : 'No network access'
    case 'unrestricted':
      return 'Unrestricted — this plugin declares no allowed hosts'
    default:
      return 'Checked when installed'
  }
})
</script>

<template>
  <div
    class="text-[11px] flex items-start gap-1.5"
    :class="plugin.network_access === 'unrestricted' ? 'text-amber-400/80' : 'text-muted/30'"
    :title="plugin.network_access === 'unrestricted'
      ? 'The plugin can send requests to any host, including devices on your local network.'
      : undefined"
  >
    <span class="shrink-0">Network:</span>
    <span class="break-all">{{ summary }}</span>
  </div>
</template>
