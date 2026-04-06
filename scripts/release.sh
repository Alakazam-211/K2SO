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

# ── Step 3: Sign with hardened runtime ──
echo ""
echo "Step 3: Signing with hardened runtime..."
codesign --force --options runtime --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "src-tauri/target/release/bundle/macos/K2SO.app/Contents/MacOS/k2so"
codesign --force --options runtime --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "src-tauri/target/release/bundle/macos/K2SO.app"
echo "  Signed."

# ── Step 4: Notarize app via ZIP ──
echo ""
echo "Step 4: Notarizing app..."
cd src-tauri/target/release/bundle/macos
ditto -c -k --keepParent "K2SO.app" "/tmp/K2SO_${VERSION}.zip"
xcrun notarytool submit "/tmp/K2SO_${VERSION}.zip" \
    --keychain-profile "$KEYCHAIN_PROFILE" --wait
xcrun stapler staple "K2SO.app"
echo "  App notarized and stapled."

# ── Step 5: Create update bundle (tar.gz) from notarized app + sign it ──
echo ""
echo "Step 5: Creating and signing update bundle..."
cd "$PROJECT_DIR"
COPYFILE_DISABLE=1 tar -czf "src-tauri/target/release/bundle/macos/K2SO.app.tar.gz" \
    -C "src-tauri/target/release/bundle/macos" "K2SO.app"

# Sign the update bundle with Tauri updater key
bunx @tauri-apps/cli@2 signer sign \
    "src-tauri/target/release/bundle/macos/K2SO.app.tar.gz" \
    --private-key "$TAURI_SIGNING_PRIVATE_KEY" \
    --password "$TAURI_SIGNING_PRIVATE_KEY_PASSWORD"
echo "  Update bundle signed."

# ── Step 6: Create DMG from notarized app ──
echo ""
echo "Step 6: Creating DMG..."
rm -f "src-tauri/target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg"
hdiutil create -volname "K2SO" \
    -srcfolder "src-tauri/target/release/bundle/macos/K2SO.app" \
    -ov -format UDZO \
    "src-tauri/target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg"
codesign --force --timestamp \
    --sign "$SIGNING_IDENTITY" \
    "src-tauri/target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg"

# ── Step 7: Notarize DMG ──
echo ""
echo "Step 7: Notarizing DMG..."
xcrun notarytool submit "src-tauri/target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg" \
    --keychain-profile "$KEYCHAIN_PROFILE" --wait
xcrun stapler staple "src-tauri/target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg"
echo "  DMG notarized and stapled."

# ── Step 8: Generate latest.json ──
echo ""
echo "Step 8: Generating latest.json..."
SIG_CONTENT=""
SIG_FILE="src-tauri/target/release/bundle/macos/K2SO.app.tar.gz.sig"
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
    "src-tauri/target/release/bundle/dmg/K2SO_${VERSION}_aarch64.dmg"
    "src-tauri/target/release/bundle/macos/K2SO.app.tar.gz"
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
