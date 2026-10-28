#!/bin/bash
# Local release from a Mac: version → universal app build → tag → GitHub Release.
#
#   scripts/release.sh 0.2.0
#
# Signed and notarized when a Developer ID certificate (and signing.env) is present,
# otherwise an unsigned build with a warning.
# The release is created with gh (brew install gh && gh auth login); without gh you get
# instructions for a manual upload.
set -euo pipefail

VERSION="${1:?usage: scripts/release.sh X.Y.Z}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must be X.Y.Z" >&2; exit 1; }
TAG="v$VERSION"
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
OUT="$ROOT/release/$TAG"
REPO=billo32/x-studio

step() { printf '\n\033[1;33m== %s\033[0m\n' "$*"; }

# ---------- checks ----------
[[ "$(uname)" == "Darwin" ]] || { echo "run this on macOS" >&2; exit 1; }
[[ -z "$(git status --porcelain)" ]] || { echo "uncommitted changes — commit first" >&2; git status --short; exit 1; }
git rev-parse "$TAG" >/dev/null 2>&1 && { echo "tag $TAG already exists" >&2; exit 1; }
HAVE_GH=0
if command -v gh >/dev/null && gh auth status >/dev/null 2>&1; then HAVE_GH=1; fi
rm -rf "$OUT" && mkdir -p "$OUT"

# ---------- version ----------
step "Version $VERSION"
sed -i '' -E "s/^version = \"[^\"]+\"/version = \"$VERSION\"/" Cargo.toml
sed -i '' -E "s/\"version\": \"[^\"]+\"/\"version\": \"$VERSION\"/" src-tauri/tauri.conf.json
sed -i '' -E "1,/\"version\":/ s/\"version\": \"[^\"]+\"/\"version\": \"$VERSION\"/" package.json

# ---------- checks before building ----------
step "Tests"
npm ci --silent
npm test
cargo test --workspace --quiet

# ---------- app ----------
step "X Studio"
# rustup's cargo/rustc must come before Homebrew's: targets only install into rustup toolchains
export PATH="$HOME/.cargo/bin:$PATH"
TARGET=universal-apple-darwin
ARCH_LABEL=universal
if command -v rustup >/dev/null && rustup target add aarch64-apple-darwin x86_64-apple-darwin >/dev/null 2>&1 \
   && [[ -d "$(rustc --print sysroot)/lib/rustlib/x86_64-apple-darwin" ]]; then
  echo "rustc: $(command -v rustc) — universal (Apple Silicon + Intel)"
else
  TARGET=aarch64-apple-darwin
  ARCH_LABEL=arm64
  echo "⚠ rustc $(command -v rustc) has no x86_64 target — building for Apple Silicon only."
  echo "  For universal builds: brew uninstall rust; install rustup (https://rustup.rs)."
fi
SIGNED=0
NOTARIZED=0
if [[ -f signing.env ]]; then set -a; source signing.env; set +a; fi
HAVE_NOTARY=0
[[ -n "${APPLE_NOTARY_PROFILE:-}${APPLE_API_KEY:-}${APPLE_ID:-}" ]] && HAVE_NOTARY=1
if [[ -n "${APPLE_SIGNING_IDENTITY:-}" ]] || security find-identity -v -p codesigning | grep -q '"Developer ID Application'; then
  SKIP_NOTARIZE=1 ./scripts/build-signed.sh --target "$TARGET"
  SIGNED=1
  ((HAVE_NOTARY)) || echo "⚠ No notarization credentials (signing.env) — the release will be signed but not notarized."
else
  echo "⚠ No Developer ID certificate — unsigned build (a downloaded copy needs \"Open Anyway\" in Privacy & Security)."
  npx tauri build --bundles app,dmg --target "$TARGET"
fi
BUNDLE="target/$TARGET/release/bundle"
DMG=$(ls -t "$BUNDLE"/dmg/*.dmg | head -1)

cp "$DMG" "$OUT/XStudio-$VERSION-$ARCH_LABEL.dmg"
(cd "$BUNDLE/macos" && ditto -c -k --keepParent "X Studio.app" "$OUT/XStudio-$VERSION-$ARCH_LABEL.app.zip")

if ((SIGNED && HAVE_NOTARY)); then
  step "Notarization"
  scripts/notarize-release.sh "$TAG" --no-upload
  NOTARIZED=1
else
  (cd "$OUT" && shasum -a 256 ./*.dmg ./*.zip > SHA256SUMS.txt)
fi
ls -lh "$OUT"

# ---------- git ----------
step "Commit and tag $TAG"
git add Cargo.toml Cargo.lock src-tauri/tauri.conf.json package.json package-lock.json
git commit -m "Release $TAG" || true
git tag -a "$TAG" -m "X Studio $TAG"
git push origin HEAD "$TAG"

# ---------- release ----------
NOTES="$OUT/NOTES.md"
cat > "$NOTES" <<NOTES_EOF
## X Studio $TAG

**X Studio for macOS** ($( [[ $ARCH_LABEL == universal ]] && echo "Apple Silicon + Intel" || echo "Apple Silicon only" )): \`XStudio-$VERSION-$ARCH_LABEL.dmg\`
$( ((NOTARIZED)) && echo "Signed with Developer ID and notarized by Apple." || { ((SIGNED)) && echo "Signed with Developer ID."; } || echo "Unsigned: after the first launch attempt open System Settings → Privacy & Security → \"Open Anyway\", or run \`xattr -dr com.apple.quarantine \"/Applications/X Studio.app\"\`." )

Works best with the [CrossInk](https://github.com/uxjulia/CrossInk) firmware.
NOTES_EOF

if ((HAVE_GH)); then
  step "GitHub Release"
  gh release create "$TAG" "$OUT"/*.dmg "$OUT"/*.zip "$OUT/SHA256SUMS.txt" \
    --repo "$REPO" --title "X Studio $TAG" --notes-file "$NOTES"
  gh release view "$TAG" --repo "$REPO" --web >/dev/null 2>&1 || true
else
  step "Manual release"
  echo "gh is not set up. Open https://github.com/$REPO/releases/new?tag=$TAG"
  echo "and drop in the files from $OUT (notes: $NOTES)."
  open "$OUT"
fi
