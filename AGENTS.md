# AGENTS.md

## What it is
Disk Map: a Tauri 2 desktop app (Rust backend, TypeScript/Vite frontend) for macOS and Windows. It scans a folder, shows a treemap and sorted list, tags known space hogs, and can reveal or trash items. Scans stay local; the only network call is the update check. See `README.md` for features.

## Setup
- Prerequisites: Rust, Node, pnpm (CI uses pnpm 10, Node 22), and the Tauri prerequisites.
- `pnpm install`

## Commands
- `pnpm tauri dev`: run the app.
- `pnpm build`: `tsc --noEmit` + `vite build` (typecheck and frontend build).
- `cd src-tauri && cargo test`: scanner and hog-rule unit tests.
- `cd src-tauri && cargo run --release --example scan_cli -- <folder>`: scan without the UI.
- `pnpm tauri build` (installers in `src-tauri/target/release/bundle`) or `pnpm tauri build --no-bundle` (binary only).
- `scripts/release-macos.sh`: universal macOS build, sign and notarize (needs `APPLE_SIGNING_IDENTITY` and a notarytool profile; see README).

## Where things live
- `src/`: frontend (`main.ts`, `treemap.ts`, `style.css`); `index.html` at root.
- `src-tauri/src/`: `scanner.rs` (scan), `hogs.rs` (known-hog rules), `apps.rs` (macOS app uninstall), `permissions.rs`, `update.rs`, `lib.rs`, `main.rs`.
- `src-tauri/tauri.conf.json`, `capabilities/`, `Info.plist`: Tauri config.
- `.github/workflows/`: `ci.yml` (PRs and main: build, cargo test, tauri build on Windows and macOS), `release.yml` (tags `v*`).

## Conventions
- Follow `CLAUDE.md` for release checks and UI rules (update open views in place after actions, cache slow lists, look up dialog elements inside their dialog).

## Gotchas
- The scan tree lives in the Rust process; drill-down only re-prunes it, so views can be stale until a rescan.
- Files under 1 MiB are folded into a "(small files)" entry per directory.
- Under `pnpm tauri dev`, macOS Full Disk Access belongs to the terminal app, not Disk Map.
