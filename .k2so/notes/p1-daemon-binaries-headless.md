# Remote-update P1 — Standalone daemon binaries + headless buildability

**Status:** P1 deliverables landed (worktree). macOS standalone daemon
build + sign + manifest VERIFIED native. Linux build is CI's job and is
best-effort until exercised on ubuntu runners.

## What P1 ships

1. `daemon-latest.json` manifest schema (P2/P3 consume it) — versioned,
   per-platform `{ url, sig, sha256 }` under `artifacts`. Keys:
   `macos-aarch64`, `linux-x86_64`, `linux-aarch64`. A missing key means
   "no build for that platform yet" — consumers must tolerate it.
2. `scripts/release.sh` Step 8.5: builds the native macos-aarch64
   standalone daemon (reuses the `target/release/k2so-daemon` already
   built in Step 2.5), minisign-signs it with the Tauri updater key
   (`bunx @tauri-apps/cli@2 signer sign`, same mechanism as
   `K2SO.app.tar.gz.sig`), computes sha256, emits `daemon-latest.json`,
   and uploads `k2so-daemon-macos-aarch64{,.sig}` + manifest in Step 9.
   Merges CI Linux artifacts if staged in `target/release/daemon-dist/`,
   else emits a macos-only manifest.
3. `.github/workflows/daemon-binaries.yml`: on a `v*` tag, builds
   `k2so-daemon` on native ubuntu for `x86_64-unknown-linux-gnu` and
   (cross) `aarch64-unknown-linux-gnu`, signs each with the same key
   (CI secrets `TAURI_SIGNING_PRIVATE_KEY` / `..._PASSWORD`), uploads
   `k2so-daemon-linux-<arch>{,.sig}` to the release.

P3 verifies all `.sig` files against `plugins.updater.pubkey` in
`src-tauri/tauri.conf.json` — daemon binaries and the app bundle share
one signing key, so one trust root covers both.

## Headless (Linux) buildability — findings

Good news: the daemon is **far less mac-coupled than feared.** Most
mac-only paths were already cfg-gated with Linux fallbacks before P1
(prior Linux-port work). What was already in place:

- `crates/k2so-core/Cargo.toml` — `cocoa` / `objc` already under
  `[target.'cfg(target_os = "macos")'.dependencies]`.
- `fs_commands.rs` — `clipboard_read_file_paths` macos pasteboard code in
  `#[cfg(target_os = "macos")]`, with a `#[cfg(target_os = "linux")]`
  fallback (lines ~678/691/716).
- `heartbeats/install.rs` — launchd (macos) vs systemd/linux branches
  throughout.
- `claude_auth_host.rs` — macos Keychain vs linux file-based branches.
- `terminal/alacritty_backend.rs` — per-os branches.
- `companion/keychain.rs` — macos-gated, used behind cfg elsewhere.

### What P1 cfg-gated (the one remaining hard blocker)

`llama-cpp-2` was pulled with `features = ["metal", "sampler"]`
unconditionally. The `metal` feature compiles **ggml-metal**, which is
macOS-only and would fail a Linux build. Fix in
`crates/k2so-core/Cargo.toml`:

- Base `[dependencies]`: `llama-cpp-2 = { default-features = false,
  features = ["sampler"] }` (no `metal`).
- `[target.'cfg(target_os = "macos")'.dependencies]`: re-add
  `llama-cpp-2 = { features = ["metal"] }`.

cargo unions per-crate feature sets, so macOS still builds with
`["sampler","metal"]` (verified: native rebuild was a 0.89s no-op — same
artifact, metal intact). Linux gets CPU-only llama, and the Rust API
surface in `llm/mod.rs` is identical either way (`with_n_gpu_layers`
just no-ops without a GPU backend).

### Could NOT run a real Linux build here

This macOS dev box has **no Linux rust target and no cross-linker**
installed (`rustup target list --installed` shows only apple/wasm
targets; no `*-linux-gnu-gcc`). So `cargo check --target
x86_64-unknown-linux-gnu` is not runnable locally — it would fail at the
linker/C-dep stage for lack of a toolchain, not for a code reason. **CI
(daemon-binaries.yml) is the correct + only place to actually exercise
the Linux build.** This matches the task's stance: native macOS is the
must-verify; Linux is best-effort, CI-exercised.

### Remaining Linux-port risk to watch when CI first runs

These are *expected to compile* (cfg-gated or cross-platform crates) but
have not been runtime-verified on Linux:

- **ggml/llama-cpp-2 CPU build** — needs `cmake` + `clang` on the runner
  (workflow installs them). First Linux compile of the C++ may surface
  warnings-as-errors or a missing dep; adjust the apt list if so.
- **git2 `vendored-openssl`** + **reqwest rustls-tls** — vendored, should
  be self-contained; `libssl-dev` installed as belt-and-suspenders.
- **alacritty_terminal / portable-pty / rustix / mio / signal-hook** —
  cross-platform but PTY semantics differ; compile should pass, runtime
  PTY behavior on Linux is untested.
- **ngrok 0.18** — cross-platform crate; fine to compile. (Headless
  servers will more likely use the FRP-based K2 Connect path than ngrok,
  but that's a runtime config concern, not a build one.)
- **`#[cfg(target_os = "linux")]` fallbacks are stubs in places** (e.g.
  heartbeat install writes systemd units) — compiles, but the headless
  install/uninstall flow on Linux is not end-to-end tested.
- **aarch64-linux cross build** — second-order; the gnu cross toolchain
  is wired in the workflow but unproven.

None of these block P1. They're the follow-up checklist for the first
green CI run + a real headless smoke test (boot daemon on a Linux box,
hit `/cli/*`).

## Verify log (this worktree)

- `CARGO_TARGET_DIR=/tmp/k2so-p1-target cargo build --release --bin
  k2so-daemon` → SUCCESS, `Mach-O 64-bit executable arm64`, 30 MB.
- Re-build after the `metal` cfg change → 0.89s no-op (macOS feature
  union preserved metal; artifact unchanged).
- `bash -n scripts/release.sh` → OK.
- `daemon-binaries.yml` → parses via `python3 yaml.safe_load`, ruby, node.
- Signed the locally-built `k2so-daemon-macos-aarch64` with a THROWAWAY
  tauri keypair (real key password not available in worktree) → produced
  a valid base64 minisign `.sig` with `file:k2so-daemon-macos-aarch64`
  trusted comment; sample manifests (macos-only + complete) are valid
  JSON. The real release signs with `~/.tauri/k2so-updater.key`.
