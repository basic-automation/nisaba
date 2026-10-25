# Phase 7 — Frontend and UX

[← Back to the checklist](../../ROADMAP.md#phase-7--frontend-and-ux)

## Nuxt 3 SPA with Tailwind and shadcn-vue-style components (`components/ui/`, on `radix-vue`), covering products, listings, vendors, inventory and companies

The `shadcn-nuxt` module is a dependency but is not registered in `nuxt.config.ts`.

## Empty and error states

The pages assume data exists. Named cases:

- Creating a product (`products/index.vue` ~254–264) has no validation and no
  `catch`; a duplicate SKU hits the unique index and the user sees nothing.
- The dashboard's `Promise.all` (`pages/index.vue` ~14–22) has no `catch`.
- "Sync now" (`StatusBar.vue` ~40) gives no progress or error feedback.
- `inventory.vue` shows a load error as "No analytics data yet".

[N-11.3: create-product dialog validates (name/SKU required) and shows
backend errors inline incl. duplicate SKU; dashboard wraps the `Promise.all`
in try/catch with an error banner + retry; Sync Now shows a spinner and
notifies on failure; inventory distinguishes load errors (with retry)
from the genuine empty state.]

## Confirm destructive actions, using one mechanism

Unlinking a mapping, removing a peer and uninstalling a plugin act immediately. Import (`products/index.vue` ~309) does `INSERT OR REPLACE` over live rows with no preview. The confirms that do exist use three different mechanisms: `window.confirm`, the dialog plugin, and an in-app modal. [N-11.3: one shared `<ConfirmDialog>` component (+ `useConfirmDialog`) now covers unlinking a mapping, removing a peer, uninstalling/removing a plugin, deleting a product/variant, leaving a company and restoring a backup; import is preview-then-apply (`preview_import_data`/`apply_import_data` commands) showing new/replaced/skipped counts and the affected SKUs before anything is written.]

## Sync status goes stale

Each `useSync()` call creates its own `isRunning`, starting at `true` (`useSync.ts:7`). Nothing refetches it on a company switch, so the dashboard and sidebar disagree after a pause. Make it one shared store, updated from engine events.

## N+1 IPC calls

The dashboard, listings and products pages call `get_mappings`, and sometimes `list_variants`, once per product, sequentially, on every change. Add batch commands.

## Type drift between TypeScript and Rust

- `SyncEngineEvent` lacks `DryRunChange`;
- `MergeSummary` lacks `vendor_plugins_updated`, `variants_updated` and
  `rejected_timestamps`;
- `ProductVariant` lacks `source_plugin_id` and `source_vendor_item_id`.

`company.vue` also ignores the merge summary, so rejected peer rows are never shown.
Generate the types from Rust (`ts-rs` or `specta`) instead of maintaining them by hand.

## Dead code

- `usePricing.ts` and `useTauriCommand.ts` are never imported, so `update_price` and
  `get_pricing_history` cannot be reached from the UI.
- Five registered commands have no caller: `get_cached_images`, `get_ebay_auth_url`,
  `get_product_cache_freshness`, `list_company_peers`, `update_variant_quantity`.
- Several npm dependencies are never imported: `chart.js`, `vue-chartjs`, `diff`,
  `lucide-vue-next`, `class-variance-authority`, `clsx`, `tailwind-merge`,
  `@tauri-apps/plugin-shell`.

Wire up or remove each one.

## The per-platform sales breakdown is always empty

`analytics.rs` (~76–91) groups by `source_platform`, but every sync event is written with `"sync_engine"` there (`sync_engine.rs` ~465, ~497, ~565, ~597), and `from_str_loose` drops those rows, so `inventory.vue` always shows "—". N-11.8: the engine now records the real platform on push events, and `analytics.rs`/`sync_engine.rs` carry tests covering the grouping for engine-written events.

## Startup failures are silent

`main.rs` (~72, ~84–86) calls `expect()` on loading the config and opening the database, and a Windows release build has no console, so the app just exits. Show a native error dialog and write a log file.

## Accessibility pass

Keyboard navigation, focus rings, and contrast against the Palenight surfaces

## `frontend/public/guide/` is an empty directory with a `.gitkeep`

Either ship the in-app guide it was made for or remove it [N-12.1: shipped the in-app guide; `.gitkeep` removed.]

## The window is `"decorations": false`

Confirm the custom chrome behaves on Linux/Wayland and macOS, not just Windows

## A WebDriver harness that drives the real app

`scripts/ui-smoke.py` plus the `ui_fixture` example that seeds a throwaway install. It runs the app from the fixture directory (the app reads `./config.toml` first) inside `unshare -rn`, so it has no network at all and cannot touch live data, and drives it through tauri-driver + WebKitWebDriver. First green run 2026-09-26 on Hyprland: 6 checks on the marketplace's network-access badges and the per-plugin time limit. WebKitGTK needs `WEBKIT_DISABLE_DMABUF_RENDERER=1` there ("Error 71 (Protocol error) dispatching to Wayland display" otherwise).

## Grow the UI smoke test beyond the marketplace

The product, listings and vendor pages, and the empty states above. (Whether it can run in CI is settled: c99c566 added the job.)

## Fix the CI UI smoke job, which fails on PR #2

(run 36223531255). `scripts/ui-smoke.py` builds the app path from a relative `--target-dir target` without `abspath`. Its existence check passes from the repo root, but tauri-driver is then started with `cwd` set to the temp fixture dir, so WebKitWebDriver cannot find `target/debug/nisaba-tauri`. Locally it passes because the README passes an absolute path. Fix: `os.path.abspath(args.target_dir)`.
