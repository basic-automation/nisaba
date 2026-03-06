import { invoke } from '@tauri-apps/api/core'

// HMR-safe global state (same pattern as useNotifications.ts)
const _hmr: Record<string, any> = (import.meta as any).hot?.data ?? {}

// Map: original URL → cached filename (e.g. "abc123.jpg")
const cacheMap: Ref<Map<string, string>> =
  _hmr.imageCacheMap ?? (_hmr.imageCacheMap = ref(new Map<string, string>()))

// Set of URLs currently being downloaded (avoids duplicate requests)
const pendingUrls: Set<string> =
  _hmr.imagePendingUrls ?? (_hmr.imagePendingUrls = new Set<string>())

// Whether the manifest has been loaded from disk (only needs to happen once)
let manifestLoaded: boolean = _hmr.manifestLoaded ?? (_hmr.manifestLoaded = false)

// Promise that resolves when the manifest finishes loading.
// ensureCached() awaits this to avoid racing ahead with an empty cacheMap.
let manifestPromise: Promise<void> | null =
  _hmr.manifestPromise ?? (_hmr.manifestPromise = null)

// Tauri v2 custom protocol URL format differs by platform:
//   Windows/Linux: http://<scheme>.localhost/{path}
//   macOS:         <scheme>://localhost/{path}
const isMac = typeof navigator !== 'undefined' && navigator.userAgent.includes('Macintosh')
const PROTOCOL_BASE = isMac
  ? 'cachedimg://localhost/'
  : 'http://cachedimg.localhost/'

/**
 * Derive the thumbnail filename from a full image filename.
 * e.g. "abc123.jpg" → "abc123_thumb.jpg"
 * Must match the Rust `thumb_filename()` logic exactly.
 */
function toThumbFilename(fname: string): string {
  const dot = fname.lastIndexOf('.')
  if (dot === -1) return `${fname}_thumb`
  return `${fname.slice(0, dot)}_thumb.${fname.slice(dot + 1)}`
}

export function useImageCache() {
  /**
   * Load the entire cache manifest from disk into cacheMap.
   * Called once on app startup — makes all previously-cached images
   * resolve instantly without any per-page IPC calls.
   * Also triggers background thumbnail generation for existing cache entries.
   */
  async function loadManifest(): Promise<void> {
    if (manifestLoaded) return
    // If already in-flight, return the existing promise (dedup concurrent calls)
    if (manifestPromise) return manifestPromise

    manifestPromise = (async () => {
      try {
        const entries = await invoke<[string, string][]>('load_image_cache_manifest')
        if (entries.length > 0) {
          const newMap = new Map(cacheMap.value)
          for (const [url, filename] of entries) {
            newMap.set(url, filename)
          }
          cacheMap.value = newMap
        }
        manifestLoaded = true
        _hmr.manifestLoaded = true

        // Backfill thumbnails for existing cached images in background
        invoke('generate_missing_thumbnails').catch(() => {})
      } catch (e) {
        console.warn('Failed to load image cache manifest:', e)
      }
    })()
    _hmr.manifestPromise = manifestPromise
    return manifestPromise
  }

  /**
   * Resolve an image URL to its full-size cached version.
   * Use this for detail/gallery views where full resolution is needed.
   */
  function resolveImage(url: string | null | undefined): string | null {
    if (!url) return null
    const cached = cacheMap.value.get(url)
    if (cached) return `${PROTOCOL_BASE}${cached}`
    return url
  }

  /**
   * Resolve an image URL to its thumbnail cached version (~400px wide, ~20-60KB).
   * Use this for grid/card views. Falls back to full cached image, then original URL.
   */
  function resolveThumb(url: string | null | undefined): string | null {
    if (!url) return null
    const cached = cacheMap.value.get(url)
    if (cached) return `${PROTOCOL_BASE}${toThumbFilename(cached)}`
    return url
  }

  /**
   * Download + cache a batch of image URLs in the background.
   * Updates cacheMap reactively as each batch completes.
   * Already-cached or in-progress URLs are skipped automatically.
   */
  async function cacheImages(urls: (string | null | undefined)[]): Promise<void> {
    const toCache: string[] = []
    for (const url of urls) {
      if (!url) continue
      if (cacheMap.value.has(url)) continue
      if (pendingUrls.has(url)) continue
      toCache.push(url)
    }
    if (toCache.length === 0) return

    // Mark as pending
    for (const url of toCache) pendingUrls.add(url)

    try {
      const results = await invoke<[string, string][]>('cache_images', { urls: toCache })
      // Update reactive map
      if (results.length > 0) {
        const newMap = new Map(cacheMap.value)
        for (const [originalUrl, filename] of results) {
          newMap.set(originalUrl, filename)
        }
        cacheMap.value = newMap
      }
    } catch (e) {
      console.warn('Image cache error:', e)
    } finally {
      for (const url of toCache) pendingUrls.delete(url)
    }
  }

  /**
   * Ensure images are cached: download any that aren't already in the cache map.
   * Waits for the manifest to load first so we don't re-download images that
   * are already on disk (avoiding a double-load: CDN then protocol switch).
   */
  async function ensureCached(urls: (string | null | undefined)[]): Promise<void> {
    // Wait for manifest so cacheMap is populated before we check what's missing
    if (!manifestLoaded && manifestPromise) {
      await manifestPromise
    }
    const missing = urls.filter((u): u is string => !!u && !cacheMap.value.has(u) && !pendingUrls.has(u))
    if (missing.length === 0) return
    await cacheImages(missing)
  }

  return {
    cacheMap: readonly(cacheMap),
    loadManifest,
    resolveImage,
    resolveThumb,
    cacheImages,
    ensureCached,
  }
}
