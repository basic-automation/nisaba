/**
 * Rewrite CDN image URLs to request smaller thumbnails.
 *
 * - Squarespace: append `?format={size}w` (strip existing ?format= if present)
 * - eBay: replace `s-l{N}` suffix with `s-l{size}`
 * - Unknown / XMR Bazaar: pass through unchanged
 */
export function thumbnailUrl(url: string, size: number = 300): string {
  if (!url) return url

  // Squarespace CDN: images.squarespace-cdn.com
  if (url.includes('squarespace-cdn.com')) {
    const base = url.split('?')[0]
    return `${base}?format=${size}w`
  }

  // eBay CDN: i.ebayimg.com — replace s-l{N} with s-l{size}
  if (url.includes('ebayimg.com')) {
    return url.replace(/s-l\d+/, `s-l${size}`)
  }

  return url
}
