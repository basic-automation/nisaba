<script setup lang="ts">
import { SelectIcon, SelectTrigger as RadixSelectTrigger, type SelectTriggerProps } from 'radix-vue'

const props = defineProps<SelectTriggerProps & { class?: string; clearable?: boolean }>()
const emit = defineEmits<{ clear: [] }>()
</script>

<template>
  <RadixSelectTrigger v-bind="{ ...props, clearable: undefined }" :class="['select-trigger', props.class]">
    <slot />
    <span
      v-if="clearable"
      class="select-trigger__clear"
      @mousedown.prevent.stop="emit('clear')"
    >
      <svg class="w-3 h-3" viewBox="0 0 15 15" fill="none">
        <path d="M11.7816 4.03157C12.0062 3.80702 12.0062 3.44295 11.7816 3.2184C11.5571 2.99385 11.193 2.99385 10.9685 3.2184L7.50005 6.68682L4.03164 3.2184C3.80708 2.99385 3.44301 2.99385 3.21846 3.2184C2.99391 3.44295 2.99391 3.80702 3.21846 4.03157L6.68688 7.49999L3.21846 10.9684C2.99391 11.193 2.99391 11.557 3.21846 11.7816C3.44301 12.0061 3.80708 12.0061 4.03164 11.7816L7.50005 8.31316L10.9685 11.7816C11.193 12.0061 11.5571 12.0061 11.7816 11.7816C12.0062 11.557 12.0062 11.193 11.7816 10.9684L8.31322 7.49999L11.7816 4.03157Z" fill="currentColor" fill-rule="evenodd" clip-rule="evenodd" />
      </svg>
    </span>
    <SelectIcon as-child>
      <svg class="select-trigger__icon" viewBox="0 0 15 15" fill="none" xmlns="http://www.w3.org/2000/svg">
        <path d="M4.93179 5.43179C4.75605 5.60753 4.75605 5.89245 4.93179 6.06819C5.10753 6.24392 5.39245 6.24392 5.56819 6.06819L7.49999 4.13638L9.43179 6.06819C9.60753 6.24392 9.89245 6.24392 10.0682 6.06819C10.2439 5.89245 10.2439 5.60753 10.0682 5.43179L7.81819 3.18179C7.73379 3.0974 7.61933 3.04999 7.49999 3.04999C7.38064 3.04999 7.26618 3.0974 7.18179 3.18179L4.93179 5.43179ZM10.0682 9.56819C10.2439 9.39245 10.2439 9.10753 10.0682 8.93179C9.89245 8.75606 9.60753 8.75606 9.43179 8.93179L7.49999 10.8636L5.56819 8.93179C5.39245 8.75606 5.10753 8.75606 4.93179 8.93179C4.75605 9.10753 4.75605 9.39245 4.93179 9.56819L7.18179 11.8182C7.35753 11.9939 7.64245 11.9939 7.81819 11.8182L10.0682 9.56819Z" fill="currentColor" fill-rule="evenodd" clip-rule="evenodd" />
      </svg>
    </SelectIcon>
  </RadixSelectTrigger>
</template>

<style scoped>
.select-trigger {
  display: inline-flex;
  align-items: center;
  justify-content: space-between;
  width: 100%;
  padding: 8px 10px;
  font-size: 13px;
  color: #e2e8f0;
  background: transparent;
  border: 1px solid rgba(71, 85, 105, 0.35);
  border-radius: 0.5rem;
  outline: none;
  cursor: pointer;
  transition: border-color 0.2s ease;
  gap: 8px;
}
.select-trigger:hover {
  border-color: rgba(71, 85, 105, 0.6);
}
.select-trigger:focus {
  border-color: rgba(139, 92, 246, 0.4);
}
.select-trigger__icon {
  width: 14px;
  height: 14px;
  opacity: 0.5;
  flex-shrink: 0;
}
.select-trigger__clear {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: rgba(148, 163, 184, 0.4);
  flex-shrink: 0;
  transition: color 0.15s ease;
}
.select-trigger__clear:hover {
  color: rgba(248, 113, 113, 0.8);
}
</style>
