import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import type { UnlistenFn } from '@tauri-apps/api/event'

export interface LogLine {
  timestamp: string
  level: string
  target: string
  message: string
}

export function useLogs() {
  const lines = shallowRef<LogLine[]>([])
  let cursor = 0
  const levelFilter = ref('')
  const searchQuery = ref('')
  const autoScroll = ref(true)
  let unlisten: UnlistenFn | null = null

  // Buffer incoming log lines and flush at most once per animation frame
  let buffer: LogLine[] = []
  let flushScheduled = false

  function scheduleFlush() {
    if (flushScheduled) return
    flushScheduled = true
    requestAnimationFrame(() => {
      flushScheduled = false
      if (buffer.length === 0) return
      let updated = lines.value.concat(buffer)
      buffer = []
      if (updated.length > 2000) {
        updated = updated.slice(-1500)
      }
      lines.value = updated
    })
  }

  const filteredLines = computed(() => {
    let result = lines.value
    if (levelFilter.value) {
      result = result.filter(l => l.level === levelFilter.value)
    }
    if (searchQuery.value) {
      const q = searchQuery.value.toLowerCase()
      result = result.filter(l =>
        l.message.toLowerCase().includes(q) ||
        l.target.toLowerCase().includes(q)
      )
    }
    return result
  })

  async function fetchLogs() {
    const [newLines, newCursor] = await invoke<[LogLine[], number]>('get_logs', {
      cursor,
    })
    if (newLines.length > 0) {
      lines.value = lines.value.concat(newLines)
      cursor = newCursor
    }
  }

  function startListening() {
    onMounted(async () => {
      await fetchLogs()

      unlisten = await listen<LogLine>('log-line', (event) => {
        buffer.push(event.payload)
        cursor++
        scheduleFlush()
      })
    })

    onUnmounted(() => {
      if (unlisten) {
        unlisten()
        unlisten = null
      }
      // Flush any remaining buffered lines
      buffer = []
      flushScheduled = false
    })
  }

  function clear() {
    buffer = []
    lines.value = []
  }

  return {
    lines,
    filteredLines,
    levelFilter,
    searchQuery,
    autoScroll,
    startListening,
    fetchLogs,
    clear,
  }
}
