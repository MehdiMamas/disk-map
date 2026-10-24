#!/usr/bin/env bash
# Build a universal binary, sign (Developer ID), notarize and staple the macOS app.
set -euo pipefail
cd "$(dirname "$0")/.."

export APPLE_SIGNING_IDENTITY="${APPLE_SIGNING_IDENTITY:-Developer ID Application: El Mehdi Mamas (7LPSQ8W7G8)}"
PROFILE=disk-map-notary
BUNDLE=src-tauri/target/universal-apple-darwin/release/bundle

# A failed earlier run can leave its writable dmg mounted, and the next
# hdiutil detach then fails with "Resource busy".
detach_stale_dmgs() {
  hdiutil info | awk '/^image-path/ && /rw\.[^\/]*\.dmg$/ {f=1; next} f && /^\/dev\/disk[0-9]+\t/ {print $1; f=0}' |
    while read -r dev; do hdiutil detach -force "$dev" >/dev/null || true; done
  rm -f "$BUNDLE"/dmg/rw.*.dmg "$BUNDLE"/macos/rw.*.dmg
}

rustup target add aarch64-apple-darwin x86_64-apple-darwin
detach_stale_dmgs
if ! pnpm tauri build --target universal-apple-darwin; then
  # The dmg step drives Finder by AppleScript to lay out the window, which can
  # fail (-10006) even though the signed app is already built. Pack it plainly.
  APP=$(ls -d "$BUNDLE"/macos/*.app 2>/dev/null | head -1)
  [ -n "$APP" ] || exit 1
  echo "Tauri dmg step failed; packing the dmg with hdiutil instead." >&2
  detach_stale_dmgs
  NAME=$(basename "$APP" .app)
  VERSION=$(defaults read "$PWD/$APP/Contents/Info.plist" CFBundleShortVersionString)
  STAGE=$(mktemp -d)
  ditto "$APP" "$STAGE/$NAME.app"
  ln -s /Applications "$STAGE/Applications"
  mkdir -p "$BUNDLE/dmg"
  hdiutil create -volname "$NAME" -srcfolder "$STAGE" -ov -format UDZO \
    "$BUNDLE/dmg/${NAME}_${VERSION}_universal.dmg"
  rm -rf "$STAGE"
  codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$BUNDLE/dmg/${NAME}_${VERSION}_universal.dmg"
fi

APP=$(ls -d "$BUNDLE"/macos/*.app | head -1)
DMG=$(ls "$BUNDLE"/dmg/*.dmg | grep -v '/rw\.' | head -1)

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
