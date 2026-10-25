# Phase 6 — Release and distribution

[← Back to the checklist](../../ROADMAP.md#phase-6--release-and-distribution)

## Signing key for the Tauri updater

`tauri.conf.json` ships an empty `pubkey`, so auto-update cannot work at all until this exists. Concretely: `tauri signer generate` produces the pair, the *contents* of the public key (not a path) go in `pubkey`, and the release workflow needs `TAURI_SIGNING_PRIVATE_KEY` (plus `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` if set) exported as real environment variables. https://v2.tauri.app/plugin/updater/

## Pin `createUpdaterArtifacts` to the v2 format rather than `"v1Compatible"` when the release workflow lands

Tauri documents the setting as removed in v3. https://v2.tauri.app/plugin/updater/

## Track the Tauri 3 alphas before they land on a stable line

`v3.0.0-alpha.2` (2026-09-21) renames plugin APIs (`js_init_script` → `initialization_script`, `Plugin::extend_api` → `Plugin::run_invoke_handler`) and removes `Invoke::state`/`state_ref`. Nisaba stays on Tauri 2 until 3 is stable; this is the migration checklist's first entry. https://github.com/tauri-apps/tauri/releases

## `.updater/latest.json` published from a real release workflow

The Tauri CLI does not write `latest.json`, so the workflow must assemble it from the `.sig` files. Alternatively, point the endpoint at a release asset instead of a file on `master`. Today the endpoint (`tauri.conf.json` ~47–49) is a raw file on `master`, so any commit to that file is an update push.

## Correction to the item above about pinning `createUpdaterArtifacts`

The key is not in `tauri.conf.json` at all, so nothing produces `.sig` or `.app.tar.gz` files, and the globs in `release.yml` for them can never match. Add the key (v2 format) together with the signing key.

## The updater checks a GitHub raw URL at launch and every 6 hours (`useUpdater.ts` ~61–65), outside Tor and with no opt-out

Add a setting, and record the choice in the Sync v2 privacy decision.

## CI runners

Every run carries Node 20 deprecation annotations for the actions, and `ubuntu-latest` moves to Ubuntu 26 from 2026-10-19. Bump the action versions, and check that the apt list (`libappindicator3-dev`, the WebKitGTK packages) still resolves on the new image, or pin `ubuntu-24.04`.

## CI has no macOS leg, although every release ships a universal macOS bundle, and `release.yml` runs no tests before publishing

Add `macos-latest` to `ci.yml`, or a test job that `publish` depends on.

## A `release.yml` that builds **Linux, Windows and macOS** bundles and attaches them to the tag's GitHub release

One matrix, `fail-fast: false`, Linux on `ubuntu-22.04` so bundles link against an older glibc, macOS as a `universal-apple-darwin` binary covering Apple Silicon and Intel. `workflow_dispatch` with `publish` off builds all three without publishing, so the matrix can be checked without cutting a release. Owner directive 2026-09-25: every release covers all three platforms, and a release missing one is a failed release rather than a partial one.

## Prove the release matrix green

Build-only dispatch run 36218681088 (2026-09-26, on `master` at `5fd136d`) passed on all three legs and uploaded every bundle: Linux `.deb`/`.rpm`/`.AppImage`, Windows `.msi` and NSIS `.exe`, macOS universal `.dmg` — the first bundles ever built for this project. They are unsigned (no updater key, no Apple identity), so there are no `.sig` files. https://github.com/basic-automation/nisaba/actions/runs/36218681088

## The tag-push path of `release.yml` has never run

As first written it could not have worked — each leg ran `gh release upload` against a release nothing created — and it uploaded per leg, so one failed leg would have left a published two-platform release. Now each leg uploads into a *draft* (the first creates it, pre-release for `v0.*`) and a final `publish` job, which only runs when every leg succeeded, checks the draft holds an AppImage, a `.deb`, an `.msi` and a `.dmg` before publishing it. The shell was syntax-checked and the asset check dry-run against the real bundle names; the path itself is unverified until the first `v*` tag.

## `bundle` in `tauri.conf.json` had **no `active` flag, which defaults to `false`**

`tauri-utils`' `BundleConfig::active` is `#[serde(default)]` on a `bool`, so `cargo tauri build` produced only the executable and never a single bundle, on any platform. Found by the first release-matrix run: the build step passed and the upload found nothing. Now `"active": true` with an explicit target list (`deb`, `rpm`, `appimage`, `app`, `dmg`, `msi`, `nsis`) instead of bundler defaults.

## macOS code signing and notarization

Without an Apple Developer identity the `.dmg` is unsigned and Gatekeeper blocks it on first open for most users. Decide whether to sign, or document the right-click-Open workaround in the README.

## Decide whether `crates/service` (headless sync daemon) and `crates/tui` ship as real products

They were tracked in git but excluded from `[workspace] members`, so they were not built, not tested and not covered by CI. **Decided 2026-10-22 (N-10.3): deleted** — restoring them was close to a rewrite (neither had changed since `1f3cdc7`; both read platform credentials from `config.toml` and use the app database where the app now uses per-company encrypted config; neither supports Amazon, plugins or P2P). A headless daemon can return post-1.0 designed against the current config model. `crates/service` and `crates/tui` were removed from the repo, including all stale references. (This also retires Phase 4's "dry run in the headless daemon" as a 1.0 item; dry-run remains engine-supported for the UI.)

## Version consistency

`Cargo.toml`, `tauri.conf.json` and any updater manifest must be bumped together; add a check that fails CI when they disagree (N-10.2, 2026-10-21: `scripts/check-version-consistency.py` runs as its own `version-consistency` CI job, comparing `Cargo.toml [workspace.package]`, `crates/tauri-app/tauri.conf.json` and `frontend/package.json`; the missing `version` field was added to `frontend/package.json`)
