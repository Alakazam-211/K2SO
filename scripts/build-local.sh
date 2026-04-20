#!/bin/bash
# K2SO Local-Only Release Build
#
# Builds, signs, and notarizes K2SO into a DMG you can drag into
# /Applications for on-machine testing (especially the P4 acceptance
# checklist: close the lid, wake on schedule, reconnect from mobile).
#
# Does NOT upload to GitHub. Does NOT generate latest.json. Does NOT
# tag the commit. Safe to run multiple times against the same
# version string — each run overwrites the previous DMG.
#
# Prerequisites (same as release.sh):
#   - TAURI_SIGNING_PRIVATE_KEY env var (or ~/.tauri/k2so-updater.key)
#   - TAURI_SIGNING_PRIVATE_KEY_PASSWORD env var (or will prompt)
#   - Apple signing identity in keychain ("K2SO-notarize" profile)
#
# Usage:
#   ./scripts/build-local.sh <version>
#   Example: ./scripts/build-local.sh 0.33.0-rc1
#
# Output:
#   src-tauri/target/release/bundle/dmg/K2SO_<version>_aarch64.dmg
#
# After the script finishes:
#   open src-tauri/target/release/bundle/dmg/
#   → drag K2SO.app to Applications → run the P4 acceptance checklist.

set -euo pipefail

VERSION="${1:-}"
if [ -z "$VERSION" ]; then
    echo "Usage: ./scripts/build-local.sh <version>" >&2
    echo "Example: ./scripts/build-local.sh 0.33.0-rc1" >&2
    exit 1
fi

SIGNING_IDENTITY="Developer ID Application: LZTEK, LLC (36B8R93HXV)"
KEYCHAIN_PROFILE="K2SO-notarize"
PROJECT_DIR="$(cd "$(dirname "$0")/.." && pwd)"

# rustup installs cargo at ~/.cargo/bin, which interactive shells source
# via .zshrc / .bashrc. `bun run tauri build` spawns a non-interactive
# subshell that does NOT source those, so cargo appears missing. Prepend
# explicitly to survive that spawn path.
if [ -d "$HOME/.cargo/bin" ] && ! command -v cargo >/dev/null 2>&1; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi
if ! command -v cargo >/dev/null 2>&1; then
    echo "ERROR: cargo not found on PATH. Install rustup or export PATH manually." >&2
    exit 1
fi

echo "═══════════════════════════════════════════════════"
echo "  K2SO Local Build: v${VERSION}"
echo "  (no GitHub upload, no updater manifest)"
echo "═══════════════════════════════════════════════════"

# Load .env file if present (contains TAURI_SIGNING_PRIVATE_KEY_PASSWORD)
if [ -f "$PROJECT_DIR/.env" ]; then
    set -a
    source "$PROJECT_DIR/.env"
    set +a
    echo "Loaded .env"
fi

# Load signing key from file if env var not set
if [ -z "${TAURI_SIGNING_PRIVATE_KEY:-}" ]; then
    KEY_FILE="$HOME/.tauri/k2so-updater.key"
    if [ -f "$KEY_FILE" ]; then
        export TAURI_SIGNING_PRIVATE_KEY="$(cat "$KEY_FILE")"
        echo "Loaded signing key from $KEY_FILE"
    else
        echo "ERROR: TAURI_SIGNING_PRIVATE_KEY not set and $KEY_FILE not found" >&2
        exit 1
    fi
fi

if [ -z "${TAURI_SIGNING_PRIVATE_KEY_PASSWORD:-}" ]; then
    echo "Enter signing key password:"
    read -s TAURI_SIGNING_PRIVATE_KEY_PASSWORD
    export TAURI_SIGNING_PRIVATE_KEY_PASSWORD
fi

cd "$PROJECT_DIR"

# ── Step 1: Bump version ──
echo ""
echo "Step 1: Bumping version to ${VERSION}..."
sed -i '' "s/\"version\": \"[^\"]*\"/\"version\": \"${VERSION}\"/" package.json src-tauri/tauri.conf.json
sed -i '' "s/^version = \"[^\"]*\"/version = \"${VERSION}\"/" src-tauri/Cargo.toml
sed -i '' "s/K2SO_CLI_VERSION=\"[^\"]*\"/K2SO_CLI_VERSION=\"${VERSION}\"/" cli/k2so
echo "  Done."

# ── Step 2: Build ──
echo ""
echo "Step 2: Building release..."
export APPLE_SIGNING_IDENTITY="$SIGNING_IDENTITY"
export APPLE_TEAM_ID="36B8R93HXV"
bun run tauri build
echo "  Build complete."

# ── Step 2.5: Build + bundle k2so-daemon sidecar ──
echo ""
echo "Step 2.5: Bundling k2so-daemon sidecar..."
cargo build --release -p k2so-daemon
DAEMON_SRC="target/release/k2so-daemon"
if [ ! -x "$DAEMON_SRC" ]; then
    echo "  FATAL: k2so-daemon not at $DAEMON_SRC after cargo build" >&2
    exit 1
fi
cp "$DAEMON_SRC" \
    "src-tauri/target/release/bundle/macos/K2SO.app/Contents/MacOS/k2so-daemon"
echo "  k2so-daemon copied into K2SO.app/Contents/MacOS/"

# ── Step 3: Sign with hardened runtime ──
echo ""
echo "Step 3: Signing with hardened runtime..."
codesign --force --options runtime --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "src-tauri/target/release/bundle/macos/K2SO.app/Contents/MacOS/k2so"
codesign --force --options runtime --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "src-tauri/target/release/bundle/macos/K2SO.app/Contents/MacOS/k2so-daemon"
codesign --force --options runtime --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "src-tauri/target/release/bundle/macos/K2SO.app"
echo "  Signed (main + daemon + bundle)."

# ── Step 4: Notarize app via ZIP ──
echo ""
echo "Step 4: Notarizing app..."
cd src-tauri/target/release/bundle/macos
ditto -c -k --keepParent "K2SO.app" "/tmp/K2SO_${VERSION}.zip"
xcrun notarytool submit "/tmp/K2SO_${VERSION}.zip" \
    --keychain-profile "$KEYCHAIN_PROFILE" --wait
xcrun stapler staple "K2SO.app"
echo "  App notarized and stapled."

# ── Step 5: Create DMG from notarized app ──
echo ""
echo "Step 5: Creating DMG..."
cd "$PROJECT_DIR"
rm -f "src-tauri/target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg"
hdiutil create -volname "K2SO" \
    -srcfolder "src-tauri/target/release/bundle/macos/K2SO.app" \
    -ov -format UDZO \
    "src-tauri/target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg"
codesign --force --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "src-tauri/target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg"

# ── Step 6: Notarize DMG ──
echo ""
echo "Step 6: Notarizing DMG..."
xcrun notarytool submit "src-tauri/target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg" \
    --keychain-profile "$KEYCHAIN_PROFILE" --wait
xcrun stapler staple "src-tauri/target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg"
echo "  DMG notarized and stapled."

DMG_PATH="src-tauri/target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg"

echo ""
echo "═══════════════════════════════════════════════════"
echo "  Local build complete — v${VERSION}"
echo "═══════════════════════════════════════════════════"
echo ""
echo "DMG: $PROJECT_DIR/$DMG_PATH"
echo ""
echo "Next steps:"
echo "  1. open $(dirname "$DMG_PATH")"
echo "  2. Double-click the DMG and drag K2SO.app into /Applications"
echo "  3. Launch K2SO from /Applications (not the dev tree)"
echo "  4. Run the P4 acceptance checklist against the installed app"
echo ""
echo "If you decide to cut a real release from this version:"
echo "  ./scripts/release.sh ${VERSION} [notes-file]"
echo "  (it rebuilds from scratch and adds the GitHub upload steps)"
