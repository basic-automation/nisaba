import { check } from '@tauri-apps/plugin-updater'
import { relaunch } from '@tauri-apps/plugin-process'
import { invoke } from '@tauri-apps/api/core'

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
      return !!update
    } catch (e: any) {
      // Silently ignore check errors (offline, no endpoint configured, etc.)
      console.warn('Update check failed:', e)
      return false
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

  async function startAutoCheck() {
    // Background checks honour the updater opt-out (N-10.1): the check
    // leaves the machine, so a user who disabled it in Settings gets no
    // launch check and no timer. A manual checkForUpdate() call is an
    // explicit user action and is not gated here.
    let enabled = true
    try {
      enabled = await invoke<boolean>('get_update_check_enabled')
    } catch {
      // If the preference cannot be read, fall back to the default (on).
    }
    if (!enabled) return
    // Check on mount. Installing by itself happens only here, at launch, when there is no
    // unsaved work to lose; the later checks below just show the banner.
    checkForUpdate().then(async (found) => {
      if (!found) return
      let auto = false
      try {
        auto = await invoke<boolean>('get_auto_install_updates')
      } catch {
        // Unreadable preference: do not install unattended.
      }
      if (auto) await downloadAndInstall()
    })
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
