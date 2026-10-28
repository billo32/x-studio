#!/bin/bash
# Notarizes a built release (dmg + app.zip in release/<tag>) and, optionally, replaces the files
# in its GitHub Release.
#   scripts/notarize-release.sh v0.2.0              # notarize and upload to the release
#   scripts/notarize-release.sh v0.2.0 --no-upload  # notarize only (called from release.sh)
# Credentials: signing.env (see signing.env.example).
set -euo pipefail
TAG="${1:?usage: scripts/notarize-release.sh vX.Y.Z [--no-upload]}"
UPLOAD=1; [[ "${2:-}" == "--no-upload" ]] && UPLOAD=0
ROOT="$(cd "$(dirname "$0")/.." && pwd)"; cd "$ROOT"
OUT="$ROOT/release/$TAG"
[[ -d "$OUT" ]] || { echo "no $OUT — build the release first" >&2; exit 1; }
if [[ -f signing.env ]]; then set -a; source signing.env; set +a; fi

if [[ -n "${APPLE_NOTARY_PROFILE:-}" ]]; then NA=(--keychain-profile "$APPLE_NOTARY_PROFILE")
elif [[ -n "${APPLE_API_KEY:-}" ]]; then NA=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER")
elif [[ -n "${APPLE_ID:-}" ]]; then NA=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
else echo "no notarization credentials — fill in signing.env (see signing.env.example)" >&2; exit 1; fi

ID="${APPLE_SIGNING_IDENTITY:-$(security find-identity -v -p codesigning | grep -m1 '"Developer ID Application' | sed -E 's/.*"(.*)"/\1/')}"
[[ -n "$ID" ]] || { echo "no Developer ID Application certificate" >&2; exit 1; }
DMG=$(ls "$OUT"/*.dmg | head -1)
ZIP=$(ls "$OUT"/*.app.zip | head -1)

notarize() {
  echo "== notarytool: $(basename "$1") (usually 1–10 minutes)"
  local out
  out=$(xcrun notarytool submit "$1" "${NA[@]}" --wait 2>&1) || true
  echo "$out"
  if ! grep -q "status: Accepted" <<<"$out"; then
    local id; id=$(grep -m1 -oE 'id: [0-9a-f-]{36}' <<<"$out" | cut -d' ' -f2)
    echo "✗ Apple rejected $(basename "$1")${id:+. Details: xcrun notarytool log $id ${NA[*]}}" >&2
    exit 1
  fi
}

# .app: notarize the zip → staple the ticket to the .app itself → re-zip
WORK=$(mktemp -d); trap 'rm -rf "$WORK"' EXIT
ditto -x -k "$ZIP" "$WORK"
APP=$(ls -d "$WORK"/*.app | head -1)
codesign --verify --deep --strict "$APP"
notarize "$ZIP"
xcrun stapler staple "$APP"
rm "$ZIP"; ditto -c -k --keepParent "$APP" "$ZIP"

# .dmg: staple the app inside → sign → notarize → staple
scripts/staple-dmg-app.sh "$DMG"
codesign --force --timestamp --sign "$ID" "$DMG"
notarize "$DMG"
xcrun stapler staple "$DMG"

echo "== Gatekeeper"
spctl -a -vv -t exec "$APP"
spctl -a -vv -t install "$DMG"

(cd "$OUT" && shasum -a 256 ./*.dmg ./*.zip > SHA256SUMS.txt)

if ((UPLOAD)); then
  gh release upload "$TAG" "$DMG" "$ZIP" "$OUT/SHA256SUMS.txt" --clobber
  NOTES=$(gh release view "$TAG" --json body -q .body | sed 's/^Signed with Developer ID\.$/Signed with Developer ID and notarized by Apple./')
  gh release edit "$TAG" --notes "$NOTES" >/dev/null
  echo "✓ $TAG: release files replaced with notarized ones"
fi
