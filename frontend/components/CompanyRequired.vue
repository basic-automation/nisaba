<script setup lang="ts">
const { activeCompanyId, companiesInitialized } = useCompanyContext()
</script>

<template>
  <div v-if="!companiesInitialized" class="flex flex-col items-center justify-center py-24 gap-4">
    <p class="text-sm text-muted/40">Loading...</p>
  </div>
  <div v-else-if="!activeCompanyId" class="flex flex-col items-center justify-center py-24 gap-4">
    <p class="text-sm text-muted/40">No company selected</p>
    <NuxtLink
      to="/companies"
      class="text-xs text-accent hover:text-accent/80 transition-colors"
    >
      Create or join a company
    </NuxtLink>
  </div>
  <slot v-else />
</template>
