#!/usr/bin/env bash
# Build a universal binary, sign (Developer ID), notarize and staple the macOS app.
set -euo pipefail
cd "$(dirname "$0")/.."

export APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:-Developer ID Application: El Mehdi Mamas (7LPSQ8W7G8)}"
PROFILE=disk-map-notary
BUNDLE=src-tauri/target/universal-apple-darwin/release/bundle

rustup target add aarch64-apple-darwin x86_64-apple-darwin
pnpm tauri build --target universal-apple-darwin

APP=$(ls -d "$BUNDLE"/macos/*.app | head -1)
DMG=$(ls "$BUNDLE"/dmg/*.dmg | head -1)

codesign --verify --deep --strict --verbose=2 "$APP"
codesign -dv --verbose=4 "$APP" 2>&1 | grep -E 'Authority|flags|TeamIdentifier'
echo "Signed: $APP"

if ! xcrun notarytool history --keychain-profile "$PROFILE" >/dev/null 2>&1; then
  cat >&2 <<M
Signed build is done, but notarization is skipped: keychain profile '$PROFILE' is missing.
Store it once (needs an app-specific password from account.apple.com), then rerun:
  xcrun notarytool store-credentials $PROFILE --apple-id <your Apple ID> --team-id 7LPSQ8W7G8
M
  exit 2
fi

xcrun notarytool submit "$DMG" --keychain-profile "$PROFILE" --wait
xcrun stapler staple "$APP"
xcrun stapler staple "$DMG"
spctl -a -vv -t exec "$APP"
