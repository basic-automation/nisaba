import { test, before, after } from 'node:test'
import assert from 'node:assert/strict'
import http from 'node:http'
import { createServer, loadConfig, SCOPES } from './server.mjs'

const STATE = 'abcdefghijklmnop0123456789'
const SECRET = 'super-secret-value'
let ebayCalls
let ebay
let ebayUrl

before(async () => {
  ebay = http.createServer((req, res) => {
    let body = ''
    req.on('data', (c) => (body += c))
    req.on('end', () => {
      ebayCalls.push({ headers: req.headers, form: Object.fromEntries(new URLSearchParams(body)) })
      const form = Object.fromEntries(new URLSearchParams(body))
      if (form.code === 'bad-code' || form.refresh_token === 'bad-refresh') {
        res.writeHead(400, { 'Content-Type': 'application/json' })
        return res.end(JSON.stringify({ error: 'invalid_grant', error_description: 'code expired', secret_internal: 'x' }))
      }
      res.writeHead(200, { 'Content-Type': 'application/json' })
      res.end(JSON.stringify({
        access_token: 'AT', expires_in: 7200, token_type: 'User Access Token',
        refresh_token: 'RT', refresh_token_expires_in: 47304000, internal_debug: 'must-not-leak',
      }))
    })
  })
  await new Promise((r) => ebay.listen(0, '127.0.0.1', r))
  ebayUrl = `http://127.0.0.1:${ebay.address().port}/token`
})
after(() => ebay.close())

function config(over = {}) {
  return { ...loadConfig({ EBAY_CLIENT_ID: 'CID', EBAY_CLIENT_SECRET: SECRET, EBAY_RUNAME: 'RU-name', EBAY_ENV: 'sandbox' }),
    tokenUrl: ebayUrl, rateLimitPerMinute: 1000, ...over }
}

async function withBroker(over, fn, opts = {}) {
  ebayCalls = []
  const logs = []
  const server = createServer(config(over), { log: (l) => logs.push(l), ...opts })
  await new Promise((r) => server.listen(0, '127.0.0.1', r))
  const base = `http://127.0.0.1:${server.address().port}`
  try { await fn(base, logs) } finally { server.close(); server.closeAllConnections?.() }
}

const AMAZON_ENV = {
  AMAZON_APP_ID: 'amzn1.sp.solution.test-app',
  AMAZON_CLIENT_ID: 'amzn1.application-oa2-client.test',
  AMAZON_CLIENT_SECRET: 'amazon-secret-value',
  AMAZON_REDIRECT_URI: 'https://auth.example.test/amazon/callback',
  AMAZON_DRAFT: '1',
}
const ASTATE = '54321.abcdefghijklmnopqrst'
const withAmazon = (over, fn, opts) =>
  withBroker({ amazon: loadConfig({ EBAY_CLIENT_ID: 'a', EBAY_CLIENT_SECRET: 'b', EBAY_RUNAME: 'c', ...AMAZON_ENV }).amazon, ...over }, fn, opts)

const post = (url, body, headers = { 'Content-Type': 'application/json' }) =>
  fetch(url, { method: 'POST', headers, body: typeof body === 'string' ? body : JSON.stringify(body) })

test('loadConfig requires the three eBay values and a known environment', () => {
  assert.throws(() => loadConfig({}), /EBAY_CLIENT_ID, EBAY_CLIENT_SECRET, EBAY_RUNAME/)
  assert.throws(() => loadConfig({ EBAY_CLIENT_ID: 'a', EBAY_CLIENT_SECRET: 'b', EBAY_RUNAME: 'c', EBAY_ENV: 'prod' }), /EBAY_ENV/)
  assert.equal(loadConfig({ EBAY_CLIENT_ID: 'a', EBAY_CLIENT_SECRET: 'b', EBAY_RUNAME: 'c' }).authUrl, 'https://auth.ebay.com/oauth2/authorize')
})

test('authorize redirects to eBay with client id, RuName, scopes and the caller state', () =>
  withBroker({}, async (base) => {
    const r = await fetch(`${base}/ebay/authorize?state=${STATE}`, { redirect: 'manual' })
    assert.equal(r.status, 302)
    const loc = new URL(r.headers.get('location'))
    assert.equal(loc.origin + loc.pathname, 'https://auth.sandbox.ebay.com/oauth2/authorize')
    assert.equal(loc.searchParams.get('client_id'), 'CID')
    assert.equal(loc.searchParams.get('redirect_uri'), 'RU-name')
    assert.equal(loc.searchParams.get('response_type'), 'code')
    assert.equal(loc.searchParams.get('scope'), SCOPES)
    assert.equal(loc.searchParams.get('state'), STATE)
    assert.ok(!loc.toString().includes(SECRET))
  }))

test('authorize rejects a missing or malformed state', () =>
  withBroker({}, async (base) => {
    for (const q of ['', '?state=short', `?state=${'a'.repeat(300)}`, '?state=has spaces and !! chars ......']) {
      const r = await fetch(`${base}/ebay/authorize${q}`, { redirect: 'manual' })
      assert.equal(r.status, 400, q)
    }
  }))

test('callback page is static, uncacheable and leaks no referrer', () =>
  withBroker({}, async (base) => {
    const r = await fetch(`${base}/ebay/callback?code=SECRETCODE&state=${STATE}`)
    assert.equal(r.status, 200)
    assert.equal(r.headers.get('referrer-policy'), 'no-referrer')
    assert.equal(r.headers.get('cache-control'), 'no-store')
    assert.equal(r.headers.get('content-security-policy'), "default-src 'none'")
    assert.ok(!(await r.text()).includes('SECRETCODE'))
  }))

test('callback forwards a loopback sign-in to the app on 127.0.0.1, passing only code and state', () =>
  withBroker({}, async (base) => {
    const r = await fetch(`${base}/ebay/callback?code=CODE123&state=54321.abcdefghijklmnopqrst&expires_in=299&evil=1`, { redirect: 'manual' })
    assert.equal(r.status, 302)
    assert.equal(r.headers.get('location'), 'http://127.0.0.1:54321/ebay/callback?code=CODE123&state=54321.abcdefghijklmnopqrst')
    assert.equal(r.headers.get('referrer-policy'), 'no-referrer')
  }))

test('callback forwards a decline to the app too', () =>
  withBroker({}, async (base) => {
    const r = await fetch(`${base}/ebay/callback?error=access_denied&error_description=The+user+denied&state=40000.abcdefghijklmnopqrst`, { redirect: 'manual' })
    assert.equal(r.status, 302)
    const loc = new URL(r.headers.get('location'))
    assert.equal(loc.origin, 'http://127.0.0.1:40000')
    assert.equal(loc.searchParams.get('error'), 'access_denied')
  }))

test('callback never redirects anywhere but loopback, and only for a valid port prefix', () =>
  withBroker({}, async (base) => {
    for (const state of [
      'abcdefghijklmnop0123',                         // no port: the page, as for the embedded flow
      '80.abcdefghijklmnopqrst',                      // port too short
      '1023.abcdefghijklmnopqrst',                    // privileged range
      '65536.abcdefghijklmnopqrst',                   // out of range
      '12345.short',                                  // random part too short
      'evil.com:80.abcdefghijklmnopqrst',             // not digits
      '//evil.example/.abcdefghijklmnopqrst',
      '54321@evil.example.abcdefghijklmnopqrst',
    ]) {
      const r = await fetch(`${base}/ebay/callback?code=C&state=${encodeURIComponent(state)}`, { redirect: 'manual' })
      assert.equal(r.status, 200, state)
      assert.equal(r.headers.get('location'), null, state)
    }
  }))

test('token exchange uses Basic auth, the RuName, and returns only the whitelisted fields', () =>
  withBroker({}, async (base) => {
    const r = await post(`${base}/ebay/token`, { code: 'good-code' })
    assert.equal(r.status, 200)
    const body = await r.json()
    assert.deepEqual(Object.keys(body).sort(), ['access_token', 'expires_in', 'refresh_token', 'refresh_token_expires_in', 'token_type'])
    const call = ebayCalls[0]
    assert.equal(call.headers.authorization, 'Basic ' + Buffer.from(`CID:${SECRET}`).toString('base64'))
    assert.deepEqual(call.form, { grant_type: 'authorization_code', code: 'good-code', redirect_uri: 'RU-name' })
    assert.ok(!JSON.stringify(body).includes(SECRET))
  }))

test('refresh sends the refresh grant with the fixed scopes', () =>
  withBroker({}, async (base) => {
    const r = await post(`${base}/ebay/refresh`, { refresh_token: 'my-rt', scope: 'attacker-chosen' })
    assert.equal(r.status, 200)
    assert.deepEqual(ebayCalls[0].form, { grant_type: 'refresh_token', refresh_token: 'my-rt', scope: SCOPES })
  }))

test('an eBay rejection becomes a 502 that carries eBay\'s error code but nothing else', () =>
  withBroker({}, async (base) => {
    const r = await post(`${base}/ebay/token`, { code: 'bad-code' })
    assert.equal(r.status, 502)
    assert.deepEqual(await r.json(), { error: 'upstream_rejected', upstream_status: 400, upstream_error: 'invalid_grant', upstream_error_description: 'code expired' })
  }))

test('an unreachable eBay is a 504', () =>
  withBroker({}, async (base) => {
    const r = await post(`${base}/ebay/token`, { code: 'x' })
    assert.equal(r.status, 504)
  }, { fetchImpl: async () => { throw new Error('ECONNREFUSED') } }))

test('rejects non-JSON content type, oversized bodies and malformed bodies', () =>
  withBroker({}, async (base) => {
    assert.equal((await post(`${base}/ebay/token`, 'code=x', { 'Content-Type': 'text/plain' })).status, 415)
    assert.equal((await post(`${base}/ebay/token`, { code: 'a'.repeat(5000) })).status, 413)
    assert.equal((await post(`${base}/ebay/token`, '{not json')).status, 400)
    assert.equal((await post(`${base}/ebay/token`, '[]')).status, 400)
    assert.equal((await post(`${base}/ebay/token`, {})).status, 400)
    assert.equal((await post(`${base}/ebay/refresh`, { refresh_token: 5 })).status, 400)
    assert.equal(ebayCalls.length, 0)
  }))

test('rate limits per client, and healthz is exempt', () =>
  withBroker({ rateLimitPerMinute: 3 }, async (base) => {
    for (let i = 0; i < 3; i++) assert.equal((await fetch(`${base}/ebay/callback`)).status, 200)
    const limited = await fetch(`${base}/ebay/callback`)
    assert.equal(limited.status, 429)
    assert.equal(limited.headers.get('retry-after'), '60')
    assert.equal((await fetch(`${base}/healthz`)).status, 200)
  }))

test('with TRUST_PROXY the rightmost X-Forwarded-For entry is the client', () =>
  withBroker({ rateLimitPerMinute: 1, trustProxy: true }, async (base) => {
    const get = (xff) => fetch(`${base}/ebay/callback`, { headers: { 'X-Forwarded-For': xff } })
    assert.equal((await get('1.1.1.1, 9.9.9.9')).status, 200)
    assert.equal((await get('2.2.2.2, 9.9.9.9')).status, 429) // same proxy-added client
    assert.equal((await get('1.1.1.1, 8.8.8.8')).status, 200) // different client
  }))

test('CLIENT_IP_HEADER keys the limiter on the header, falling back to the socket when absent', () =>
  withBroker({ rateLimitPerMinute: 1, clientIpHeader: 'cf-connecting-ip' }, async (base) => {
    const get = (ip) => fetch(`${base}/ebay/callback`, { headers: ip ? { 'Cf-Connecting-Ip': ip } : {} })
    assert.equal((await get('1.1.1.1')).status, 200)
    assert.equal((await get('1.1.1.1')).status, 429)
    assert.equal((await get('2.2.2.2')).status, 200) // a different real client is not limited
    assert.equal((await get()).status, 200)          // no header: the socket address
    assert.equal((await get()).status, 429)
  }))

test('logs method, path and status only: never a query string, code or token', () =>
  withBroker({}, async (base, logs) => {
    await fetch(`${base}/ebay/callback?code=LEAKME&state=${STATE}`)
    await post(`${base}/ebay/token`, { code: 'LEAKME-TOO' })
    await post(`${base}/ebay/refresh`, { refresh_token: 'LEAKME-RT' })
    await new Promise((r) => setTimeout(r, 50))
    assert.ok(logs.length >= 3)
    const all = logs.join('\n')
    for (const s of ['LEAKME', STATE, SECRET, 'AT', 'RT']) assert.ok(!all.includes(s), `log leaked ${s}`)
  }))

test('unknown routes and methods are 404', () =>
  withBroker({}, async (base) => {
    assert.equal((await fetch(`${base}/nope`)).status, 404)
    assert.equal((await fetch(`${base}/ebay/token`)).status, 404)
  }))

test('amazon is off unless fully configured, and a partial setup is refused', () => {
  const base = { EBAY_CLIENT_ID: 'a', EBAY_CLIENT_SECRET: 'b', EBAY_RUNAME: 'c' }
  assert.equal(loadConfig(base).amazon, null)
  assert.throws(() => loadConfig({ ...base, AMAZON_APP_ID: 'x' }), /AMAZON_CLIENT_ID, AMAZON_CLIENT_SECRET, AMAZON_REDIRECT_URI/)
  assert.throws(() => loadConfig({ ...base, ...AMAZON_ENV, AMAZON_SELLER_CENTRAL: 'https://evil.example' }), /Amazon HTTPS host/)
  assert.throws(() => loadConfig({ ...base, ...AMAZON_ENV, AMAZON_SELLER_CENTRAL: 'http://sellercentral.amazon.com' }), /Amazon HTTPS host/)
  const live = loadConfig({ ...base, ...AMAZON_ENV, AMAZON_DRAFT: '', AMAZON_SELLER_CENTRAL: 'https://sellercentral-europe.amazon.com/' }).amazon
  assert.equal(live.draft, false)
  assert.equal(live.consentBase, 'https://sellercentral-europe.amazon.com')
})

test('amazon routes are 404 when amazon is not configured', () =>
  withBroker({}, async (base) => {
    assert.equal((await fetch(`${base}/amazon/authorize?state=${ASTATE}`, { redirect: 'manual' })).status, 404)
    assert.equal((await post(`${base}/amazon/token`, { code: 'x' })).status, 404)
  }))

test('amazon authorize redirects to the consent page with the app id, state and beta flag, and remembers the state', () =>
  withAmazon({}, async (base) => {
    const r = await fetch(`${base}/amazon/authorize?state=${ASTATE}`, { redirect: 'manual' })
    assert.equal(r.status, 302)
    const loc = new URL(r.headers.get('location'))
    assert.equal(loc.origin + loc.pathname, 'https://sellercentral.amazon.com/apps/authorize/consent')
    assert.equal(loc.searchParams.get('application_id'), AMAZON_ENV.AMAZON_APP_ID)
    assert.equal(loc.searchParams.get('state'), ASTATE)
    assert.equal(loc.searchParams.get('version'), 'beta')
    assert.ok(!loc.toString().includes('amazon-secret-value'))
    const cookie = r.headers.get('set-cookie')
    assert.match(cookie, new RegExp(`^nisaba_amazon_state=${ASTATE}; `))
    assert.match(cookie, /HttpOnly/)
    assert.match(cookie, /Secure/)
    assert.match(cookie, /SameSite=Lax/)
    assert.match(cookie, /Max-Age=600/)
    assert.equal((await fetch(`${base}/amazon/authorize?state=short`, { redirect: 'manual' })).status, 400)
  }))

test('a published amazon app is sent without the beta flag', () =>
  withAmazon({ amazon: { ...loadConfig({ EBAY_CLIENT_ID: 'a', EBAY_CLIENT_SECRET: 'b', EBAY_RUNAME: 'c', ...AMAZON_ENV, AMAZON_DRAFT: '' }).amazon } }, async (base) => {
    const r = await fetch(`${base}/amazon/authorize?state=${ASTATE}`, { redirect: 'manual' })
    assert.equal(new URL(r.headers.get('location')).searchParams.get('version'), null)
  }))

test('amazon login hands the remembered state back to amazon, with the registered redirect URI', () =>
  withAmazon({}, async (base) => {
    const cb = encodeURIComponent('https://sellercentral.amazon.com/apps/authorize/confirm/amzn1.sp.solution.test-app')
    const r = await fetch(`${base}/amazon/login?amazon_callback_uri=${cb}&amazon_state=AMZSTATE&selling_partner_id=A1B2C3&version=beta`, {
      redirect: 'manual', headers: { Cookie: `other=1; nisaba_amazon_state=${ASTATE}` },
    })
    assert.equal(r.status, 302)
    const loc = new URL(r.headers.get('location'))
    assert.equal(loc.origin + loc.pathname, 'https://sellercentral.amazon.com/apps/authorize/confirm/amzn1.sp.solution.test-app')
    assert.equal(loc.searchParams.get('amazon_state'), 'AMZSTATE')
    assert.equal(loc.searchParams.get('state'), ASTATE)
    assert.equal(loc.searchParams.get('redirect_uri'), AMAZON_ENV.AMAZON_REDIRECT_URI)
    assert.equal(loc.searchParams.get('version'), 'beta')
  }))

test('amazon login is never an open redirect and needs the cookie from authorize', () =>
  withAmazon({}, async (base) => {
    const good = encodeURIComponent('https://sellercentral.amazon.com/apps/authorize/confirm/x')
    const cookie = { Cookie: `nisaba_amazon_state=${ASTATE}` }
    for (const [target, headers] of [
      [encodeURIComponent('https://evil.example/x'), cookie],
      [encodeURIComponent('https://amazon.com.evil.example/x'), cookie],
      [encodeURIComponent('https://amazon.evil.com/x'), cookie],
      [encodeURIComponent('http://sellercentral.amazon.com/x'), cookie],
      [encodeURIComponent('https://user:pw@sellercentral.amazon.com/x'), cookie],
      [encodeURIComponent('//evil.example'), cookie],
      ['', cookie],
      [good, {}],                                         // no cookie: not started from Nisaba
      [good, { Cookie: 'nisaba_amazon_state=short' }],    // malformed cookie
    ]) {
      const r = await fetch(`${base}/amazon/login?amazon_callback_uri=${target}&amazon_state=S`, { redirect: 'manual', headers })
      assert.equal(r.status, 400, target)
      assert.equal(r.headers.get('location'), null, target)
    }
  }))

test('amazon callback forwards the code to the app on loopback as `code`, and clears the cookie', () =>
  withAmazon({}, async (base) => {
    const r = await fetch(`${base}/amazon/callback?spapi_oauth_code=CODE9&state=${ASTATE}&selling_partner_id=A1B2C3&evil=1`, { redirect: 'manual' })
    assert.equal(r.status, 302)
    assert.equal(r.headers.get('location'), `http://127.0.0.1:54321/amazon/callback?code=CODE9&state=${ASTATE}&selling_partner_id=A1B2C3`)
    assert.match(r.headers.get('set-cookie'), /Max-Age=0/)
    assert.equal(r.headers.get('referrer-policy'), 'no-referrer')
  }))

test('amazon callback shows a static page unless the state names a valid loopback port', () =>
  withAmazon({}, async (base) => {
    for (const state of ['abcdefghijklmnop0123', '80.abcdefghijklmnopqrst', '70000.abcdefghijklmnopqrst', '//evil.example/.abcdefghijklmnopqrst']) {
      const r = await fetch(`${base}/amazon/callback?spapi_oauth_code=C&state=${encodeURIComponent(state)}`, { redirect: 'manual' })
      assert.equal(r.status, 200, state)
      assert.equal(r.headers.get('location'), null, state)
      assert.ok(!(await r.text()).includes('C&'))
    }
  }))

test('amazon token exchange sends the credentials and the registered redirect URI, and returns only the whitelisted fields', () =>
  withAmazon({ amazonTokenUrl: ebayUrl }, async (base) => {
    const r = await post(`${base}/amazon/token`, { code: 'good-code' })
    assert.equal(r.status, 200)
    const body = await r.json()
    assert.deepEqual(Object.keys(body).sort(), ['access_token', 'expires_in', 'refresh_token', 'token_type'])
    const call = ebayCalls[0]
    assert.equal(call.headers.authorization, undefined)
    assert.deepEqual(call.form, {
      grant_type: 'authorization_code', code: 'good-code', redirect_uri: AMAZON_ENV.AMAZON_REDIRECT_URI,
      client_id: AMAZON_ENV.AMAZON_CLIENT_ID, client_secret: AMAZON_ENV.AMAZON_CLIENT_SECRET,
    })
    assert.ok(!JSON.stringify(body).includes('amazon-secret-value'))
  }))

test('amazon refresh sends the refresh grant, and an amazon rejection carries only its error code', () =>
  withAmazon({ amazonTokenUrl: ebayUrl }, async (base) => {
    const r = await post(`${base}/amazon/refresh`, { refresh_token: 'my-rt' })
    assert.equal(r.status, 200)
    assert.deepEqual(ebayCalls[0].form, {
      grant_type: 'refresh_token', refresh_token: 'my-rt',
      client_id: AMAZON_ENV.AMAZON_CLIENT_ID, client_secret: AMAZON_ENV.AMAZON_CLIENT_SECRET,
    })
    const bad = await post(`${base}/amazon/refresh`, { refresh_token: 'bad-refresh' })
    assert.equal(bad.status, 502)
    assert.deepEqual(await bad.json(), { error: 'upstream_rejected', upstream_status: 400, upstream_error: 'invalid_grant', upstream_error_description: 'code expired' })
    assert.equal((await post(`${base}/amazon/token`, {})).status, 400)
    assert.equal((await post(`${base}/amazon/refresh`, { refresh_token: 5 })).status, 400)
  }))

test('amazon traffic is logged by path only: never a state, code, token or cookie', () =>
  withAmazon({ amazonTokenUrl: ebayUrl }, async (base, logs) => {
    await fetch(`${base}/amazon/authorize?state=${ASTATE}`, { redirect: 'manual' })
    await fetch(`${base}/amazon/callback?spapi_oauth_code=LEAKME&state=${ASTATE}`, { redirect: 'manual' })
    await post(`${base}/amazon/token`, { code: 'LEAKME-TOO' })
    await new Promise((r) => setTimeout(r, 50))
    const all = logs.join('\n')
    for (const needle of ['LEAKME', ASTATE, 'amazon-secret-value', 'nisaba_amazon_state']) assert.ok(!all.includes(needle), `log leaked ${needle}`)
    assert.ok(logs.length >= 3)
  }))
