import { listen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'

export function useTauriEvent<T>(eventName: string, handler: (payload: T) => void) {
  let unlisten: UnlistenFn | null = null

  onMounted(async () => {
    unlisten = await listen<T>(eventName, (event) => {
      handler(event.payload)
    })
  })

  onUnmounted(() => {
    if (unlisten) {
      unlisten()
      unlisten = null
    }
  })
}
