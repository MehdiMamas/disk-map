# Contributing to Disk Map

Thanks for helping. Bug reports, ideas and pull requests are all welcome.

## Before you start

- Look through the [open issues](https://github.com/MehdiMamas/disk-map/issues); issues labelled `good first issue` are a good place to begin.
- For a larger change, open an issue or a [discussion](https://github.com/MehdiMamas/disk-map/discussions) first so we can agree on the approach.

## Set up

You need Rust, Node 22, pnpm 10 and the [Tauri prerequisites](https://tauri.app/start/prerequisites/).

```
pnpm install
pnpm tauri dev
```

## Check your change

```
pnpm build                    # typecheck and frontend build
cd src-tauri && cargo test    # scanner and hog-rule tests
```

CI runs the same commands on Windows and macOS for every pull request.

## Pull requests

- Keep each pull request to one change, and describe what it changes and why.
- Add or update a test when you change the scanner or the hog rules (`src-tauri/src/hogs.rs`).
- After an action such as trashing or rescanning, open views should update in place.
- Scans stay local: do not add network calls other than the update check.

By contributing you agree that your work is released under the [MIT license](LICENSE).
