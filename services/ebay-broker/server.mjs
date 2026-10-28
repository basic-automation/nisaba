// Nisaba eBay OAuth broker.
//
// A stateless service that holds Nisaba's eBay client secret so desktop users can sign
// in with one click. It stores nothing and logs no query strings or bodies: the
// authorization code, the state and the tokens only ever pass through memory.
//
//   GET  /ebay/authorize?state=...   302 to eBay's consent page
//   GET  /ebay/callback              static page eBay redirects to (the RuName accept URL);
//                                    the app reads `code` and `state` from this URL
//   POST /ebay/token    {code}           exchange the code for tokens
//   POST /ebay/refresh  {refresh_token}  refresh an access token
//   GET  /healthz
import http from 'node:http'
import { pathToFileURL } from 'node:url'

export const SCOPES =
  'https://api.ebay.com/oauth/api_scope https://api.ebay.com/oauth/api_scope/sell.inventory https://api.ebay.com/oauth/api_scope/sell.account.readonly'

const ENVIRONMENTS = {
  production: {
    authUrl: 'https://auth.ebay.com/oauth2/authorize',
    tokenUrl: 'https://api.ebay.com/identity/v1/oauth2/token',
  },
  sandbox: {
    authUrl: 'https://auth.sandbox.ebay.com/oauth2/authorize',
    tokenUrl: 'https://api.sandbox.ebay.com/identity/v1/oauth2/token',
  },
}

const MAX_BODY_BYTES = 4096
const UPSTREAM_TIMEOUT_MS = 10_000
const MAX_TRACKED_CLIENTS = 10_000
const STATE_PATTERN = /^[A-Za-z0-9._~-]{16,256}$/
// Only these fields of eBay's token response are handed back to the app.
const TOKEN_FIELDS = ['access_token', 'expires_in', 'token_type', 'refresh_token', 'refresh_token_expires_in']

export function loadConfig(env = process.env) {
  const missing = ['EBAY_CLIENT_ID', 'EBAY_CLIENT_SECRET', 'EBAY_RUNAME'].filter((k) => !env[k])
  if (missing.length > 0) {
    throw new Error(`Missing required environment variable(s): ${missing.join(', ')}`)
  }
  const name = env.EBAY_ENV || 'production'
  const urls = ENVIRONMENTS[name]
  if (!urls) throw new Error(`EBAY_ENV must be "production" or "sandbox", got "${name}"`)
  return {
    clientId: env.EBAY_CLIENT_ID,
    clientSecret: env.EBAY_CLIENT_SECRET,
    ruName: env.EBAY_RUNAME,
    authUrl: urls.authUrl,
    tokenUrl: urls.tokenUrl,
    port: Number(env.PORT || 8080),
    trustProxy: env.TRUST_PROXY === '1',
    // Header that carries the real client IP when a CDN or tunnel sits in front of the
    // proxy (for example `Cf-Connecting-Ip` behind a Cloudflare tunnel). Only set it when
    // the broker is reachable solely through that path: any caller could forge the header.
    clientIpHeader: (env.CLIENT_IP_HEADER || '').trim().toLowerCase(),
    rateLimitPerMinute: Number(env.RATE_LIMIT_PER_MIN || 30),
  }
}

class RateLimiter {
  constructor(limit, windowMs = 60_000, now = () => Date.now()) {
    this.limit = limit
    this.windowMs = windowMs
    this.now = now
    this.hits = new Map()
  }

  allow(key) {
    const t = this.now()
    if (this.hits.size >= MAX_TRACKED_CLIENTS) this.sweep(t)
    // Still full of live entries: refuse new clients rather than grow without bound.
    if (this.hits.size >= MAX_TRACKED_CLIENTS && !this.hits.has(key)) return false
    const entry = this.hits.get(key)
    if (!entry || entry.resetAt <= t) {
      this.hits.set(key, { count: 1, resetAt: t + this.windowMs })
      return true
    }
    entry.count += 1
    return entry.count <= this.limit
  }

  sweep(t) {
    for (const [k, v] of this.hits) if (v.resetAt <= t) this.hits.delete(k)
  }
}

const SECURITY_HEADERS = {
  'Cache-Control': 'no-store',
  'Referrer-Policy': 'no-referrer',
  'X-Content-Type-Options': 'nosniff',
}

function sendJson(res, status, body) {
  const payload = JSON.stringify(body)
  res.writeHead(status, {
    ...SECURITY_HEADERS,
    'Content-Type': 'application/json',
    'Content-Length': Buffer.byteLength(payload),
  })
  res.end(payload)
}

const CALLBACK_PAGE = `<!doctype html><meta charset="utf-8"><title>Nisaba</title>
<p>Authorization received. You can return to Nisaba and close this window.</p>`

function readJsonBody(req) {
  return new Promise((resolve, reject) => {
    if (!/^application\/json\b/i.test(req.headers['content-type'] || '')) {
      return reject(Object.assign(new Error('Content-Type must be application/json'), { status: 415 }))
    }
    const chunks = []
    let size = 0
    req.on('data', (chunk) => {
      size += chunk.length
      if (size > MAX_BODY_BYTES) {
        // Stop buffering, answer 413, then close: destroying here would drop the
        // connection before the caller got any response.
        req.removeAllListeners('data')
        req.pause()
        reject(Object.assign(new Error('Request body too large'), { status: 413 }))
        return
      }
      chunks.push(chunk)
    })
    req.on('end', () => {
      try {
        const parsed = JSON.parse(Buffer.concat(chunks).toString('utf8'))
        if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) throw new Error()
        resolve(parsed)
      } catch {
        reject(Object.assign(new Error('Body must be a JSON object'), { status: 400 }))
      }
    })
    req.on('error', reject)
  })
}

export function createServer(config, { fetchImpl = fetch, log = (line) => console.log(line), now } = {}) {
  const limiter = new RateLimiter(config.rateLimitPerMinute, 60_000, now)
  const basic = 'Basic ' + Buffer.from(`${config.clientId}:${config.clientSecret}`).toString('base64')

  function clientKey(req) {
    if (config.clientIpHeader) {
      const fromHeader = String(req.headers[config.clientIpHeader] || '').split(',')[0].trim()
      if (fromHeader) return fromHeader
    }
    if (config.trustProxy) {
      // Behind one reverse proxy, the rightmost entry is the one the proxy itself added.
      const forwarded = String(req.headers['x-forwarded-for'] || '').split(',').pop().trim()
      if (forwarded) return forwarded
    }
    return req.socket.remoteAddress || 'unknown'
  }

  async function callEbay(form) {
    let resp
    try {
      resp = await fetchImpl(config.tokenUrl, {
        method: 'POST',
        headers: { Authorization: basic, 'Content-Type': 'application/x-www-form-urlencoded' },
        body: new URLSearchParams(form).toString(),
        signal: AbortSignal.timeout(UPSTREAM_TIMEOUT_MS),
      })
    } catch {
      throw Object.assign(new Error('eBay could not be reached'), { status: 504 })
    }
    let data = null
    try {
      data = await resp.json()
    } catch {
      /* non-JSON upstream body */
    }
    if (!resp.ok) {
      const detail = { error: 'upstream_rejected', upstream_status: resp.status }
      if (data && typeof data.error === 'string') detail.upstream_error = data.error.slice(0, 100)
      if (data && typeof data.error_description === 'string') {
        detail.upstream_error_description = data.error_description.slice(0, 200)
      }
      throw Object.assign(new Error('eBay rejected the request'), { status: 502, detail })
    }
    if (!data || typeof data.access_token !== 'string') {
      throw Object.assign(new Error('eBay returned an unexpected response'), { status: 502 })
    }
    return Object.fromEntries(TOKEN_FIELDS.filter((k) => k in data).map((k) => [k, data[k]]))
  }

  async function route(req, res, url) {
    const { pathname } = url
    if (req.method === 'GET' && pathname === '/healthz') return sendJson(res, 200, { ok: true })

    if (req.method === 'GET' && pathname === '/ebay/authorize') {
      const state = url.searchParams.get('state') || ''
      if (!STATE_PATTERN.test(state)) {
        return sendJson(res, 400, { error: 'invalid_state', message: 'state must be 16-256 URL-safe characters' })
      }
      const target = new URL(config.authUrl)
      target.search = new URLSearchParams({
        client_id: config.clientId,
        redirect_uri: config.ruName,
        response_type: 'code',
        scope: SCOPES,
        state,
      }).toString()
      res.writeHead(302, { ...SECURITY_HEADERS, Location: target.toString() })
      return res.end()
    }

    if (req.method === 'GET' && pathname === '/ebay/callback') {
      res.writeHead(200, {
        ...SECURITY_HEADERS,
        'Content-Type': 'text/html; charset=utf-8',
        'Content-Security-Policy': "default-src 'none'",
      })
      return res.end(CALLBACK_PAGE)
    }

    if (req.method === 'POST' && (pathname === '/ebay/token' || pathname === '/ebay/refresh')) {
      const body = await readJsonBody(req)
      if (pathname === '/ebay/token') {
        const { code } = body
        if (typeof code !== 'string' || code.length < 1 || code.length > 2048) {
          return sendJson(res, 400, { error: 'invalid_request', message: 'code is required' })
        }
        return sendJson(
          res,
          200,
          await callEbay({ grant_type: 'authorization_code', code, redirect_uri: config.ruName }),
        )
      }
      const { refresh_token: refreshToken } = body
      if (typeof refreshToken !== 'string' || refreshToken.length < 1 || refreshToken.length > 4096) {
        return sendJson(res, 400, { error: 'invalid_request', message: 'refresh_token is required' })
      }
      return sendJson(res, 200, await callEbay({ grant_type: 'refresh_token', refresh_token: refreshToken, scope: SCOPES }))
    }

    return sendJson(res, 404, { error: 'not_found' })
  }

  const server = http.createServer(async (req, res) => {
    const started = Date.now()
    const url = new URL(req.url || '/', 'http://broker.invalid')
    res.on('finish', () => {
      // Path only: the query string carries the authorization code and state.
      log(`${req.method} ${url.pathname} ${res.statusCode} ${Date.now() - started}ms`)
    })
    try {
      if (url.pathname !== '/healthz' && !limiter.allow(clientKey(req))) {
        res.setHeader('Retry-After', '60')
        return sendJson(res, 429, { error: 'rate_limited' })
      }
      await route(req, res, url)
    } catch (e) {
      if (res.headersSent) return
      const status = e.status || 500
      if (status === 413) res.setHeader('Connection', 'close')
      sendJson(res, status, e.detail || { error: status === 500 ? 'internal_error' : 'bad_request', message: status === 500 ? undefined : e.message })
    }
  })
  server.requestTimeout = 15_000
  server.headersTimeout = 10_000
  server.keepAliveTimeout = 5_000
  return server
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const config = loadConfig()
  createServer(config).listen(config.port, () => {
    console.log(`nisaba-ebay-broker listening on :${config.port} (${config.authUrl.includes('sandbox') ? 'sandbox' : 'production'})`)
  })
}
