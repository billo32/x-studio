#!/bin/bash
# Checks a built X Studio the way Gatekeeper sees it: signature, hardened runtime,
# notarization tickets, Gatekeeper verdict for the app and the disk image (and the app inside it).
#
#   scripts/verify-release.sh                  # newest build in target/ (or the newest release/<tag>)
#   scripts/verify-release.sh release/v0.2.0   # a release folder (.dmg + .app.zip)
#   scripts/verify-release.sh path/to/X\ Studio.app | file.dmg | file.app.zip
#
# Exit code 0 only if every check passes.
set -uo pipefail
cd "$(dirname "$0")/.." || exit 1

[[ "$(uname)" == "Darwin" ]] || { echo "run this on macOS" >&2; exit 1; }
[[ -f signing.env ]] && set -a && source signing.env && set +a

PASS=0 FAIL=0 WARN=0
WORK=$(mktemp -d)
MOUNT=""
cleanup() {
  [[ -n "$MOUNT" ]] && hdiutil detach "$MOUNT" -quiet -force >/dev/null 2>&1
  rm -rf "$WORK"
}
trap cleanup EXIT

green() { printf '  \033[32m✓\033[0m %s\n' "$*"; }
red()   { printf '  \033[31m✗\033[0m %s\n' "$*"; }
note()  { printf '  \033[33m!\033[0m %s\n' "$*"; }
head1() { printf '\n\033[1m%s\033[0m\n' "$*"; }
indent() { sed 's/^/      /'; }

# check "description" command…   — passes when the command succeeds
check() {
  local desc=$1; shift
  local out
  if out=$("$@" 2>&1); then green "$desc"; PASS=$((PASS + 1))
  else red "$desc"; printf '%s\n' "$out" | tail -8 | indent; FAIL=$((FAIL + 1)); fi
}

# expect "description" "regex" command…   — passes when the output matches
expect() {
  local desc=$1 pattern=$2; shift 2
  local out
  out=$("$@" 2>&1)
  if grep -qE "$pattern" <<<"$out"; then green "$desc"; PASS=$((PASS + 1))
  else red "$desc"; printf '%s\n' "$out" | tail -8 | indent; FAIL=$((FAIL + 1)); fi
}

warn() { note "$*"; WARN=$((WARN + 1)); }

# ---------- what to check ----------
APP="" DMG="" DIR=""
newest() { ls -td "$@" 2>/dev/null | head -1; }
unzip_app() { ditto -x -k "$1" "$WORK/zip" && newest "$WORK"/zip/*.app; }

ARG="${1:-}"
if [[ -z "$ARG" ]]; then
  APP=$(newest target/*/release/bundle/macos/*.app target/release/bundle/macos/*.app)
  DMG=$(newest target/*/release/bundle/dmg/*.dmg target/release/bundle/dmg/*.dmg)
  if [[ -z "$APP" && -z "$DMG" ]]; then
    ARG=$(newest release/v*)
    [[ -n "$ARG" ]] || { echo "nothing to check: no build in target/ or release/ — run npm run release first" >&2; exit 1; }
  fi
fi
if [[ -n "$ARG" ]]; then
  if [[ -d "$ARG" && "$ARG" == *.app ]]; then APP=$ARG
  elif [[ -d "$ARG" ]]; then
    DIR=$ARG
    DMG=$(newest "$DIR"/*.dmg)
    ZIP=$(newest "$DIR"/*.app.zip)
    [[ -n "$ZIP" ]] && APP=$(unzip_app "$ZIP")
  elif [[ "$ARG" == *.dmg ]]; then DMG=$ARG
  elif [[ "$ARG" == *.zip ]]; then APP=$(unzip_app "$ARG")
  else echo "don't know how to check $ARG" >&2; exit 1; fi
fi

# ---------- checks for an .app ----------
check_app() {
  local app=$1
  check "code signature is valid (deep, strict)" codesign --verify --deep --strict --verbose=2 "$app"
  local info
  info=$(codesign -dv --verbose=4 "$app" 2>&1)
  expect "signed with a Developer ID Application certificate" "Authority=Developer ID Application" echo "$info"
  expect "hardened runtime is on" "flags=.*runtime" echo "$info"
  expect "secure timestamp" "^Timestamp=" echo "$info"
  expect "notarization ticket is stapled" "worked" xcrun stapler validate "$app"
  expect "Gatekeeper: accepted as notarized" "accepted" spctl -a -vvv -t exec "$app"
  expect "Gatekeeper: source is Notarized Developer ID" "source=Notarized Developer ID" spctl -a -vvv -t exec "$app"
  # macOS 14+: Apple's own pre-distribution check. Advisory — reported as a warning.
  if command -v syspolicy_check >/dev/null; then
    local sp
    if sp=$(syspolicy_check distribution "$app" 2>&1); then green "syspolicy_check: ready for distribution"
    else warn "syspolicy_check reported issues:"; printf '%s\n' "$sp" | tail -6 | indent; fi
  fi

  local plist="$app/Contents/Info.plist"
  local version bundle_id
  version=$(/usr/libexec/PlistBuddy -c "Print CFBundleShortVersionString" "$plist" 2>/dev/null)
  bundle_id=$(/usr/libexec/PlistBuddy -c "Print CFBundleIdentifier" "$plist" 2>/dev/null)
  local exe archs
  exe="$app/Contents/MacOS/$(/usr/libexec/PlistBuddy -c "Print CFBundleExecutable" "$plist" 2>/dev/null)"
  archs=$(lipo -archs "$exe" 2>/dev/null)
  note "version ${version:-?}, ${bundle_id:-?}, architectures: ${archs:-?}"
  [[ "$archs" == *x86_64* && "$archs" == *arm64* ]] || warn "not universal — Intel Macs can't run it (fine if that's intended)"
}

if [[ -n "$APP" ]]; then
  head1 "App: $APP"
  check_app "$APP"
fi

# ---------- checks for a .dmg ----------
if [[ -n "$DMG" ]]; then
  head1 "Disk image: $DMG"
  check "disk image is signed" codesign --verify --verbose=2 "$DMG"
  expect "notarization ticket is stapled" "worked" xcrun stapler validate "$DMG"
  expect "Gatekeeper: source is Notarized Developer ID" "source=Notarized Developer ID" \
    spctl -a -vvv -t open --context context:primary-signature "$DMG"

  # As if it had been downloaded with a browser
  cp "$DMG" "$WORK/download.dmg"
  xattr -w com.apple.quarantine "0081;$(printf %x "$(date +%s)");Safari;" "$WORK/download.dmg"
  expect "Gatekeeper: a downloaded (quarantined) copy is accepted" "accepted" \
    spctl -a -vvv -t open --context context:primary-signature "$WORK/download.dmg"

  MOUNT="$WORK/mnt"
  mkdir -p "$MOUNT"
  if hdiutil attach "$DMG" -nobrowse -readonly -noautoopen -mountpoint "$MOUNT" -quiet; then
    INNER=$(newest "$MOUNT"/*.app)
    if [[ -n "$INNER" ]]; then
      head1 "App inside the disk image"
      check_app "$INNER"
      expect "Applications shortcut is present" "Applications" ls -la "$MOUNT"
    else
      red "no .app inside the disk image"; FAIL=$((FAIL + 1))
    fi
    hdiutil detach "$MOUNT" -quiet -force >/dev/null 2>&1
  else
    red "could not mount the disk image"; FAIL=$((FAIL + 1))
  fi
  MOUNT=""
fi

# ---------- release folder ----------
if [[ -n "$DIR" && -f "$DIR/SHA256SUMS.txt" ]]; then
  head1 "Checksums"
  check "SHA256SUMS.txt matches the files" bash -c "cd \"$DIR\" && shasum -a 256 -c SHA256SUMS.txt"
fi

# ---------- Apple's side ----------
if [[ -n "${APPLE_NOTARY_PROFILE:-}" ]]; then
  head1 "Recent notarization submissions"
  xcrun notarytool history --keychain-profile "$APPLE_NOTARY_PROFILE" 2>&1 \
    | grep -E "createdDate|name|status" | head -9 | indent
fi

# ---------- summary ----------
echo
if ((FAIL == 0)); then
  printf '\033[32m✓ All %d checks passed\033[0m' "$PASS"
  ((WARN)) && printf ' (%d warning%s)' "$WARN" "$([[ $WARN == 1 ]] || echo s)"
  echo
  exit 0
fi
printf '\033[31m✗ %d of %d checks failed\033[0m\n' "$FAIL" $((PASS + FAIL))
exit 1
