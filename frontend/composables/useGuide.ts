import { WebviewWindow, getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'

/**
 * Label of the dedicated user-guide window.
 *
 * The guide is first-party static HTML shipped in `frontend/public/guide/`
 * (served at `/guide/` from the app's own origin). It is opened in its own
 * webview window so reading docs never navigates the main window
 * away from the app.
 */
export const GUIDE_WINDOW_LABEL = 'nisaba-guide'

/**
 * Open the in-app user guide, or focus it if it is already open.
 * Works from any window that holds the
 * `core:webview:allow-create-webview-window` permission.
 */
export async function openGuideWindow(): Promise<void> {
  const existing = await WebviewWindow.getByLabel(GUIDE_WINDOW_LABEL)
  if (existing) {
    await existing.setFocus().catch(() => {})
    return
  }
  const win = new WebviewWindow(GUIDE_WINDOW_LABEL, {
    url: '/guide/index.html',
    title: 'Nisaba — User guide',
    width: 1000,
    height: 780,
    minWidth: 800,
    minHeight: 600,
    center: true,
  })
  win.once('tauri://error', (e) => {
    console.error('[guide-window] failed to open', e)
  })
}

/** True when this code runs inside the dedicated guide window. */
export function isGuideWindow(): boolean {
  try {
    return getCurrentWebviewWindow().label === GUIDE_WINDOW_LABEL
  } catch {
    return false
  }
}
