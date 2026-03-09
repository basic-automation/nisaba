<script setup lang="ts">
defineProps<{
  name: string
}>()
</script>

<template>
  <span class="vendor-badge inline-flex items-center gap-1.5 px-2 py-0.5 rounded text-xs font-medium bg-emerald-500/20 text-emerald-400">
    <svg class="w-3.5 h-3.5 shrink-0" viewBox="0 0 24 24" fill="none" xmlns="http://www.w3.org/2000/svg">
      <path
        fill="currentColor"
        d="M20 4H4c-1.1 0-2 .9-2 2v2c0 .74.4 1.38 1 1.73V18c0 1.1.9 2 2 2h14c1.1 0 2-.9 2-2V9.73c.6-.35 1-.99 1-1.73V6c0-1.1-.9-2-2-2zm0 4H4V6h16v2zm-5 10H9v-4h6v4z"
      />
    </svg>
    <span>{{ name }}</span>
  </span>
</template>

<style scoped>
.vendor-badge {
  position: relative;
  isolation: isolate;
  overflow: hidden;
  border-top: 1px solid rgba(255, 255, 255, 0.06);
  border-left: 1px solid rgba(255, 255, 255, 0.04);
  border-bottom: 1px solid rgba(71, 85, 105, 0.18);
  border-right: 1px solid rgba(71, 85, 105, 0.10);
  box-shadow:
    inset 0 1px 0 0 rgba(255, 255, 255, 0.04),
    0 1px 1px -0.5px rgba(0, 0, 0, 0.12),
    0 2px 4px -2px rgba(0, 0, 0, 0.10);
}

.vendor-badge::after {
  content: '';
  position: absolute;
  inset: 0;
  z-index: -1;
  pointer-events: none;
  border-radius: inherit;
  opacity: 0.18;
  background-blend-mode: soft-light;
  background: repeating-radial-gradient(
    circle,
    #1a2035,
    #1a2035 2px,
    #253050 2px 4px,
    #1a2035 4px 6px,
    #253050 6px 8px,
    #1a2035 8px 10px,
    #253050 10px 12px
  ) 0 0 / 100% 100%;
}
</style>
