import { check } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'

export function useUpdater() {
  const updateAvailable = ref(false)
  const updateVersion = ref('')
  const updateBody = ref('')
  const downloading = ref(false)
  const downloadProgress = ref(0)
  const error = ref('')

  let updateObj: Awaited<ReturnType<typeof check>> | null = null
  let checkInterval: ReturnType<typeof setInterval> | null = null

  async function checkForUpdate() {
    try {
      error.value = ''
      const update = await check()
      if (update) {
        updateAvailable.value = true
        updateVersion.value = update.version
        updateBody.value = update.body ?? ''
        updateObj = update
      }
    } catch (e: any) {
      // Silently ignore check errors (offline, no endpoint configured, etc.)
      console.warn('Update check failed:', e)
    }
  }

  async function downloadAndInstall() {
    if (!updateObj) return
    downloading.value = true
    downloadProgress.value = 0
    try {
      let totalLen = 0
      let downloaded = 0
      await updateObj.downloadAndInstall((event) => {
        if (event.event === 'Started') {
          totalLen = event.data.contentLength ?? 0
        } else if (event.event === 'Progress') {
          downloaded += event.data.chunkLength
          if (totalLen > 0) {
            downloadProgress.value = Math.round((downloaded / totalLen) * 100)
          }
        } else if (event.event === 'Finished') {
          downloadProgress.value = 100
        }
      })
      await relaunch()
    } catch (e: any) {
      error.value = e?.toString() || 'Update failed'
      downloading.value = false
    }
  }

  function dismiss() {
    updateAvailable.value = false
  }

  function startAutoCheck() {
    // Check on mount
    checkForUpdate()
    // Check every 6 hours
    checkInterval = setInterval(checkForUpdate, 6 * 60 * 60 * 1000)
  }

  function stopAutoCheck() {
    if (checkInterval) {
      clearInterval(checkInterval)
      checkInterval = null
    }
  }

  return {
    updateAvailable,
    updateVersion,
    updateBody,
    downloading,
    downloadProgress,
    error,
    checkForUpdate,
    downloadAndInstall,
    dismiss,
    startAutoCheck,
    stopAutoCheck,
  }
}
