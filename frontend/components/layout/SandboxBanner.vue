<script setup lang="ts">
import { invoke } from '@tauri-apps/api/core'

// Dev builds talk to the eBay and Amazon sandboxes and keep their own data folder
// (nisaba_core::sandbox). Say so on every page so test data is never mistaken for real.
const sandbox = ref(false)

onMounted(async () => {
  try {
    sandbox.value = await invoke<boolean>('get_sandbox_mode')
  } catch {
    sandbox.value = false
  }
})
</script>

<template>
  <div
    v-if="sandbox"
    class="sandbox-banner flex items-center gap-3 px-4 py-1.5 text-xs shrink-0 text-amber-300"
    role="status"
  >
    <span class="font-medium uppercase tracking-wider">Sandbox</span>
    <span class="text-foreground/70">
      eBay and Amazon test environments, with a separate data folder. Nothing here touches real listings.
    </span>
  </div>
</template>

<style scoped>
.sandbox-banner {
  background: rgba(245, 158, 11, 0.08);
  border-bottom: 1px solid rgba(245, 158, 11, 0.2);
}
</style>
