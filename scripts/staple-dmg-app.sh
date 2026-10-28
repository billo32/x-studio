#!/bin/bash
# Staples the notarization ticket to the .app inside a disk image, keeping the image's layout
# (background, icon positions, Applications link). Run it after the .app has been notarized and
# before the .dmg is signed and notarized itself — Tauri packs the dmg before notarization, so
# without this the copy users drag to Applications has no ticket and needs a network check on
# first launch.
#
#   scripts/staple-dmg-app.sh path/to/X\ Studio.dmg
set -euo pipefail
DMG="${1:?usage: scripts/staple-dmg-app.sh file.dmg}"

WORK=$(mktemp -d)
MNT="$WORK/mnt"
cleanup() {
  hdiutil detach "$MNT" -quiet -force >/dev/null 2>&1 || true
  rm -rf "$WORK"
}
trap cleanup EXIT

echo "== stapling the app inside $(basename "$DMG")"
hdiutil convert "$DMG" -format UDRW -o "$WORK/rw.dmg" -quiet
hdiutil resize -size "$(( $(stat -f %z "$WORK/rw.dmg") / 1048576 + 16 ))m" "$WORK/rw.dmg" >/dev/null 2>&1 || true
mkdir "$MNT"
hdiutil attach "$WORK/rw.dmg" -nobrowse -noautoopen -mountpoint "$MNT" -quiet

APP=$(ls -d "$MNT"/*.app | head -1)
[[ -n "$APP" ]] || { echo "no .app inside $DMG" >&2; exit 1; }
xcrun stapler staple "$APP"
xcrun stapler validate "$APP" >/dev/null

for _ in 1 2 3 4 5; do hdiutil detach "$MNT" -quiet && break || sleep 1; done
hdiutil convert "$WORK/rw.dmg" -format UDZO -imagekey zlib-level=9 -o "$WORK/out.dmg" -quiet
mv -f "$WORK/out.dmg" "$DMG"
echo "✓ $(basename "$APP") inside the disk image has its ticket"
