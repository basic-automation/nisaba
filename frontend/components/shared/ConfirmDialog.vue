<!-- N-11.3: the single confirmation mechanism for destructive actions.
     Replaces the three ad-hoc mechanisms (window.confirm, the Tauri dialog
     plugin, per-page inline modals) so every destructive action — unlinking a
     mapping, removing a peer, uninstalling a plugin, applying an import over
     live rows, deleting a product, restoring a backup — asks through this. -->
<script setup lang="ts">
const props = withDefaults(defineProps<{
  open: boolean
  title: string
  message: string
  confirmLabel?: string
  cancelLabel?: string
  /** Destructive confirmations render the action button red; non-dangerous ones accent. */
  danger?: boolean
  /** Disable the confirm button (e.g. while the action is running). */
  confirming?: boolean
}>(), {
  confirmLabel: 'Confirm',
  cancelLabel: 'Cancel',
  danger: true,
  confirming: false,
})

const emit = defineEmits<{
  (e: 'confirm'): void
  (e: 'cancel'): void
}>()

function onKeydown(e: KeyboardEvent) {
  if (e.key === 'Escape') emit('cancel')
}

watch(() => props.open, (open) => {
  if (open) window.addEventListener('keydown', onKeydown)
  else window.removeEventListener('keydown', onKeydown)
})

onUnmounted(() => window.removeEventListener('keydown', onKeydown))
</script>

<template>
  <Teleport to="body">
    <Transition name="modal">
      <div
        v-if="open"
        class="fixed inset-0 z-[60] flex items-center justify-center bg-black/40"
        @click.self="emit('cancel')"
      >
        <GroundGlass
          :opacity="3"
          :blur="12"
          :sizes="['70%', '70%', '65%']"
          class="p-8 w-full max-w-sm"
          style="background: rgba(30, 41, 59, 0.35)"
        >
          <div class="relative z-[1]">
            <h3 class="text-sm font-medium tracking-widest uppercase text-foreground/70 mb-4">{{ title }}</h3>
            <p class="text-sm text-muted/60 mb-8 whitespace-pre-line">{{ message }}</p>
            <div class="flex justify-end gap-3">
              <Button variant="solid" color="muted" size="sm" @click="emit('cancel')">
                {{ cancelLabel }}
              </Button>
              <Button
                variant="solid"
                :color="danger ? 'danger' : 'accent'"
                size="sm"
                :disabled="confirming"
                @click="emit('confirm')"
              >
                {{ confirming ? 'Working…' : confirmLabel }}
              </Button>
            </div>
          </div>
        </GroundGlass>
      </div>
    </Transition>
  </Teleport>
</template>
