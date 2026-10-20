import DOMPurify from 'dompurify'

/**
 * Sanitise untrusted HTML (marketplace / vendor-plugin listing descriptions)
 * before it reaches a `v-html` sink.
 *
 * The webview runs with full Tauri IPC access, so a `<script>` tag or an
 * `on*` event handler inside a supplier's description could invoke
 * secret-returning commands (`get_company_secret`, `get_config`). Every
 * `v-html` binding in the app must go through this function.
 *
 * DOMPurify only runs in the browser; on the server it returns an empty
 * string so untrusted markup is never rendered outside the hardened
 * client-side path.
 */
const SANITIZE_CONFIG: DOMPurify.Config = {
  // Tight allowlist: formatting, links, lists, tables, images. No
  // executable or embedding elements.
  ALLOWED_TAGS: [
    'a', 'b', 'blockquote', 'br', 'caption', 'code', 'col', 'colgroup',
    'del', 'div', 'em', 'figcaption', 'figure', 'h1', 'h2', 'h3', 'h4',
    'hr', 'i', 'img', 'li', 'ol', 'p', 'pre', 's', 'small', 'span',
    'strike', 'strong', 'sub', 'sup', 'table', 'tbody', 'td', 'tfoot',
    'th', 'thead', 'tr', 'u', 'ul',
  ],
  ALLOWED_ATTR: [
    'alt', 'class', 'colspan', 'height', 'href', 'rel', 'rowspan',
    'src', 'target', 'title', 'width',
  ],
  FORBID_TAGS: [
    'base', 'button', 'embed', 'form', 'iframe', 'input', 'link', 'math',
    'meta', 'object', 'script', 'select', 'style', 'svg', 'textarea',
    'video', 'audio',
  ],
  // Inline styles have no legitimate use in a product description; drop them
  // rather than auditing CSS values.
  FORBID_ATTR: ['style'],
  ALLOW_DATA_ATTR: false,
  // Keep DOMPurify's default URI allowlist (blocks javascript:, data:text/html,
  // etc.).
}

function hasDom(): boolean {
  return typeof window !== 'undefined' && typeof window.document !== 'undefined'
}

if (hasDom()) {
  // Harden outbound links opened from sanitised descriptions.
  DOMPurify.addHook('afterSanitizeAttributes', (node) => {
    if (node.tagName === 'A') {
      const target = node.getAttribute('target')
      if (target === '_blank') {
        node.setAttribute('rel', 'noopener noreferrer')
      }
    }
  })
}

export function sanitizeHtml(dirty: string | null | undefined): string {
  if (!dirty) return ''
  if (!hasDom()) return ''
  return DOMPurify.sanitize(dirty, SANITIZE_CONFIG)
}
