<script setup lang="ts">
import { SelectItem as RadixSelectItem, type SelectItemProps, SelectItemIndicator, SelectItemText } from 'radix-vue'

const props = defineProps<SelectItemProps & { class?: string }>()
const slots = useSlots()

const filterText = inject<Ref<string>>('nisaba-select-filter', ref(''))

const isVisible = computed(() => {
  const query = filterText.value.toLowerCase().trim()
  if (!query) return true
  // Check textContent prop or the value as fallback
  const text = props.textValue?.toLowerCase() || props.value?.toString().toLowerCase() || ''
  return text.includes(query)
})
</script>

<template>
  <RadixSelectItem v-show="isVisible" v-bind="props" :class="['nisaba-select-item', props.class]">
    <span class="nisaba-select-item__indicator">
      <SelectItemIndicator>
        <svg class="w-3 h-3 text-emerald-400" viewBox="0 0 15 15" fill="none">
          <path d="M11.4669 3.72684C11.7558 3.91574 11.8369 4.30308 11.648 4.59198L7.39799 11.092C7.29783 11.2452 7.13556 11.3467 6.95402 11.3699C6.77247 11.3931 6.58989 11.3354 6.45446 11.2124L3.70446 8.71241C3.44905 8.48022 3.43023 8.08494 3.66242 7.82953C3.89461 7.57412 4.28989 7.55529 4.5453 7.78749L6.75292 9.79441L10.6018 3.90792C10.7907 3.61902 11.178 3.53795 11.4669 3.72684Z" fill="currentColor" fill-rule="evenodd" clip-rule="evenodd" />
        </svg>
      </SelectItemIndicator>
    </span>
    <SelectItemText class="truncate">
      <slot />
    </SelectItemText>
  </RadixSelectItem>
</template>

<style>
/* Unscoped — rendered inside portal, scoped styles don't apply */
.nisaba-select-item {
  position: relative;
  display: flex;
  align-items: center;
  gap: 8px;
  width: 100%;
  padding: 6px 10px;
  font-size: 12px;
  color: rgba(196, 205, 214, 0.7);
  border-radius: 0.375rem;
  cursor: pointer;
  outline: none;
  transition: background 0.1s ease, color 0.1s ease;
  user-select: none;
}
.nisaba-select-item:hover,
.nisaba-select-item[data-highlighted] {
  background: rgba(139, 92, 246, 0.12);
  color: #e2e8f0;
}
.nisaba-select-item[data-state='checked'] {
  color: #e2e8f0;
}

.nisaba-select-item__indicator {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 12px;
  height: 12px;
  flex-shrink: 0;
}
</style>
