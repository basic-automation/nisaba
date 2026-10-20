import { WebviewWindow, getCurrentWebviewWindow } from '@tauri-apps/api/webviewWindow'

/**
 * Label of the dedicated settings window.
 * Must match `windows` in crates/tauri-app/capabilities/settings.json —
 * secret-returning Tauri commands (get_company_secret, get_config,
 * get_company_platform_config) are allowed ONLY in this window.
 */
export const SETTINGS_WINDOW_LABEL = 'nisaba-settings'

export type SettingsView = 'config' | 'company' | 'setup'

const VIEW_TITLES: Record<SettingsView, string> = {
  config: 'Nisaba — Configuration',
  company: 'Nisaba — Company',
  setup: 'Nisaba — Platform Setup',
}

/**
 * Open the dedicated settings window on the requested view, or focus it if it
 * is already open. The settings views that legitimately display secrets live
 * there; the main window (which renders untrusted listing/plugin HTML) cannot
 * invoke the secret-returning commands.
 */
export async function openSettingsWindow(
  view: SettingsView,
  query: Record<string, string> = {},
): Promise<void> {
  const existing = await WebviewWindow.getByLabel(SETTINGS_WINDOW_LABEL)
  if (existing) {
    await existing.setFocus().catch(() => {})
    return
  }
  // The target view is passed via the URL hash so the window always loads `/`
  // (which the Tauri asset server can always resolve) and the SPA router
  // navigates client-side on startup.
  const qs = new URLSearchParams(query).toString()
  const win = new WebviewWindow(SETTINGS_WINDOW_LABEL, {
    url: `/#settings:${view}${qs ? `?${qs}` : ''}`,
    title: VIEW_TITLES[view],
    width: 1120,
    height: 780,
    minWidth: 960,
    minHeight: 640,
    center: true,
  })
  win.once('tauri://error', (e) => {
    console.error('[settings-window] failed to open', e)
  })
}

/** True when this code runs inside the dedicated settings window. */
export function isSettingsWindow(): boolean {
  try {
    return getCurrentWebviewWindow().label === SETTINGS_WINDOW_LABEL
  } catch {
    return false
  }
}

/**
 * Parse the settings view requested via the window URL hash,
 * e.g. `#settings:setup?companyId=abc`.
 */
export function parseSettingsHash(): { view: SettingsView; query: Record<string, string> } {
  const fallback = { view: 'config' as SettingsView, query: {} as Record<string, string> }
  if (typeof window === 'undefined') return fallback
  const m = window.location.hash.match(/^#settings:(config|company|setup)(?:\?(.*))?$/)
  if (!m) return fallback
  return {
    view: m[1] as SettingsView,
    query: Object.fromEntries(new URLSearchParams(m[2] ?? '')),
  }
}

/** Close the window this code runs in (used by settings-window flows). */
export async function closeCurrentWindow(): Promise<void> {
  await getCurrentWebviewWindow().close()
}
