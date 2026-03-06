<script setup lang="ts">
const props = defineProps<{
  quantity: number
  threshold?: number | null
}>()

const colorClass = computed(() => {
  if (props.quantity <= 0) return 'bg-red-500/20 text-red-400'
  if (props.threshold && props.quantity <= props.threshold) return 'bg-yellow-500/20 text-yellow-400'
  return 'bg-green-500/20 text-green-400'
})
</script>

<template>
  <span
    class="inline-flex items-center px-2 py-0.5 rounded text-xs font-mono font-medium"
    :class="colorClass"
  >
    {{ quantity }}
  </span>
</template>
