# Phase 0 — Public release readiness

[← Back to the checklist](../../ROADMAP.md#phase-0--public-release-readiness)

## `README.md`

What Nisaba is, architecture, dev/build steps, config, plugin contract

## `ROADMAP.md`

This file

## Stop redistributing licensed Adobe Fonts

Gitignore `frontend/public/fonts/*.woff2` and fall back to system stacks in `tailwind.config.ts` + `app.vue`

## CI

`cargo fmt --check`, `build`, `test`, `clippy -D warnings` on Linux + Windows, plus a `npm run generate` frontend build

## CI's format gate is green

The workspace was run through rustfmt in one mechanical pass; `cargo fmt --all -- --check` now exits clean, so the build, test and clippy steps behind it finally run.

## The workspace could not build on stable at all

`onyums 0.2.5` — the Tor onion-service crate behind `crates/p2p` — carries `#![feature(addr_parse_ascii)]`, so `cargo build` on the toolchain CI installs died with `E0554` before anything else ran. The format check was failing first, so CI had never reached the build step to report it. Fixed by moving to `onyums 0.3.1` (same `serve`/`get_onion_name` API, no nightly feature) together with `artiqwest 0.3.0` → `0.4.1`; the pair has to move together because both embed arti and their versions must agree.

## Take `onyums 0.5.0` once `artiqwest` catches up

0.5 drops the polling hack in `start_onion_service` — its builder returns a handle with `onion_address()`, `ready()`, `wait_until_settled()` and `shutdown()` instead of a 120s poll on a global `get_onion_name()` — and its crypto is pure Rust (no vendored C at all). Blocked today: `onyums 0.5.0` needs `arti-client 0.46`, `artiqwest 0.4.1` (the newest) pins `0.43`, and cargo cannot resolve both. Both crates are `basic-automation`'s, so the unblock is an `artiqwest` release on arti 0.46. **On hold:** Phase 5 moves company sync to iroh, which removes `onyums` and `artiqwest` outright unless Tor is kept as an opt-in transport — do not spend an `artiqwest` release on this until that is decided.

## Clear the clippy backlog behind CI's `-D warnings` gate

With fmt and the build green it was finally measurable, and there is no backlog: `cargo clippy --workspace --all-targets -- -D warnings` exits 0 across all eight member crates including `nisaba-tauri`. The gate should hold from here.

## `crates/core`'s `turso 0.5` pulled in `aegis 0.9.7`, whose vendored C (`libaegis`) would not compile

The cause was not `-mtune=native`: libaegis guards its AVX-512 sources with `target("aes,vaes,avx512f,evex512")`, and both Clang 22 and GCC 16 dropped the `evex512` token. Clang only *warns* and then discards the whole `target` attribute, which disables `avx512f` and turns every intrinsic in the file into a hard error. Fixed by `cargo update -p aegis` (0.9.7 → 0.9.19), which vendors a libaegis that no longer uses the token. Clang 22 removal: https://releases.llvm.org/22.1.0/tools/clang/docs/ReleaseNotes.html GCC 16 removal: https://github.com/google/highway/issues/2577

## Normalize line endings

The tree is now LF in both the index and the worktree (`git ls-files --eol` reports no CRLF), and `.gitattributes` (`* text=auto eol=lf`, plus binary markers for images/fonts) keeps a Windows checkout from reintroducing it.

## Decide `rust-toolchain.toml`

Pinned to `stable` with `rustfmt` + `clippy`, matching CI, so a dev host defaulting to nightly cannot drift from what CI verifies

## `SECURITY.md` with a disclosure path

GitHub private vulnerability reporting, plus what counts as a vulnerability (credential handling, the plugin sandbox, the P2P layer, adapter handling of untrusted marketplace responses) and what does not

## Enable GitHub private vulnerability reporting on the repo (Settings → Code security)

`SECURITY.md` points contributors at it and it is off by default

## CI's Rust job could not build `nisaba-tauri`

`cargo build` does not run Tauri's `beforeBuildCommand` (only `cargo tauri build` does), so `tauri::generate_context!()` panicked on the missing `frontendDist` (`frontend/.output/public`, which is gitignored). The Rust job now runs `npm ci && npm run generate` before cargo.

## Screenshots in the README

The UI is the product and there is currently nothing to look at [N-12.5: `README.md` embeds 5 view screenshots captured from the real app on a fixture install, committed to `docs/images/` by the CI UI smoke job.]

## README accuracy

It said "Node.js 18+" (`README.md:63`), but Nuxt 3.21 and Vite 7 need `^20.19.0 || >=22.12.0`; add an `engines` field to `frontend/package.json` so npm enforces the requirement. It says analytics are "charted in the UI", but there are no charts: `inventory.vue` prints `history.length`. It also omits that `./config.toml` in the working directory overrides the data-dir config (`core/src/config.rs` ~230–235).

## `cleanup.ps1` and `parse_har.py` were purged from history but still sit untracked in at least one checkout and are not gitignored, so a `git add -A` re-adds them

Ignore them, or delete the local copies.

## The frontend typecheck has never run

`ci.yml` runs `npx nuxt typecheck` with `continue-on-error: true`, and `vue-tsc` is not a devDependency. On master (run 36194520101) `npx` fetched an incompatible `vue-tsc`, which crashed with `ERR_PACKAGE_PATH_NOT_EXPORTED './lib/tsc'`. Pin `vue-tsc` as a devDependency, fix the errors, and drop `continue-on-error`. There is also no frontend lint or test step.
