<script setup lang="ts">
export type ButtonVariant = 'solid' | 'outline' | 'ghost' | 'text'
export type ButtonSize = 'xs' | 'sm' | 'md' | 'lg'
export type ButtonColor = 'default' | 'accent' | 'danger' | 'muted'

const props = withDefaults(defineProps<{
  variant?: ButtonVariant
  size?: ButtonSize
  color?: ButtonColor
  disabled?: boolean
  type?: 'button' | 'submit' | 'reset'
  to?: string
  class?: string
}>(), {
  variant: 'solid',
  size: 'sm',
  color: 'default',
  disabled: false,
  type: 'button',
})

// ── Mesh gradient colors per color prop ──
const meshColors: Record<string, [string, string, string]> = {
  default: ['148, 163, 184', '100, 116, 139', '120, 140, 160'],
  accent:  ['139, 92, 246', '167, 139, 250', '139, 92, 246'],
  danger:  ['239, 68, 68', '220, 38, 38', '248, 113, 113'],
  muted:   ['100, 116, 139', '71, 85, 105', '100, 116, 139'],
}

const hasMesh = computed(() => props.variant === 'solid' && !props.disabled)

const colors = computed(() => meshColors[props.color] || meshColors.default)

const { splatterBg, onEnter: meshEnter, onLeave: meshLeave } = useSplatter({
  colors,
})

function onEnter() {
  if (hasMesh.value) meshEnter()
}
function onLeave() {
  meshLeave()
}
</script>

<template>
  <NuxtLink
    v-if="props.to"
    :to="props.to"
    :class="[
      'ui-btn',
      `ui-btn--${props.variant}`,
      `ui-btn--${props.size}`,
      `ui-btn--${props.color}`,
      { 'ui-btn--disabled': props.disabled },
      props.class,
    ]"
    @mouseenter="onEnter"
    @mouseleave="onLeave"
  >
    <span
      v-if="hasMesh"
      class="ui-btn__mesh"
      :style="{ background: splatterBg }"
    />
    <span class="ui-btn__label"><slot /></span>
  </NuxtLink>
  <button
    v-else
    :type="props.type"
    :disabled="props.disabled || undefined"
    :class="[
      'ui-btn',
      `ui-btn--${props.variant}`,
      `ui-btn--${props.size}`,
      `ui-btn--${props.color}`,
      { 'ui-btn--disabled': props.disabled },
      props.class,
    ]"
    @mouseenter="onEnter"
    @mouseleave="onLeave"
  >
    <span
      v-if="hasMesh"
      class="ui-btn__mesh"
      :style="{ background: splatterBg }"
    />
    <span class="ui-btn__label"><slot /></span>
  </button>
</template>

<style scoped>
/* ════ Base ════ */
.ui-btn {
  position: relative;
  isolation: isolate;
  display: inline-flex;
  align-items: center;
  justify-content: center;
  gap: 6px;
  font-weight: 500;
  white-space: nowrap;
  cursor: pointer;
  outline: none;
  overflow: hidden;
  border: none;
  border-radius: 0;
  text-decoration: none;
  transition: color 0.2s ease, border-color 0.2s ease, background-color 0.2s ease;
}

.ui-btn--disabled {
  pointer-events: none;
  opacity: 0.4;
}

.ui-btn__mesh {
  position: absolute;
  inset: 0;
  z-index: 0;
  pointer-events: none;
  /* Grainy mask: noise luminance → alpha channel, range 5%–95% */
  -webkit-mask-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='200'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.65' numOctaves='3' stitchTiles='stitch' result='noise'/%3E%3CfeColorMatrix type='saturate' values='0' in='noise' result='gray'/%3E%3CfeColorMatrix type='matrix' in='gray' values='0 0 0 0 1 0 0 0 0 1 0 0 0 0 1 1 0 0 0 0' result='a'/%3E%3CfeComponentTransfer in='a'%3E%3CfeFuncA type='linear' slope='0.9' intercept='0.05'/%3E%3C/feComponentTransfer%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)'/%3E%3C/svg%3E");
  mask-image: url("data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='200' height='200'%3E%3Cfilter id='n'%3E%3CfeTurbulence type='fractalNoise' baseFrequency='0.65' numOctaves='3' stitchTiles='stitch' result='noise'/%3E%3CfeColorMatrix type='saturate' values='0' in='noise' result='gray'/%3E%3CfeColorMatrix type='matrix' in='gray' values='0 0 0 0 1 0 0 0 0 1 0 0 0 0 1 1 0 0 0 0' result='a'/%3E%3CfeComponentTransfer in='a'%3E%3CfeFuncA type='linear' slope='0.9' intercept='0.05'/%3E%3C/feComponentTransfer%3E%3C/filter%3E%3Crect width='100%25' height='100%25' filter='url(%23n)'/%3E%3C/svg%3E");
  -webkit-mask-size: 200px 200px;
  mask-size: 200px 200px;
}

.ui-btn__label {
  position: relative;
  z-index: 1;
}


/* ════ Sizes ════ */
.ui-btn--xs {
  padding: 7px 16px;
  font-size: 11px;
}
.ui-btn--sm {
  padding: 9px 22px;
  font-size: 12px;
}
.ui-btn--md {
  padding: 12px 28px;
  font-size: 13px;
}
.ui-btn--lg {
  padding: 14px 34px;
  font-size: 14px;
  height: 2.75rem;
}

/* ════ Variant: solid (mesh background) ════ */
.ui-btn--solid.ui-btn--default { color: rgba(226, 232, 240, 0.9); background: rgba(71, 85, 105, 0.25); }
.ui-btn--solid.ui-btn--accent  { color: #e2e8f0; background: rgba(139, 92, 246, 0.2); }
.ui-btn--solid.ui-btn--danger  { color: #e2e8f0; background: rgba(239, 68, 68, 0.2); }
.ui-btn--solid.ui-btn--muted   { color: rgba(196, 205, 214, 0.85); background: rgba(71, 85, 105, 0.2); }

/* ════ Variant: outline ════ */
.ui-btn--outline {
  background: transparent;
  border-radius: 0.375rem;
}
.ui-btn--outline.ui-btn--default {
  border: 1px solid rgba(71, 85, 105, 0.4);
  color: #94a3b8;
}
.ui-btn--outline.ui-btn--default:hover {
  border-color: rgba(71, 85, 105, 0.7);
  color: #e2e8f0;
}
.ui-btn--outline.ui-btn--accent {
  border: 1px solid rgba(139, 92, 246, 0.6);
  color: #8b5cf6;
}
.ui-btn--outline.ui-btn--accent:hover {
  background: rgba(139, 92, 246, 0.10);
  color: #a78bfa;
}
.ui-btn--outline.ui-btn--danger {
  border: 1px solid rgba(71, 85, 105, 0.4);
  color: rgba(148, 163, 184, 0.6);
}
.ui-btn--outline.ui-btn--danger:hover {
  border-color: rgba(248, 113, 113, 0.4);
  color: rgba(248, 113, 113, 0.8);
}
.ui-btn--outline.ui-btn--muted {
  border: 1px solid rgba(71, 85, 105, 0.4);
  color: rgba(148, 163, 184, 0.6);
}
.ui-btn--outline.ui-btn--muted:hover {
  border-color: rgba(71, 85, 105, 0.6);
  color: rgba(148, 163, 184, 0.8);
}

/* ════ Variant: ghost (subtle hover) ════ */
.ui-btn--ghost {
  background: transparent;
}
.ui-btn--ghost.ui-btn--default {
  color: rgba(196, 205, 214, 0.5);
}
.ui-btn--ghost.ui-btn--default:hover {
  color: rgba(196, 205, 214, 0.8);
  background: rgba(255, 255, 255, 0.04);
}
.ui-btn--ghost.ui-btn--accent {
  color: rgba(139, 92, 246, 0.7);
}
.ui-btn--ghost.ui-btn--accent:hover {
  color: #a78bfa;
  background: rgba(139, 92, 246, 0.08);
}
.ui-btn--ghost.ui-btn--danger {
  color: rgba(196, 205, 214, 0.5);
}
.ui-btn--ghost.ui-btn--danger:hover {
  color: rgba(248, 113, 113, 0.8);
  background: rgba(239, 68, 68, 0.08);
}
.ui-btn--ghost.ui-btn--muted {
  color: rgba(196, 205, 214, 0.4);
}
.ui-btn--ghost.ui-btn--muted:hover {
  color: rgba(196, 205, 214, 0.7);
}

/* ════ Variant: text (minimal, no background) ════ */
.ui-btn--text {
  background: transparent;
  padding-left: 0;
  padding-right: 0;
}
.ui-btn--text.ui-btn--default {
  color: rgba(196, 205, 214, 0.5);
}
.ui-btn--text.ui-btn--default:hover {
  color: rgba(196, 205, 214, 0.8);
}
.ui-btn--text.ui-btn--accent {
  color: #8b5cf6;
}
.ui-btn--text.ui-btn--accent:hover {
  color: rgba(139, 92, 246, 0.8);
}
.ui-btn--text.ui-btn--danger {
  color: rgba(196, 205, 214, 0.5);
}
.ui-btn--text.ui-btn--danger:hover {
  color: rgba(248, 113, 113, 0.8);
}
.ui-btn--text.ui-btn--muted {
  color: rgba(196, 205, 214, 0.35);
}
.ui-btn--text.ui-btn--muted:hover {
  color: rgba(196, 205, 214, 0.6);
}
</style>
