<script setup lang="ts">
import { SelectContent as RadixSelectContent, type SelectContentProps, SelectPortal, SelectViewport } from "radix-vue";

const props = withDefaults(defineProps<SelectContentProps & { class?: string; filterable?: boolean }>(), {
  position: "popper",
  sideOffset: 4,
  filterable: false,
});

const { meshBg, containerStyle, onEnter, onLeave } = useGroundGlass({ opacity: 2.5 })

// Prevent animation on mount when cursor is already over the dropdown area
const ready = ref(false)
onMounted(() => { setTimeout(() => { ready.value = true }, 200) })

function handleEnter() { if (ready.value) onEnter() }
function handleLeave() { if (ready.value) onLeave() }

// Filter support
const filterText = ref('')
provide('nisaba-select-filter', filterText)

const searchRef = ref<HTMLInputElement | null>(null)

// Auto-focus search input when dropdown opens
onMounted(() => {
  if (props.filterable) {
    nextTick(() => searchRef.value?.focus())
  }
})

// Reset filter when unmounted (dropdown closes)
onBeforeUnmount(() => { filterText.value = '' })
</script>

<template>
  <SelectPortal>
    <RadixSelectContent
      v-bind="props"
      :class="['nisaba-select-content', props.class]"
      :style="containerStyle"
      @mouseenter="handleEnter"
      @mouseleave="handleLeave"
    >
      <span class="nisaba-select-mesh" :style="{ background: meshBg }" />
      <!-- Search input -->
      <div v-if="filterable" class="nisaba-select-search">
        <input
          ref="searchRef"
          v-model="filterText"
          type="text"
          class="nisaba-select-search__input"
          placeholder="Search..."
          @keydown.stop
        >
      </div>
      <SelectViewport
        :class="[
          'nisaba-select-viewport',
          position === 'popper' ? 'h-[var(--radix-select-trigger-height)] w-full min-w-[var(--radix-select-trigger-width)]' : '',
        ]"
      >
        <slot />
      </SelectViewport>
    </RadixSelectContent>
  </SelectPortal>
</template>

<style>
/* Unscoped — portaled to body, scoped styles don't apply */
.nisaba-select-content {
  position: relative;
  z-index: 50;
  isolation: isolate;
  overflow: hidden;
  min-width: 180px;
  max-height: 280px;
  border-radius: 0.75rem;
  padding: 4px;
  background: rgba(30, 41, 59, 0.30);
  /* Glass slab borders */
  border-top: 1px solid rgba(255, 255, 255, 0.06);
  border-left: 1px solid rgba(255, 255, 255, 0.04);
  border-bottom: 1.5px solid rgba(71, 85, 105, 0.18);
  border-right: 1px solid rgba(71, 85, 105, 0.10);
  box-shadow:
    inset 0 1px 0 0 rgba(255, 255, 255, 0.04),
    0 1.5px 1px -0.5px rgba(0, 0, 0, 0.18),
    0 3px 8px -3px rgba(0, 0, 0, 0.12);
  animation: nisaba-select-enter 0.15s ease-out;
}

@keyframes nisaba-select-enter {
  from { opacity: 0; transform: translateY(-4px) scale(0.97); }
  to   { opacity: 1; transform: translateY(0) scale(1); }
}

/* Layer 0: animated mesh gradient */
.nisaba-select-mesh {
  position: absolute;
  inset: 0;
  z-index: 0;
  pointer-events: none;
  border-radius: inherit;
}

/* Search input */
.nisaba-select-search {
  position: relative;
  z-index: 3;
  padding: 4px 4px 2px;
}
.nisaba-select-search__input {
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
.nisaba-select-search__input::placeholder {
  color: rgba(148, 163, 184, 0.3);
}
.nisaba-select-search__input:focus {
  border-color: rgba(139, 92, 246, 0.4);
}

/* Layer 1: scrollable viewport */
.nisaba-select-viewport {
  position: relative;
  z-index: 1;
  overflow-y: auto;
  max-height: 232px;
}

/* Layer 2: ground glass noise */
.nisaba-select-content::after {
  content: '';
  position: absolute;
  inset: 0;
  z-index: 2;
  pointer-events: none;
  border-radius: 0.75rem;
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
