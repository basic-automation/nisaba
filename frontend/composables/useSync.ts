import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'
import type { SyncEvent, SyncEngineEvent } from '~/types'

export function useSync() {
  const isRunning = ref(true)
  const recentEvents = ref<SyncEvent[]>([])
  const liveEvents = ref<SyncEngineEvent[]>([])
  let unlisten: UnlistenFn | null = null

  async function triggerSync() {
    await invoke('trigger_sync')
  }

  async function pauseSync() {
    await invoke('pause_sync')
    isRunning.value = false
  }

  async function resumeSync() {
    await invoke('resume_sync')
    isRunning.value = true
  }

  async function fetchStatus() {
    isRunning.value = await invoke<boolean>('get_sync_status')
  }

  async function fetchRecentEvents(limit = 50) {
    recentEvents.value = await invoke<SyncEvent[]>('get_recent_events', { limit })
  }

  function startListening() {
    onMounted(async () => {
      unlisten = await listen<SyncEngineEvent>('sync-event', (event) => {
        liveEvents.value.unshift(event.payload)
        // Keep last 100 events in memory
        if (liveEvents.value.length > 100) {
          liveEvents.value = liveEvents.value.slice(0, 100)
        }
      })
    })

    onUnmounted(() => {
      if (unlisten) {
        unlisten()
        unlisten = null
      }
    })
  }

  return {
    isRunning,
    recentEvents,
    liveEvents,
    triggerSync,
    pauseSync,
    resumeSync,
    fetchStatus,
    fetchRecentEvents,
    startListening,
  }
}
