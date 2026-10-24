# Disk Map

A small desktop app (Tauri 2, Rust + TypeScript) that shows where your disk space went. macOS lumps caches, Xcode data, Docker and simulators under "System Data"; Disk Map scans a folder (default: your home) and shows a treemap and a sorted list, tags known space hogs, and lets you reveal or trash them.

## Features
- Parallel scan (jwalk). Physical size on disk (`blocks * 512` on macOS/Linux, file size on Windows). Symlinks are never followed, other filesystems are skipped, hard links count once, unreadable folders are counted and reported.
- Squarified treemap plus a sorted list with size bars, breadcrumb and back button. Children under 0.5% of their parent are grouped into "(other)".
- Known hogs (Xcode DerivedData, CoreSimulator, Docker, node_modules, Library/Caches and AppData caches, .gradle, .npm, pnpm store, Cargo `target`) get a tag with a plain note and a "safe to clear?" value. Rules live in `src-tauri/src/hogs.rs`.
- Reveal in Finder/Explorer. Move to Trash after a confirm dialog (system Trash/Recycle Bin via the `trash` crate; nothing is ever deleted permanently).
- macOS: if folders are blocked, a hint explains how to grant Full Disk Access.

## Design choice: drill-down
The scan keeps the full tree in memory in the Rust process and drill-down only re-prunes it, so clicking into a folder is instant and costs no disk I/O. To keep memory bounded, directories are all kept but files under 1 MiB are folded into one "(small files)" entry per directory (a 600k-file home scan stays small). The trade-off: the view can be stale until you rescan, and a rescan is the only way to see new files. Trashing an item updates the in-memory tree directly.

## Run
Prerequisites: Rust, Node, pnpm, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/).

```
pnpm install
pnpm tauri dev
```

## Build
```
pnpm tauri build            # .app/.dmg (macOS), .msi/.exe (Windows) in src-tauri/target/release/bundle
pnpm tauri build --no-bundle  # just the binary
```

## Test
```
cd src-tauri && cargo test          # scanner and hog-rule unit tests
cargo run --release --bin scan-cli -- ~/Projects   # top folders and scan time without the UI
pnpm build                          # typecheck + frontend build
```

## macOS signing and notarization

`scripts/release-macos.sh` builds, signs with the Developer ID Application identity (hardened runtime is Tauri's default; no entitlements needed), notarizes and staples. The identity comes from `APPLE_SIGNING_IDENTITY` (default `Developer ID Application: El Mehdi Mamas (7LPSQ8W7G8)`), so `tauri.conf.json` stays portable for CI and other platforms. One-time notarization setup:

    xcrun notarytool store-credentials disk-map-notary --apple-id <Apple ID> --team-id 7LPSQ8W7G8

Without that profile the script stops after signing.
