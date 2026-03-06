import type { AppNotification, NotificationType } from '~/types'

// HMR-safe global state (same pattern as useCompanyContext.ts)
const _hmr: Record<string, any> = (import.meta as any).hot?.data ?? {}

const notifications: Ref<AppNotification[]> =
  _hmr.notifications ?? (_hmr.notifications = ref<AppNotification[]>([]))

let idCounter: number = _hmr.notifIdCounter ?? (_hmr.notifIdCounter = 0)

const MAX_NOTIFICATIONS = 200

export function useNotifications() {
  const unreadCount = computed(() =>
    notifications.value.filter(n => !n.read).length
  )

  // Active toasts: not yet dismissed, newest first, max 5
  const activeToasts = computed(() =>
    notifications.value
      .filter(n => !n.dismissed)
      .slice(0, 5)
  )

  function notify(opts: {
    type: NotificationType
    title: string
    message: string
    detail?: string
    source?: string
    duration?: number
  }): string {
    const id = `notif-${++idCounter}`
    _hmr.notifIdCounter = idCounter

    const notification: AppNotification = {
      id,
      type: opts.type,
      title: opts.title,
      message: opts.message,
      detail: opts.detail,
      source: opts.source,
      timestamp: Date.now(),
      read: false,
      dismissed: false,
    }

    // Prepend newest first
    notifications.value = [notification, ...notifications.value]

    // Cap at max
    if (notifications.value.length > MAX_NOTIFICATIONS) {
      notifications.value = notifications.value.slice(0, MAX_NOTIFICATIONS)
    }

    // Auto-dismiss toast
    const duration = opts.duration ?? (opts.type === 'error' ? 8000 : 5000)
    if (duration > 0) {
      setTimeout(() => dismissToast(id), duration)
    }

    return id
  }

  function dismissToast(id: string) {
    const n = notifications.value.find(n => n.id === id)
    if (n) n.dismissed = true
  }

  function markRead(id: string) {
    const n = notifications.value.find(n => n.id === id)
    if (n) n.read = true
  }

  function markAllRead() {
    for (const n of notifications.value) {
      n.read = true
    }
  }

  function clearAll() {
    notifications.value = []
  }

  function remove(id: string) {
    notifications.value = notifications.value.filter(n => n.id !== id)
  }

  return {
    notifications: readonly(notifications),
    activeToasts,
    unreadCount,
    notify,
    dismissToast,
    markRead,
    markAllRead,
    clearAll,
    remove,
  }
}
