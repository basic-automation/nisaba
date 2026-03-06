<script setup lang="ts">
const props = withDefaults(defineProps<{
  min?: number
  max?: number
  step?: number
  placeholder?: string
  disabled?: boolean
}>(), {
  step: 1,
  disabled: false,
})

const model = defineModel<number | null>({ default: null })

const glassRef = ref<{ onEnter: () => void; onLeave: () => void } | null>(null)

function onFocusIn() {
  if (!props.disabled) glassRef.value?.onEnter()
}

function onFocusOut() {
  glassRef.value?.onLeave()
}

// ── Increment / decrement ──
function increment() {
  if (props.disabled) return
  const current = model.value ?? 0
  const next = current + props.step
  model.value = props.max != null ? Math.min(next, props.max) : next
}

function decrement() {
  if (props.disabled) return
  const current = model.value ?? 0
  const next = current - props.step
  model.value = props.min != null ? Math.max(next, props.min) : next
}

function onInput(e: Event) {
  const val = (e.target as HTMLInputElement).value
  if (val === '') {
    model.value = null
  } else {
    const num = Number(val)
    if (!isNaN(num)) model.value = num
  }
}
</script>

<template>
  <GroundGlass
    ref="glassRef"
    class="input-number"
    :class="{ 'input-number--disabled': disabled }"
    @focusin="onFocusIn"
    @focusout="onFocusOut"
  >
    <button
      class="input-number__btn"
      tabindex="-1"
      :disabled="disabled || (min != null && (model ?? 0) <= min)"
      @click="decrement"
    >
      <svg class="w-3 h-3" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.5">
        <line x1="2.5" y1="6" x2="9.5" y2="6" />
      </svg>
    </button>

    <input
      type="number"
      class="input-number__input"
      :value="model ?? ''"
      :placeholder="placeholder"
      :disabled="disabled"
      :min="min"
      :max="max"
      :step="step"
      @input="onInput"
    >

    <button
      class="input-number__btn"
      tabindex="-1"
      :disabled="disabled || (max != null && (model ?? 0) >= max)"
      @click="increment"
    >
      <svg class="w-3 h-3" viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.5">
        <line x1="6" y1="2.5" x2="6" y2="9.5" />
        <line x1="2.5" y1="6" x2="9.5" y2="6" />
      </svg>
    </button>
  </GroundGlass>
</template>

<style scoped>
.input-number {
  display: flex;
  align-items: center;
  border-radius: 0.5rem;
  transition: border-color 0.2s ease;
}

.input-number:focus-within {
  border-color: rgba(139, 92, 246, 0.4);
}

.input-number--disabled {
  pointer-events: none;
  opacity: 0.4;
}

/* Input field */
.input-number__input {
  position: relative;
  z-index: 1;
  flex: 1;
  min-width: 0;
  background: transparent;
  border: none;
  outline: none;
  padding: 8px 4px;
  font-size: 16px;
  color: #e2e8f0;
  text-align: center;
  -moz-appearance: textfield;
}
.input-number__input::placeholder {
  color: rgba(255, 255, 255, 0.35);
}
.input-number__input::-webkit-inner-spin-button,
.input-number__input::-webkit-outer-spin-button {
  -webkit-appearance: none;
  margin: 0;
}

/* +/- buttons */
.input-number__btn {
  position: relative;
  z-index: 1;
  display: flex;
  align-items: center;
  justify-content: center;
  width: 32px;
  height: 100%;
  padding: 8px 0;
  background: transparent;
  border: none;
  color: rgba(196, 205, 214, 0.5);
  cursor: pointer;
  outline: none;
  transition: color 0.15s ease, background 0.15s ease;
}
.input-number__btn:hover:not(:disabled) {
  color: #e2e8f0;
  background: rgba(255, 255, 255, 0.06);
}
.input-number__btn:disabled {
  opacity: 0.25;
  cursor: default;
}
</style>
