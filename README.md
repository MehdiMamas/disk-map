<p align="center"><img src="src-tauri/icons/128x128@2x.png" width="128" alt="Disk Map icon"></p>

# Disk Map

See where your disk space went: a fast treemap and sorted list of any folder, with known space hogs tagged.

macOS lumps caches, Xcode data, Docker and simulators under "System Data". Disk Map scans a folder (default: your home), shows what is big, and lets you reveal or trash it.

## Download

Get the latest installer from the [Releases page](https://github.com/MehdiMamas/disk-map/releases/latest).

- **macOS** (10.15 or later): download the `.dmg` (universal: Apple silicon and Intel). It is signed and notarized by Apple, so it opens without warnings. Drag Disk Map to Applications.
- **Windows 10/11**: download the `-setup.exe` (an `.msi` is also available). It installs WebView2 if it is missing. The installer is not code-signed, so SmartScreen may warn: click "More info", then "Run anyway".

## First launch

- **macOS**: Disk Map needs Full Disk Access to read protected folders. On first launch it shows a hint; open System Settings, Privacy & Security, Full Disk Access, switch Disk Map on, then restart the app.
- **Windows**: run Disk Map as administrator if you want protected folders included.

## Features
- Parallel scan (jwalk). Physical size on disk (`blocks * 512` on macOS/Linux, file size on Windows). Symlinks are never followed, other filesystems are skipped, hard links count once, unreadable folders are counted and reported.
- Squarified treemap plus a sorted list with size bars, breadcrumb and back button. Children under 0.5% of their parent are grouped into "(other)".
- Known hogs (Xcode DerivedData, CoreSimulator, Docker, node_modules, Library/Caches and AppData caches, .gradle, .npm, pnpm store, Cargo `target`) get a tag with a plain note and a "safe to clear?" value. Rules live in `src-tauri/src/hogs.rs`.
- Reveal in Finder/Explorer. Move to Trash after a confirm dialog (system Trash/Recycle Bin via the `trash` crate; nothing is ever deleted permanently).
- macOS: uninstall an app completely. "Uninstall app…" on any .app (or the Apps list in the header) shows the bundle plus matching leftovers in ~/Library as a checklist and moves what you check to the Trash; shared-looking home dot-folders like `~/.cursor` start unchecked. Apple apps and running apps are refused.
- Update notifications: a few seconds after launch (and on "Check for updates") the app asks GitHub for the latest release and offers its download when it is newer.
- macOS: if folders are blocked, a hint explains how to grant Full Disk Access.

- **Optimize tab**: a side menu with Overview (health checks and the top suggestions), Profiles, Performance, Privacy, Startup, Apps and Processes. On Windows, Profiles (Work, Gaming, Balanced) set a group of switches at once after a confirm dialog that lists every change; Performance and Privacy have single switches (power plan, visual effects, Game Mode, background apps, mouse acceleration, SysMain, Windows Search, advertising ID, tips and ads, Copilot, telemetry and more) that show the live Windows setting. Before any change, the old value is saved to a small journal on your machine, so each changed switch has an Undo button and "Restore all changes" puts everything back. Switches that need administrator rights are greyed out unless Disk Map runs as administrator. It never deletes files, ends processes, or touches Defender, Windows Update, the firewall, UAC or the hosts file; services only change their start type, and nothing is stopped. On macOS the same tab works with per-user settings written through the `defaults` command (Performance, Privacy and Interface switches, plus the Gaming, Work and Balanced profiles). Turning a switch off deletes the key again, so the system default returns, and every change has an Undo. Nothing is restarted for you: switches that only show after a Dock or Finder restart say so, and a "Relaunch Dock and Finder to apply" button does it when you click it. Startup lists your user and system launch agents and daemons (system ones are read-only and marked "needs admin"; user agents can be switched off and on) and your Login Items (read-only). Network can list adapters, set and reset DNS and ping a host; flushing the DNS cache needs admin, so the exact Terminal command is shown instead. Hardware shows CPU, memory, disks, graphics and macOS version. Before the first change Disk Map asks macOS for a local Time Machine snapshot. It never touches Gatekeeper, SIP, the firewall, FileVault, software updates, Spotlight indexing or power settings. The Mac side has not been tested on a real Mac yet. Thresholds are rough rules of thumb, not measurements.

## Privacy

Scans stay on your machine and nothing about your files is ever sent anywhere. The only network request is the update check: an anonymous GET of the public GitHub latest-release page, with no data about you or your disks.

## Design choice: drill-down
The scan keeps the full tree in memory in the Rust process and drill-down only re-prunes it, so clicking into a folder is instant and costs no disk I/O. To keep memory bounded, directories are all kept but files under 1 MiB are folded into one "(small files)" entry per directory (a 600k-file home scan stays small). The trade-off: the view can be stale until you rescan, and a rescan is the only way to see new files. Trashing an item updates the in-memory tree directly.

## Development

### Run
Prerequisites: Rust, Node, pnpm, and the [Tauri prerequisites](https://tauri.app/start/prerequisites/).

```
pnpm install
pnpm tauri dev
```

### Build
```
pnpm tauri build            # .app/.dmg (macOS), .msi/.exe (Windows) in src-tauri/target/release/bundle
pnpm tauri build --no-bundle  # just the binary
```

### Test
```
cd src-tauri && cargo test          # scanner and hog-rule unit tests
cargo run --release --example scan_cli -- ~/Projects   # top folders and scan time without the UI
pnpm build                          # typecheck + frontend build
```

### macOS signing and notarization (maintainer)

`scripts/release-macos.sh` builds a universal binary, signs with the Developer ID Application identity (hardened runtime is Tauri's default; no entitlements needed), notarizes and staples. The identity comes from `APPLE_SIGNING_IDENTITY` (default `Developer ID Application: El Mehdi Mamas (7LPSQ8W7G8)`), so `tauri.conf.json` stays portable for CI and other platforms. One-time notarization setup:

    xcrun notarytool store-credentials disk-map-notary --apple-id <Apple ID> --team-id 7LPSQ8W7G8

Without that profile the script stops after signing.

## Permissions notes

- macOS needs only Full Disk Access; the app walks you through it on first launch. Under `pnpm tauri dev` the permission belongs to the terminal app, not Disk Map.
- Move to Trash uses NSFileManager, so there is no Automation prompt, but Finder's "Put Back" is not available for those items.
- Windows: run as administrator to include protected folders.

## License

[MIT](LICENSE)

The Optimize tab reads local system information (startup entries, installed apps, memory, disks, processes) to show it on screen. None of it leaves your machine, including the change journal.
