@AGENTS.md


## Before a release
- Scan `/` on a real Mac with the built app and check the total against `df` used space (System + Data volumes, within a few %); check every top-level folder appears once.
- With Full Disk Access granted, no FDA banner or onboarding card may show.

## UI
- Never make the user wait again for something they already waited for: after an action (uninstall, delete), update the open view in place and keep slow lists (apps, scans) cached until the user asks to refresh.
- Look up a dialog's elements inside that dialog, never by a page-wide id, since several dialogs can be open at once.
