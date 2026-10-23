import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'
import type {
  SyncEvent,
  SyncEngineEvent,
  SyncStatusPayload,
  DryRunChangePayload,
  CycleReconciliationReport,
} from '~/types'

// N-11.7: one shared sync store. The old per-call `ref(true)` meant every
// `useSync()` got its own `isRunning` starting at `true`, so the dashboard and
// the sidebar disagreed after a pause and nothing refetched on company
// switch. State now lives at module scope (HMR-safe, like useCompanyContext),
// the engine event listener is registered once, pause/resume update it via
// the backend `sync-status` event, and the status refetches on company switch.
// `null` = unknown (never incorrectly `true` before the first fetch).
// N-11.6's dry-run preview and per-cycle reconciliation state lives here too,
// so every surface shares the same preview/reconciliation view.
const _hmr: Record<string, any> = (import.meta as any).hot?.data ?? {}

const isRunning: Ref<boolean | null> =
  _hmr.syncIsRunning ?? (_hmr.syncIsRunning = ref<boolean | null>(null))
const recentEvents: Ref<SyncEvent[]> =
  _hmr.syncRecentEvents ?? (_hmr.syncRecentEvents = ref<SyncEvent[]>([]))
const liveEvents: Ref<SyncEngineEvent[]> =
  _hmr.syncLiveEvents ?? (_hmr.syncLiveEvents = ref<SyncEngineEvent[]>([]))

// N-11.6: dry-run preview state, accumulated from the bracketed
// DryRunStarted / DryRunChange* / DryRunComplete event sequence.
const previewActive: Ref<boolean> =
  _hmr.syncPreviewActive ?? (_hmr.syncPreviewActive = ref(false))
const previewRan: Ref<boolean> =
  _hmr.syncPreviewRan ?? (_hmr.syncPreviewRan = ref(false))
const previewChanges: Ref<DryRunChangePayload[]> =
  _hmr.syncPreviewChanges ?? (_hmr.syncPreviewChanges = ref<DryRunChangePayload[]>([]))
// N-11.6: per-cycle reconciliation summary for the last real cycle.
const lastCycleReport: Ref<CycleReconciliationReport | null> =
  _hmr.syncLastCycleReport ?? (_hmr.syncLastCycleReport = ref<CycleReconciliationReport | null>(null))

let unlistenSync: UnlistenFn | null = _hmr.syncUnlisten ?? null
let unlistenStatus: UnlistenFn | null = _hmr.syncStatusUnlisten ?? null
let listenersSetup: boolean = _hmr.syncListenersSetup ?? false

// Per-cycle accumulators, reset at each CycleComplete / DryRunStarted.
// Module-scope so the single shared listener keeps one running total.
let cycleChangesByPlatform: Record<string, number> =
  _hmr.syncCycleChangesByPlatform ?? (_hmr.syncCycleChangesByPlatform = {})
let cycleErrors: string[] = _hmr.syncCycleErrors ?? (_hmr.syncCycleErrors = [])
let cycleUnmapped: number = _hmr.syncCycleUnmapped ?? 0

function resetCycleAccumulators() {
  cycleChangesByPlatform = {}
  cycleErrors = []
  cycleUnmapped = 0
}

export function useSync() {
  async function triggerSync() {
    await invoke('trigger_sync')
  }

  async function previewSync() {
    await invoke('preview_sync')
  }

  async function pauseSync() {
    await invoke('pause_sync')
    // Optimistic: the backend `sync-status` event confirms it for every listener.
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
    if (listenersSetup) return
    listenersSetup = true
    _hmr.syncListenersSetup = true

    // Single shared subscription to engine events (N-11.7: was one listener
    // per useSync() call, each appending to its own list). It also accumulates
    // N-11.6's dry-run preview and per-cycle reconciliation report.
    listen<SyncEngineEvent>('sync-event', (event) => {
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
    }).then((u) => {
      unlistenSync = u
      _hmr.syncUnlisten = u
    })

    // Engine-driven status updates: pause_sync/resume_sync emit this, so the
    // store stays correct no matter which surface toggled it.
    listen<SyncStatusPayload>('sync-status', (event) => {
      isRunning.value = event.payload.running
    }).then((u) => {
      unlistenStatus = u
      _hmr.syncStatusUnlisten = u
    })

    // Refetch on company switch (N-11.7: nothing refetched before, so the
    // dashboard and sidebar showed the previous company's state).
    const { onCompanySwitch } = useCompanyContext()
    onCompanySwitch(() => {
      fetchStatus().catch(() => {})
      liveEvents.value = []
      // Per-company view: the preview/reconciliation state belonged to the
      // previous company's stream — clear it with the rest.
      previewActive.value = false
      previewRan.value = false
      previewChanges.value = []
      lastCycleReport.value = null
      resetCycleAccumulators()
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
