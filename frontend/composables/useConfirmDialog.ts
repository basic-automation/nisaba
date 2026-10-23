// N-11.3: shared confirmation state for destructive actions. Every page that
// asks "are you sure?" drives the single <ConfirmDialog> component through this.

export interface ConfirmRequest {
  title: string
  message: string
  confirmLabel?: string
  danger?: boolean
  action: () => void | Promise<void>
}

export function useConfirmDialog() {
  const dialog = ref({
    open: false,
    title: '',
    message: '',
    confirmLabel: 'Confirm',
    danger: true,
    action: null as (() => void | Promise<void>) | null,
  })
  const confirming = ref(false)

  function requestConfirm(req: ConfirmRequest) {
    dialog.value = {
      open: true,
      title: req.title,
      message: req.message,
      confirmLabel: req.confirmLabel ?? 'Confirm',
      danger: req.danger ?? true,
      action: req.action,
    }
  }

  async function confirmDialogAction() {
    const action = dialog.value.action
    dialog.value.action = null
    dialog.value.open = false
    if (!action) return
    confirming.value = true
    try {
      await action()
    } finally {
      confirming.value = false
    }
  }

  function cancelDialog() {
    dialog.value.action = null
    dialog.value.open = false
  }

  return { dialog, confirming, requestConfirm, confirmDialogAction, cancelDialog }
}
