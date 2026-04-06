# K2SO Workspace

## Release Process

Releases are automated via `scripts/release.sh`. **Always include release notes.**

```bash
# Usage — both arguments required:
./scripts/release.sh <version> <notes-file>

# Example:
./scripts/release.sh 0.25.0 /tmp/release-notes-0.25.0.md
```

Before running the release script:
1. Commit all changes and push to `main`
2. Write release notes to a temp file (e.g. `/tmp/release-notes-X.Y.Z.md`)
3. Ensure `source "$HOME/.cargo/env"` is run if cargo isn't on PATH
4. The script bumps versions in `package.json`, `src-tauri/Cargo.toml`, `src-tauri/tauri.conf.json`, and `cli/k2so` — do NOT bump manually

The script handles: version bump → build → codesign → notarize → update bundle (tar.gz + sig) → DMG → notarize DMG → latest.json → GitHub release.
