#!/bin/bash
# K2SO Release Script
# Builds, signs, notarizes, and releases K2SO with both DMG and update bundle.
#
# Prerequisites:
#   - TAURI_SIGNING_PRIVATE_KEY env var (or ~/.tauri/k2so-updater.key)
#   - TAURI_SIGNING_PRIVATE_KEY_PASSWORD env var
#   - Apple signing identity in keychain
#   - gh CLI authenticated
#
# Usage:
#   ./scripts/release.sh <version>
#   Example: ./scripts/release.sh 0.25.0

set -euo pipefail

VERSION="${1:-}"
NOTES_FILE="${2:-}"
if [ -z "$VERSION" ]; then
    echo "Usage: ./scripts/release.sh <version> [notes-file]" >&2
    echo "Example: ./scripts/release.sh 0.25.0 release-notes.md" >&2
    echo "" >&2
    echo "If notes-file is provided, its contents are used as GitHub release notes." >&2
    echo "Otherwise, a placeholder is used (edit on GitHub after release)." >&2
    exit 1
fi

TAG="v${VERSION}"
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
echo "  K2SO Release: ${TAG}"
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
sed -i '' "s/^version = \"[^\"]*\"/version = \"${VERSION}\"/" \
    src-tauri/Cargo.toml \
    crates/k2so-core/Cargo.toml \
    crates/k2so-daemon/Cargo.toml
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
#
# k2so-daemon is a peer binary to the main Tauri app that owns the
# persistent-agent runtime (launched by launchd, outlives the Tauri
# process). It needs to sit next to `k2so` inside `Contents/MacOS/`
# so `std::env::current_exe()?.parent()?.join("k2so-daemon")` — used
# by the `install_daemon_plist_v1` code migration — can find it on
# first launch of a release build.
#
# We build it explicitly in release mode (cargo workspace builds it
# alongside the Tauri crate, but `tauri build` copies only its own
# primary bin into the bundle) then `cp` it in. Hardened-runtime
# signing in Step 3 covers this binary too.
echo ""
echo "Step 2.5: Bundling k2so-daemon sidecar..."
# cargo workspace root is the repo root — both `k2so` (Tauri) and
# `k2so-daemon` build into `target/release/`. Tauri's bundler writes
# only its own primary bin into the .app, so we copy k2so-daemon in
# explicitly.
cargo build --release -p k2so-daemon
DAEMON_SRC="target/release/k2so-daemon"
if [ ! -x "$DAEMON_SRC" ]; then
    echo "  FATAL: k2so-daemon not at $DAEMON_SRC after cargo build" >&2
    exit 1
fi
cp "$DAEMON_SRC" \
    "target/release/bundle/macos/K2SO.app/Contents/MacOS/k2so-daemon"
echo "  k2so-daemon copied into K2SO.app/Contents/MacOS/"

# ── Step 3: Sign with hardened runtime ──
echo ""
echo "Step 3: Signing with hardened runtime..."
# Inner binaries first (Apple requires sub-binaries signed before the
# outer bundle, otherwise codesign rejects with 'resource fork … not
# allowed').
codesign --force --options runtime --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2SO.app/Contents/MacOS/k2so"
codesign --force --options runtime --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2SO.app/Contents/MacOS/k2so-daemon"
codesign --force --options runtime --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/macos/K2SO.app"
echo "  Signed (main + daemon + bundle)."

# ── Step 4: Notarize app via ZIP ──
echo ""
echo "Step 4: Notarizing app..."
cd target/release/bundle/macos
ditto -c -k --keepParent "K2SO.app" "/tmp/K2SO_${VERSION}.zip"
xcrun notarytool submit "/tmp/K2SO_${VERSION}.zip" \
    --keychain-profile "$KEYCHAIN_PROFILE" --wait
xcrun stapler staple "K2SO.app"
echo "  App notarized and stapled."

# ── Step 5: Create update bundle (tar.gz) from notarized app + sign it ──
echo ""
echo "Step 5: Creating and signing update bundle..."
cd "$PROJECT_DIR"
COPYFILE_DISABLE=1 tar -czf "target/release/bundle/macos/K2SO.app.tar.gz" \
    -C "target/release/bundle/macos" "K2SO.app"

# Sign the update bundle with Tauri updater key
bunx @tauri-apps/cli@2 signer sign \
    "target/release/bundle/macos/K2SO.app.tar.gz" \
    --private-key "$TAURI_SIGNING_PRIVATE_KEY" \
    --password "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
echo "  Update bundle signed."

# ── Step 6: Create DMG from notarized app ──
echo ""
echo "Step 6: Creating DMG..."
rm -f "target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg"
hdiutil create -volname "K2SO" \
    -srcfolder "target/release/bundle/macos/K2SO.app" \
    -ov -format UDZO \
    "target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg"
codesign --force --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg"

# ── Step 7: Notarize DMG ──
echo ""
echo "Step 7: Notarizing DMG..."
xcrun notarytool submit "target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg" \
    --keychain-profile "$KEYCHAIN_PROFILE" --wait
xcrun stapler staple "target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg"
echo "  DMG notarized and stapled."

# ── Step 8: Generate latest.json ──
echo ""
echo "Step 8: Generating latest.json..."
SIG_CONTENT=""
SIG_FILE="target/release/bundle/macos/K2SO.app.tar.gz.sig"
if [ -f "$SIG_FILE" ]; then
    SIG_CONTENT=$(cat "$SIG_FILE")
fi

PUB_DATE=$(date -u +"%Y-%m-%dT%H:%M:%SZ")
cat > "/tmp/latest.json" <<MANIFEST
{
  "version": "${VERSION}",
  "notes": "K2SO ${TAG}",
  "pub_date": "${PUB_DATE}",
  "platforms": {
    "darwin-aarch64": {
      "signature": "${SIG_CONTENT}",
      "url": "https://github.com/Alakazam-211/K2SO/releases/download/${TAG}/K2SO.app.tar.gz"
    }
  }
}
MANIFEST
echo "  latest.json generated."

# ── Step 9: Create GitHub Release ──
echo ""
echo "Step 9: Creating GitHub release ${TAG}..."
ASSETS=(
    "target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg"
    "target/release/bundle/macos/K2SO.app.tar.gz"
)
[ -f "$SIG_FILE" ] && ASSETS+=("$SIG_FILE")
ASSETS+=("/tmp/latest.json")

if [ -n "$NOTES_FILE" ] && [ -f "$NOTES_FILE" ]; then
    gh release create "$TAG" "${ASSETS[@]}" \
        --title "$TAG" \
        --notes-file "$NOTES_FILE"
else
    gh release create "$TAG" "${ASSETS[@]}" \
        --title "$TAG" \
        --notes "K2SO ${TAG} — release notes pending."
    echo "  NOTE: No notes file provided. Edit release notes on GitHub."
fi

echo ""
echo "═══════════════════════════════════════════════════"
echo "  Release ${TAG} complete!"
echo "  https://github.com/Alakazam-211/K2SO/releases/tag/${TAG}"
echo "═══════════════════════════════════════════════════"
