<script setup lang="ts">
import { SelectValue as RadixSelectValue, type SelectValueProps } from 'radix-vue'

const props = defineProps<SelectValueProps & { class?: string }>()
</script>

<template>
  <RadixSelectValue v-bind="props" :class="props.class">
    <slot />
  </RadixSelectValue>
</template>
