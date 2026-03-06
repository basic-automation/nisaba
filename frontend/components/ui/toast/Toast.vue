<script setup lang="ts">
import type { AppNotification, NotificationType } from '~/types'

const props = defineProps<{
  notification: AppNotification
  duration?: number
}>()

const emit = defineEmits<{
  dismiss: [id: string]
}>()

// Type-specific mesh gradient colors
const meshColors: Record<NotificationType, [string, string, string]> = {
  success: ['34,197,94', '74,222,128', '34,197,94'],
  error: ['239,68,68', '220,38,38', '248,113,113'],
  warning: ['251,191,36', '245,158,11', '252,211,77'],
  info: ['96,165,250', '59,130,246', '147,197,253'],
}

const meshBg = computed(() => {
  const c = meshColors[props.notification.type]
  return [
    `radial-gradient(at 20% 30%, rgba(${c[0]},0.18), transparent 55%)`,
    `radial-gradient(at 80% 70%, rgba(${c[1]},0.12), transparent 50%)`,
    `radial-gradient(at 50% 50%, rgba(${c[2]},0.06), transparent 45%)`,
  ].join(', ')
})

const effectiveDuration = computed(() =>
  props.duration ?? (props.notification.type === 'error' ? 8000 : 5000)
)

// Pause/resume on hover
const paused = ref(false)

function onMouseEnter() {
  paused.value = true
}

function onMouseLeave() {
  paused.value = false
}
</script>

<template>
  <div
    class="toast"
    :class="`toast--${notification.type}`"
    @mouseenter="onMouseEnter"
    @mouseleave="onMouseLeave"
  >
    <!-- Static mesh gradient -->
    <span class="toast__mesh" :style="{ background: meshBg }" />
    <!-- Glass noise overlay -->
    <span class="toast__noise" />

    <!-- Content -->
    <div class="toast__content">
      <!-- Type dot -->
      <span class="toast__dot" />

      <div class="toast__body">
        <p class="toast__title">{{ notification.title }}</p>
        <p class="toast__message">{{ notification.message }}</p>
      </div>

      <!-- Dismiss -->
      <button class="toast__close" @click="emit('dismiss', notification.id)">
        <svg class="w-3.5 h-3.5" viewBox="0 0 20 20" fill="none" stroke="currentColor" stroke-width="1.8">
          <path d="M6 6l8 8M14 6l-8 8" />
        </svg>
      </button>
    </div>

    <!-- Progress bar -->
    <div
      class="toast__progress"
      :style="{
        animationDuration: `${effectiveDuration}ms`,
        animationPlayState: paused ? 'paused' : 'running',
      }"
    />
  </div>
</template>

<style scoped>
.toast {
  position: relative;
  isolation: isolate;
  overflow: hidden;
  border-radius: 0.75rem;
  /* Glass slab borders (matches GroundGlass.vue) */
  border-top: 1px solid rgba(255, 255, 255, 0.06);
  border-left: 1px solid rgba(255, 255, 255, 0.04);
  border-bottom: 1.5px solid rgba(71, 85, 105, 0.18);
  border-right: 1px solid rgba(71, 85, 105, 0.10);
  box-shadow:
    inset 0 1px 0 0 rgba(255, 255, 255, 0.04),
    0 1.5px 1px -0.5px rgba(0, 0, 0, 0.18),
    0 3px 8px -3px rgba(0, 0, 0, 0.12),
    0 8px 24px -4px rgba(0, 0, 0, 0.15);
  backdrop-filter: blur(12px);
  -webkit-backdrop-filter: blur(12px);
  width: 360px;
  padding: 14px 16px;
  background: rgba(30, 41, 59, 0.65);
}

/* Mesh gradient layer */
.toast__mesh {
  position: absolute;
  inset: 0;
  z-index: 0;
  pointer-events: none;
  border-radius: inherit;
}

/* Glass noise overlay (matches GroundGlass.vue ::after) */
.toast__noise {
  position: absolute;
  inset: 0;
  z-index: 2;
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

/* Content layer */
.toast__content {
  position: relative;
  z-index: 3;
  display: flex;
  align-items: flex-start;
  gap: 10px;
}

/* Colored type dot */
.toast__dot {
  width: 8px;
  height: 8px;
  border-radius: 50%;
  flex-shrink: 0;
  margin-top: 4px;
}

.toast--success .toast__dot { background: #22c55e; box-shadow: 0 0 6px rgba(34, 197, 94, 0.4); }
.toast--error .toast__dot   { background: #ef4444; box-shadow: 0 0 6px rgba(239, 68, 68, 0.4); }
.toast--warning .toast__dot { background: #fbbf24; box-shadow: 0 0 6px rgba(251, 191, 36, 0.4); }
.toast--info .toast__dot    { background: #60a5fa; box-shadow: 0 0 6px rgba(96, 165, 250, 0.4); }

.toast__body {
  flex: 1;
  min-width: 0;
}

.toast__title {
  font-size: 13px;
  font-weight: 500;
  color: rgba(226, 232, 240, 0.9);
  line-height: 1.3;
  margin: 0;
}

.toast__message {
  font-size: 12px;
  color: rgba(226, 232, 240, 0.5);
  line-height: 1.4;
  margin: 2px 0 0;
  display: -webkit-box;
  -webkit-line-clamp: 2;
  -webkit-box-orient: vertical;
  overflow: hidden;
}

.toast__close {
  flex-shrink: 0;
  color: rgba(226, 232, 240, 0.25);
  cursor: pointer;
  padding: 2px;
  border-radius: 4px;
  transition: color 0.15s, background-color 0.15s;
}

.toast__close:hover {
  color: rgba(226, 232, 240, 0.6);
  background: rgba(255, 255, 255, 0.05);
}

/* Progress bar */
.toast__progress {
  position: absolute;
  bottom: 0;
  left: 0;
  height: 2px;
  border-radius: 0 0 0.75rem 0.75rem;
  animation: toast-countdown linear forwards;
}

@keyframes toast-countdown {
  from { width: 100%; }
  to { width: 0%; }
}

.toast--success .toast__progress { background: rgba(34, 197, 94, 0.45); }
.toast--error .toast__progress   { background: rgba(239, 68, 68, 0.45); }
.toast--warning .toast__progress { background: rgba(251, 191, 36, 0.45); }
.toast--info .toast__progress    { background: rgba(96, 165, 250, 0.45); }
</style>
