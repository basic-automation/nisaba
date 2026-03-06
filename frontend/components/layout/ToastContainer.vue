<script setup lang="ts">
const { activeToasts, dismissToast } = useNotifications()
</script>

<template>
  <Teleport to="body">
    <div class="toast-container">
      <TransitionGroup name="toast-slide">
        <Toast
          v-for="notification in activeToasts"
          :key="notification.id"
          :notification="notification"
          @dismiss="dismissToast"
        />
      </TransitionGroup>
    </div>
  </Teleport>
</template>

<style scoped>
.toast-container {
  position: fixed;
  bottom: 1.5rem;
  right: 1.5rem;
  z-index: 9999;
  display: flex;
  flex-direction: column-reverse;
  gap: 0.625rem;
  pointer-events: none;
}

.toast-container :deep(> *) {
  pointer-events: auto;
}

/* Slide-in from right */
.toast-slide-enter-active {
  transition: all 0.35s cubic-bezier(0.4, 0, 0.2, 1);
}
.toast-slide-leave-active {
  transition: all 0.25s cubic-bezier(0.4, 0, 0.2, 1);
}
.toast-slide-enter-from {
  opacity: 0;
  transform: translateX(100%) scale(0.95);
}
.toast-slide-leave-to {
  opacity: 0;
  transform: translateX(30%) scale(0.95);
}
/* Smooth reflow when items are removed */
.toast-slide-move {
  transition: transform 0.3s cubic-bezier(0.4, 0, 0.2, 1);
}
</style>
