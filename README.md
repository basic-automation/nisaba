# Nisaba

A desktop app for managing one product catalog and keeping it in sync across multiple
selling platforms.

Nisaba holds a local master catalog of products and variants, syncs inventory, pricing,
descriptions and photos to each connected marketplace, imports supplier catalogs through
sandboxed vendor plugins, and can sync between installs over Tor.

Built with Rust + Tauri 2 and a Nuxt 3 frontend.

## Features

- **Multi-platform sync** — eBay, Squarespace, XMR Bazaar and Amazon behind one
  `PlatformAdapter` trait, each declaring its own `PlatformCapabilities` so the UI and
  sync engine only attempt what a platform actually supports.
- **Inventory reconciliation** — a sync engine with quantity deltas, conflict resolution,
  and sale detection for platforms that use stock modes (unlimited / one-time) instead of
  numeric quantities.
- **Listing management** — descriptions, photos, and multi-tier pricing, edited locally
  and published per platform.
- **Vendor plugins** — TypeScript modules run in an embedded Deno runtime to import
  supplier catalogs (SKUs, variants, dealer pricing, inventory). Ships with a Rothco
  Wholesale plugin.
- **Multi-company + P2P sync** — optional peer-to-peer sync between installs over Tor
  onion services, with AES-GCM encrypted payloads and secrets held in the OS keyring.
- **Analytics** — per-product history and time-windowed analytics, charted in the UI.

## Repository layout

```
crates/
  core/                 Shared types, config, DB (turso), sync engine, conflict
                        resolution, analytics, crypto
  platform-ebay/        eBay adapter (OAuth)
  platform-squarespace/ Squarespace adapter (API key)
  platform-xmrbazaar/   XMR Bazaar adapter (HTML form scraping + cookies/CSRF)
  platform-amazon/      Amazon SP-API adapter (LWA OAuth)
  p2p/                  Tor onion service, peer client/server, company sync
  vendor-runtime/       Deno-based sandbox that executes vendor plugins
  tauri-app/            Tauri backend — ~96 commands exposed to the frontend
  service/  tui/        Not currently workspace members
frontend/               Nuxt 3 SPA (Tailwind + shadcn-vue)
plugins/
  rothco-wholesale/     Example/first-party vendor plugin (GraphQL v2 API)
migrations/             SQL migrations, applied in order
config.example.toml     Template for the runtime config
```

The Tauri layer is the only place the frontend talks to; commands are grouped by domain
in [`crates/tauri-app/src/commands`](crates/tauri-app/src/commands) (`products`,
`listings`, `sync`, `vendors`, `pricing`, `photos`, `analytics`, `company`, `auth`,
`config`, `logs`, `export_import`, `images`).

## Getting started

### Prerequisites

- Rust (stable) and the [Tauri 2 prerequisites](https://tauri.app/start/prerequisites/)
  for your OS
- Node.js 18+ and npm
- `cargo-tauri` CLI: `cargo install tauri-cli --version "^2"`

### Development

```bash
cd frontend && npm install
```

```bash
cargo tauri dev
```

`cargo tauri dev` starts the Nuxt dev server itself (port **3456**, chosen to avoid
conflicts) and then launches the desktop shell against it.

### A note on fonts

The UI is designed around four licensed Adobe Fonts (Clother, Clarendon Wide Sketch,
Pulpo Rust, Jubilat). Those `.woff2` files are **not redistributed** with this repository
and are gitignored. The app falls back to system font stacks and renders correctly
without them — just not on-brand.

If you have a license, drop your own `.woff2` files into `frontend/public/fonts/` using
the filenames listed in [`frontend/assets/css/fonts.css`](frontend/assets/css/fonts.css)
and they are picked up on the next build.

### Production build

```bash
cargo tauri build
```

This runs `npm run generate` in `frontend/` and bundles the static output into the app.

## Configuration

Config lives outside the repo, at the platform data directory:

- Windows: `%APPDATA%\nisaba\config.toml`
- Unix: `~/.config/nisaba/config.toml`

A default file is written on first run if none exists. Copy
[`config.example.toml`](config.example.toml) as a starting point — it covers the sync
schedule, database path, per-platform credentials, low-stock alert thresholds, and the
P2P company settings.

Products are **not** configured in TOML; they live in the database and are managed
through the UI. `config.toml` and `*.db` files are gitignored.

The database is SQLite-compatible (via `turso`), defaulting to `nisaba.db` in the data
directory. Schema changes go in `migrations/` as a new numbered file.

## Vendor plugins

A vendor plugin is a TypeScript module executed in a locked-down Deno runtime — no
ambient network or filesystem access, only what Nisaba injects as a `Nisaba` global:
`Nisaba.fetch`, `Nisaba.sleep`, `Nisaba.log.{trace,debug,info,warn,error}`, and
`Nisaba.emitBatch` for streaming listings back to the host as they are scraped.

A plugin exports two things:

```ts
export const metadata = {
  name: 'Rothco Wholesale',
  version: '2.5.0',
  description: '…',
  category: 'vendor',
  config_fields: [
    { key: 'api_token', label: 'API Token (Bearer)', required: true, secret: true },
  ],
}

export async function fetchListings(
  config: Record<string, string>
): Promise<any[]> { /* … */ }
```

Metadata is read without running `fetchListings`, so the app can show a plugin's config
fields before it is ever executed. `secret: true` fields are stored encrypted. Plugins
may be single-file or multi-file; see [`plugins/rothco-wholesale`](plugins/rothco-wholesale)
for a working example that pages a GraphQL catalog and emits batches of listings.

## P2P company sync

When `[company].enabled` is set, an install publishes a Tor onion service and syncs
catalog state with peers on an interval. Payloads are encrypted with a shared company
secret (AES-GCM + HKDF); the secret and platform tokens are stored in the OS keyring
rather than in config. See [`crates/p2p`](crates/p2p/src).

## Status

Pre-1.0 and under active development — releases are marked pre-release until the sync
engine has been exercised against all four platforms in production.

[`ROADMAP.md`](ROADMAP.md) is the single source of truth for planned work: a phase-ordered
checkbox queue covering test coverage, the platform capability gaps, the plugin platform,
P2P, and the release pipeline. Known gaps worth calling out up front:

- No auto-update yet — the Tauri updater ships with an empty signing key.
- Photo upload is unimplemented on every platform.
- Test coverage is limited to `crates/core`; the adapters, sync engine, P2P layer and
  plugin runtime are untested.
- `crates/service` (headless sync daemon) and `crates/tui` are tracked in git but excluded
  from the Cargo workspace, so they are not built or tested.

## License

MIT
