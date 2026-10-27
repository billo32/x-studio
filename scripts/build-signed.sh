#!/bin/bash
# Signed and notarized X Studio build (Developer ID, outside the App Store).
#
#   cp signing.env.example signing.env   # fill in once
#   npm run release                      # or: scripts/build-signed.sh --target universal-apple-darwin
#
# Output: target/[<target>/]release/bundle/{macos/X Studio.app, dmg/*.dmg}
set -euo pipefail
cd "$(dirname "$0")/.."

[[ -f signing.env ]] && set -a && source signing.env && set +a

# "Developer ID Application" certificate from the keychain unless set explicitly
if [[ -z "${APPLE_SIGNING_IDENTITY:-}" ]]; then
  APPLE_SIGNING_IDENTITY=$(security find-identity -v -p codesigning | grep -m1 '"Developer ID Application' | sed -E 's/.*"(.*)"/\1/' || true)
fi
if [[ -z "$APPLE_SIGNING_IDENTITY" ]]; then
  echo "No \"Developer ID Application\" certificate in the keychain." >&2
  echo "Xcode → Settings → Accounts → Manage Certificates → + → Developer ID Application" >&2
  exit 1
fi
export APPLE_SIGNING_IDENTITY
echo "Signing as: $APPLE_SIGNING_IDENTITY"

notarize_args=()
if [[ "${SKIP_NOTARIZE:-0}" == 1 ]]; then
  # release.sh notarizes the dmg and app.zip itself (scripts/notarize-release.sh)
  unset APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID
  echo "Notarization runs as a separate step after the build."
elif [[ -n "${APPLE_NOTARY_PROFILE:-}" ]]; then
  # Tauri can't use a keychain profile: it only signs, and notarytool notarizes below.
  notarize_args=(--keychain-profile "$APPLE_NOTARY_PROFILE")
  unset APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID
elif [[ -n "${APPLE_API_KEY:-}" ]]; then
  : "${APPLE_API_ISSUER:?APPLE_API_ISSUER is required}" "${APPLE_API_KEY_PATH:?APPLE_API_KEY_PATH is required}"
  export APPLE_API_KEY APPLE_API_ISSUER APPLE_API_KEY_PATH
  notarize_args=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER")
elif [[ -n "${APPLE_ID:-}" ]]; then
  : "${APPLE_PASSWORD:?APPLE_PASSWORD (app-specific) is required}" "${APPLE_TEAM_ID:?APPLE_TEAM_ID is required}"
  export APPLE_ID APPLE_PASSWORD APPLE_TEAM_ID
  notarize_args=(--apple-id "$APPLE_ID" --password "$APPLE_PASSWORD" --team-id "$APPLE_TEAM_ID")
else
  echo "⚠ No notarization credentials — signing only (Gatekeeper will warn on other Macs)."
fi

# Universal (Apple Silicon + Intel) unless a --target is given and rustup has both targets
if [[ " $* " != *" --target"* ]]; then
  export PATH="$HOME/.cargo/bin:$PATH"
  if command -v rustup >/dev/null && rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null 2>&1 \
     && [[ -d "$(rustc --print sysroot)/lib/rustlib/x86_64-apple-darwin" ]]; then
    set -- --target universal-apple-darwin "$@"
    echo "Building universal (Apple Silicon + Intel)"
  else
    echo "⚠ rustc $(command -v rustc) has no x86_64 target — building for this Mac only."
    echo "  For universal builds: brew uninstall rust; install rustup (https://rustup.rs)."
  fi
fi

# Tauri signs the .app (hardened runtime) and notarizes it when the API key or Apple ID variables are set
npx tauri build --bundles app,dmg "$@"

BUNDLE=target/release/bundle
for t in universal-apple-darwin aarch64-apple-darwin x86_64-apple-darwin; do
  if [[ " $* " == *" $t"* ]]; then BUNDLE="target/$t/release/bundle"; fi
done
APP="$BUNDLE/macos/X Studio.app"
DMG=$(ls -t "$BUNDLE"/dmg/*.dmg | head -1)

codesign --verify --deep --strict --verbose=2 "$APP"

if (( ${#notarize_args[@]} )); then
  # With a keychain profile Tauri didn't notarize the app: do it here (zip → notarize → staple).
  if [[ -n "${APPLE_NOTARY_PROFILE:-}" ]]; then
    ZIP=$(mktemp -d)/app.zip
    ditto -c -k --keepParent "$APP" "$ZIP"
    xcrun notarytool submit "$ZIP" "${notarize_args[@]}" --wait
    xcrun stapler staple "$APP"
  fi
  # Tauri packed the dmg before notarization: staple the copy inside it too
  scripts/staple-dmg-app.sh "$DMG"
  # The DMG is notarized on its own so it opens without warnings
  codesign --force --timestamp --sign "$APPLE_SIGNING_IDENTITY" "$DMG"
  xcrun notarytool submit "$DMG" "${notarize_args[@]}" --wait
  xcrun stapler staple "$DMG"
  spctl -a -vv -t install "$DMG"
fi
spctl -a -vv -t exec "$APP" || true

echo
echo "Done:"
echo "  $APP"
echo "  $DMG"
