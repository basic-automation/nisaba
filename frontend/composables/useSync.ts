import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'
import type {
  SyncEvent,
  SyncEngineEvent,
  DryRunChangePayload,
  CycleReconciliationReport,
} from '~/types'

export function useSync() {
  const isRunning = ref(true)
  const recentEvents = ref<SyncEvent[]>([])
  const liveEvents = ref<SyncEngineEvent[]>([])
  // N-11.6: dry-run preview state, accumulated from the bracketed
  // DryRunStarted / DryRunChange* / DryRunComplete event sequence.
  const previewActive = ref(false)
  const previewRan = ref(false)
  const previewChanges = ref<DryRunChangePayload[]>([])
  // N-11.6: per-cycle reconciliation summary for the last real cycle.
  const lastCycleReport = ref<CycleReconciliationReport | null>(null)
  let unlisten: UnlistenFn | null = null
  // Per-cycle accumulators, reset at each CycleComplete / DryRunStarted.
  let cycleChangesByPlatform: Record<string, number> = {}
  let cycleErrors: string[] = []
  let cycleUnmapped = 0

  function resetCycleAccumulators() {
    cycleChangesByPlatform = {}
    cycleErrors = []
    cycleUnmapped = 0
  }

  async function triggerSync() {
    await invoke('trigger_sync')
  }

  async function previewSync() {
    await invoke('preview_sync')
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
        const payload = event.payload
        liveEvents.value.unshift(payload)
        // Keep last 100 events in memory
        if (liveEvents.value.length > 100) {
          liveEvents.value = liveEvents.value.slice(0, 100)
        }

        // N-11.6: accumulate the dry-run preview and the per-cycle
        // reconciliation report from the live event stream.
        if ('DryRunStarted' in payload) {
          previewActive.value = true
          previewRan.value = false
          previewChanges.value = []
          // A preview's own polls/errors must not leak into the next real
          // cycle's report.
          resetCycleAccumulators()
        } else if ('DryRunChange' in payload) {
          previewChanges.value.push(payload.DryRunChange)
        } else if ('DryRunComplete' in payload) {
          previewActive.value = false
          previewRan.value = true
        } else if ('CycleComplete' in payload) {
          if (!previewActive.value) {
            lastCycleReport.value = {
              cycle_id: payload.CycleComplete.cycle_id,
              products_synced: payload.CycleComplete.products_synced,
              changes_pushed: payload.CycleComplete.changes_pushed,
              changes_by_platform: { ...cycleChangesByPlatform },
              errors: [...cycleErrors],
              unmapped_listings: cycleUnmapped,
            }
          }
          // The dry-run cycle's own CycleComplete still closes the bracket;
          // either way the accumulators restart for the next cycle.
          resetCycleAccumulators()
        } else if (!previewActive.value) {
          // Only attribute per-cycle events to a real cycle — events fired
          // during a preview describe the preview, not the last sync.
          if ('QuantityUpdated' in payload) {
            const platform = payload.QuantityUpdated.platform
            cycleChangesByPlatform[platform] = (cycleChangesByPlatform[platform] ?? 0) + 1
          } else if ('PlatformError' in payload) {
            cycleErrors.push(`${payload.PlatformError.platform}: ${payload.PlatformError.message}`)
          } else if ('UnmappedListingsDetected' in payload) {
            cycleUnmapped += payload.UnmappedListingsDetected.count
          }
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
    previewActive,
    previewRan,
    previewChanges,
    lastCycleReport,
    triggerSync,
    previewSync,
    pauseSync,
    resumeSync,
    fetchStatus,
    fetchRecentEvents,
    startListening,
  }
}
