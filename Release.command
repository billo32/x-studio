#!/bin/bash
# Double-click in Finder: build X Studio, tag it and publish a GitHub Release.
cd "$(dirname "$0")" || exit 1
pause() { echo; read -r -p "Press Enter to close" _; }
trap pause EXIT

echo "X Studio — release from $(pwd)"
echo

# gh creates the release
if ! command -v gh >/dev/null; then
  if command -v brew >/dev/null; then
    read -r -p "gh (GitHub CLI) is missing. Install it with brew? [Y/n] " a
    [[ "$a" =~ ^[Nn] ]] || brew install gh
  fi
fi
if command -v gh >/dev/null && ! gh auth status >/dev/null 2>&1; then
  echo "Sign in to GitHub (once):"
  gh auth login --web --git-protocol https || exit 1
fi

# uncommitted changes
if [[ -n "$(git status --porcelain)" ]]; then
  echo "Uncommitted changes:"
  git status --short
  read -r -p "Commit them before the release? [y/N] " a
  if [[ "$a" =~ ^[Yy] ]]; then
    git add -A && git commit -m "Prepare release" || exit 1
  else
    echo "Cancelled."; exit 1
  fi
fi

LAST=$(git describe --tags --abbrev=0 2>/dev/null || echo "none")
read -r -p "Release version (last tag: $LAST), e.g. 0.2.0: " V
[[ -n "$V" ]] || exit 1

./scripts/release.sh "$V"
