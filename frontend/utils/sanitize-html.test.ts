import { describe, expect, it } from 'vitest'
import { sanitizeHtml } from './sanitize-html'

describe('sanitizeHtml', () => {
  it('strips <script> payloads while keeping safe markup', () => {
    const out = sanitizeHtml(
      `<script>invoke('get_company_secret')</script><p>Safe <strong>description</strong></p>`,
    )
    expect(out).not.toContain('<script')
    expect(out).not.toContain('get_company_secret')
    expect(out).toContain('<p>Safe <strong>description</strong></p>')
  })

  it('strips event-handler attributes from every tag', () => {
    const out = sanitizeHtml(
      `<img src="x" onerror="invoke('get_config')"><div onclick="alert(1)" onmouseover="alert(2)">text</div>`,
    )
    expect(out).not.toContain('onerror')
    expect(out).not.toContain('onclick')
    expect(out).not.toContain('onmouseover')
    expect(out).not.toContain('get_config')
    expect(out).toContain('<img src="x">')
    expect(out).toContain('<div>text</div>')
  })

  it('neutralises javascript: URIs in links', () => {
    const out = sanitizeHtml(
      `<a href="javascript:invoke('get_company_secret')">click me</a>`,
    )
    expect(out).not.toContain('javascript:')
    expect(out).not.toContain('get_company_secret')
  })

  it('removes embedding / form elements entirely', () => {
    const out = sanitizeHtml(
      `<iframe src="https://evil.example"></iframe><object data="x"></object><form><input name="q"></form><p>ok</p>`,
    )
    expect(out).not.toContain('<iframe')
    expect(out).not.toContain('<object')
    expect(out).not.toContain('<form')
    expect(out).not.toContain('<input')
    expect(out).toContain('<p>ok</p>')
  })

  it('drops svg (scriptable vector markup) but keeps surrounding text', () => {
    const out = sanitizeHtml(`<p>before</p><svg onload="alert(1)"><circle/></svg><p>after</p>`)
    expect(out).not.toContain('<svg')
    expect(out).not.toContain('onload')
    expect(out).toContain('before')
    expect(out).toContain('after')
  })

  it('strips inline style attributes', () => {
    const out = sanitizeHtml(`<p style="position:fixed;top:0">text</p>`)
    expect(out).not.toContain('style=')
    expect(out).toContain('<p>text</p>')
  })

  it('preserves legitimate marketplace description markup', () => {
    const dirty =
      `<h2>Title</h2><p>Line one<br>Line <em>two</em></p>` +
      `<ul><li>a</li><li>b</li></ul>` +
      `<a href="https://example.com/item" target="_blank">buy</a> ` +
      `<img src="https://example.com/i.png" alt="photo">` +
      `<table><tr><th>h</th></tr><tr><td>c</td></tr></table>`
    const out = sanitizeHtml(dirty)
    expect(out).toContain('<h2>Title</h2>')
    expect(out).toContain('<ul><li>a</li><li>b</li></ul>')
    expect(out).toContain('href="https://example.com/item"')
    expect(out).toContain('target="_blank"')
    expect(out).toContain('rel="noopener noreferrer"')
    expect(out).toContain('<img src="https://example.com/i.png" alt="photo">')
    expect(out).toContain('<table>')
  })

  it('returns empty string for empty / nullish input', () => {
    expect(sanitizeHtml('')).toBe('')
    expect(sanitizeHtml(null)).toBe('')
    expect(sanitizeHtml(undefined)).toBe('')
  })
})
