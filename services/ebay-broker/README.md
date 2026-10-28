# Nisaba eBay OAuth broker

A small stateless service that holds Nisaba's eBay client secret so a user can connect
eBay with one click. eBay's authorize URL needs the client ID, and the code exchange and
every token refresh need the client secret. The desktop app is open source, so the secret
cannot live in it. This service keeps it instead.

It stores nothing. It logs only `METHOD /path status duration`: never a query string,
code, state or token. It has no dependencies (Node 22+ standard library only).

## Endpoints

| | |
|---|---|
| `GET /ebay/authorize?state=…` | 302 to eBay's consent page. `state` is 16-256 URL-safe characters, chosen and checked by the app. |
| `GET /ebay/callback` | The page eBay redirects to (the RuName's accepted URL). If `state` starts with `<port>.` (a sign-in in the user's own browser) it forwards `code`, `state` and any decline error to the app's one-shot listener at `http://127.0.0.1:<port>/ebay/callback`, and only ever to loopback on ports 1024-65535. Otherwise it shows a static page and the app reads the URL. |
| `POST /ebay/token` `{"code": "…"}` | Exchanges the code for tokens. |
| `POST /ebay/refresh` `{"refresh_token": "…"}` | Refreshes an access token. |
| `GET /healthz` | Liveness. |

Token responses contain only `access_token`, `expires_in`, `token_type`, `refresh_token`
and `refresh_token_expires_in`. Requests are limited to 4 KiB, JSON only, and rate
limited per client (default 30 per minute).

## Configuration

| Variable | |
|---|---|
| `EBAY_CLIENT_ID` | Required. The production (or sandbox) keyset's App ID. |
| `EBAY_CLIENT_SECRET` | Required. The Cert ID. Never commit it. |
| `EBAY_RUNAME` | Required. The RuName, whose accepted URL is `https://<your-host>/ebay/callback`. |
| `EBAY_ENV` | `production` (default) or `sandbox`. |
| `PORT` | Default `8080`. |
| `TRUST_PROXY` | Set to `1` behind exactly one reverse proxy, so the rate limiter uses `X-Forwarded-For`. |
| `CLIENT_IP_HEADER` | Header carrying the real client IP behind a CDN or tunnel, e.g. `Cf-Connecting-Ip` for a Cloudflare tunnel. Without it every user behind the tunnel shares one rate-limit bucket. Set it only when the broker is reachable solely through that path, because any caller could forge the header. |
| `RATE_LIMIT_PER_MIN` | Default `30`. |

## Run it

TLS must terminate in front of the container: the RuName's accepted URL has to be HTTPS
(Caddy, nginx, Traefik or a tunnel all work). Keep the secret in an env file that is not
in git, readable only by you:

```bash
docker build -t nisaba-ebay-broker services/ebay-broker
docker run -d --name nisaba-ebay-broker --restart unless-stopped \
  --read-only --cap-drop ALL --security-opt no-new-privileges \
  -p 127.0.0.1:8080:8080 --env-file /etc/nisaba-broker.env nisaba-ebay-broker
```

with `/etc/nisaba-broker.env` holding `EBAY_CLIENT_ID=`, `EBAY_CLIENT_SECRET=`,
`EBAY_RUNAME=` and `TRUST_PROXY=1`.

## Registering Nisaba with eBay (once)

1. Join the eBay Developers Program and create a **Production** keyset.
2. Create a RuName for that keyset with the accepted URL `https://<your-host>/ebay/callback`.
3. Before the first production call, subscribe to eBay's marketplace account deletion
   notifications, or apply for the exemption for apps that store no eBay user data. This
   service stores none, which is the case for the exemption; confirm with eBay.

## Tests

```bash
cd services/ebay-broker && node --test
```
