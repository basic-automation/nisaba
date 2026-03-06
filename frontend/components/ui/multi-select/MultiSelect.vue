<script setup lang="ts">
const props = withDefaults(defineProps<{
  options: { label: string; value: string }[]
  placeholder?: string
  filterable?: boolean
}>(), {
  placeholder: 'Select...',
  filterable: false,
})

const model = defineModel<string[]>({ default: () => [] })

const open = ref(false)
const filterText = ref('')
const wrapperRef = ref<HTMLElement | null>(null)

const filteredOptions = computed(() => {
  const q = filterText.value.toLowerCase().trim()
  if (!q) return props.options
  return props.options.filter(o => o.label.toLowerCase().includes(q) || o.value.toLowerCase().includes(q))
})

const displayText = computed(() => {
  if (model.value.length === 0) return ''
  if (model.value.length <= 2) {
    return model.value.map(v => props.options.find(o => o.value === v)?.label || v).join(', ')
  }
  return `${model.value.length} selected`
})

function toggle(value: string) {
  const idx = model.value.indexOf(value)
  if (idx >= 0) {
    model.value = model.value.filter(v => v !== value)
  } else {
    model.value = [...model.value, value]
  }
}

function toggleOpen() {
  open.value = !open.value
  if (open.value) {
    filterText.value = ''
  }
}

// Close on outside click
function onClickOutside(e: MouseEvent) {
  if (!wrapperRef.value?.contains(e.target as Node)) {
    open.value = false
  }
}

onMounted(() => document.addEventListener('mousedown', onClickOutside))
onBeforeUnmount(() => document.removeEventListener('mousedown', onClickOutside))
</script>

<template>
  <div ref="wrapperRef" class="multi-select">
    <button
      type="button"
      class="multi-select__trigger"
      @click="toggleOpen"
    >
      <span v-if="displayText" class="multi-select__text">{{ displayText }}</span>
      <span v-else class="multi-select__placeholder">{{ placeholder }}</span>
      <span
        v-if="model.length > 0"
        class="multi-select__clear"
        @mousedown.prevent.stop="model = []; open = false"
      >
        <svg class="w-3 h-3" viewBox="0 0 15 15" fill="none">
          <path d="M11.7816 4.03157C12.0062 3.80702 12.0062 3.44295 11.7816 3.2184C11.5571 2.99385 11.193 2.99385 10.9685 3.2184L7.50005 6.68682L4.03164 3.2184C3.80708 2.99385 3.44301 2.99385 3.21846 3.2184C2.99391 3.44295 2.99391 3.80702 3.21846 4.03157L6.68688 7.49999L3.21846 10.9684C2.99391 11.193 2.99391 11.557 3.21846 11.7816C3.44301 12.0061 3.80708 12.0061 4.03164 11.7816L7.50005 8.31316L10.9685 11.7816C11.193 12.0061 11.5571 12.0061 11.7816 11.7816C12.0062 11.557 12.0062 11.193 11.7816 10.9684L8.31322 7.49999L11.7816 4.03157Z" fill="currentColor" fill-rule="evenodd" clip-rule="evenodd" />
        </svg>
      </span>
      <svg class="multi-select__icon" viewBox="0 0 15 15" fill="none">
        <path d="M4.93179 5.43179C4.75605 5.60753 4.75605 5.89245 4.93179 6.06819C5.10753 6.24392 5.39245 6.24392 5.56819 6.06819L7.49999 4.13638L9.43179 6.06819C9.60753 6.24392 9.89245 6.24392 10.0682 6.06819C10.2439 5.89245 10.2439 5.60753 10.0682 5.43179L7.81819 3.18179C7.73379 3.0974 7.61933 3.04999 7.49999 3.04999C7.38064 3.04999 7.26618 3.0974 7.18179 3.18179L4.93179 5.43179ZM10.0682 9.56819C10.2439 9.39245 10.2439 9.10753 10.0682 8.93179C9.89245 8.75606 9.60753 8.75606 9.43179 8.93179L7.49999 10.8636L5.56819 8.93179C5.39245 8.75606 5.10753 8.75606 4.93179 8.93179C4.75605 9.10753 4.75605 9.39245 4.93179 9.56819L7.18179 11.8182C7.35753 11.9939 7.64245 11.9939 7.81819 11.8182L10.0682 9.56819Z" fill="currentColor" fill-rule="evenodd" clip-rule="evenodd" />
      </svg>
    </button>

    <GroundGlass v-if="open" :opacity="2.5" class="multi-select__dropdown">
      <div v-if="filterable" class="multi-select__search">
        <input
          v-model="filterText"
          type="text"
          class="multi-select__search-input"
          placeholder="Search..."
          @keydown.stop
        >
      </div>
      <div class="multi-select__list">
        <button
          v-for="opt in filteredOptions"
          :key="opt.value"
          type="button"
          class="multi-select__item"
          :class="{ 'multi-select__item--checked': model.includes(opt.value) }"
          @click="toggle(opt.value)"
        >
          <span class="multi-select__check">
            <svg v-if="model.includes(opt.value)" class="w-3 h-3 text-emerald-400" viewBox="0 0 15 15" fill="none">
              <path d="M11.4669 3.72684C11.7558 3.91574 11.8369 4.30308 11.648 4.59198L7.39799 11.092C7.29783 11.2452 7.13556 11.3467 6.95402 11.3699C6.77247 11.3931 6.58989 11.3354 6.45446 11.2124L3.70446 8.71241C3.44905 8.48022 3.43023 8.08494 3.66242 7.82953C3.89461 7.57412 4.28989 7.55529 4.5453 7.78749L6.75292 9.79441L10.6018 3.90792C10.7907 3.61902 11.178 3.53795 11.4669 3.72684Z" fill="currentColor" fill-rule="evenodd" clip-rule="evenodd" />
            </svg>
          </span>
          <span class="truncate">{{ opt.label }}</span>
        </button>
        <div v-if="filteredOptions.length === 0" class="multi-select__empty">No matches</div>
      </div>
    </GroundGlass>
  </div>
</template>

<style scoped>
.multi-select {
  position: relative;
}

.multi-select__trigger {
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
.multi-select__trigger:hover {
  border-color: rgba(71, 85, 105, 0.6);
}

.multi-select__text {
  flex: 1;
  text-align: left;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.multi-select__placeholder {
  flex: 1;
  text-align: left;
  color: rgba(148, 163, 184, 0.35);
}

.multi-select__icon {
  width: 14px;
  height: 14px;
  opacity: 0.5;
  flex-shrink: 0;
}

.multi-select__dropdown {
  position: absolute;
  top: calc(100% + 4px);
  left: 0;
  right: 0;
  z-index: 50;
  min-width: 180px;
  max-height: 280px;
  padding: 4px;
}

.multi-select__search {
  position: relative;
  z-index: 3;
  padding: 4px 4px 2px;
}

.multi-select__search-input {
  width: 100%;
  padding: 5px 8px;
  font-size: 12px;
  color: #e2e8f0;
  background: rgba(255, 255, 255, 0.04);
  border: 1px solid rgba(71, 85, 105, 0.3);
  border-radius: 0.375rem;
  outline: none;
  transition: border-color 0.15s ease;
}
.multi-select__search-input::placeholder {
  color: rgba(148, 163, 184, 0.3);
}
.multi-select__search-input:focus {
  border-color: rgba(139, 92, 246, 0.4);
}

.multi-select__list {
  position: relative;
  z-index: 1;
  overflow-y: auto;
  max-height: 232px;
}

.multi-select__item {
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
  background: transparent;
  border: none;
  transition: background 0.1s ease, color 0.1s ease;
  user-select: none;
  text-align: left;
}
.multi-select__item:hover {
  background: rgba(139, 92, 246, 0.12);
  color: #e2e8f0;
}
.multi-select__item--checked {
  color: #e2e8f0;
}

.multi-select__check {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 12px;
  height: 12px;
  flex-shrink: 0;
}

.multi-select__clear {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  color: rgba(148, 163, 184, 0.4);
  flex-shrink: 0;
  transition: color 0.15s ease;
}
.multi-select__clear:hover {
  color: rgba(248, 113, 113, 0.8);
}

.multi-select__empty {
  padding: 8px 10px;
  font-size: 12px;
  color: rgba(148, 163, 184, 0.3);
}
</style>
