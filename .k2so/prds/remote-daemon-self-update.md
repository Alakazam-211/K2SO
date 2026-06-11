# PRD: Remote daemon self-update ("Update this machine" over K2 Connect)

**Status:** Draft / parked (design only — not yet scheduled)
**Owner:** Rosson
**Created:** 2026-06-05
**Area:** K2 Connect / software updates / daemon lifecycle
**Related:** #638 (server-capability/version cache), #629 (connect-user roles), #626 (LaunchAgent DMG-path staleness), #646 (tunnel survives restart — prerequisite for this to even be reachable)

---

## 1. Problem

When you're connected to a remote host and open **General Settings → Software Update**, "Download" and "Install & restart" act on the **local** Mac, not the remote you're driving. So there is no way to update a remote machine from the app — you have to physically go to it. We want **"update the machine I'm connected to"** so a fleet of remote K2SO hosts can be kept current from one place.

## 2. Why it's local today (current state)

- The entire update flow lives in the renderer's `useUpdateStore` (`src/renderer/stores/update.ts`) and is built on **`@tauri-apps/plugin-updater`** — `check()`, `downloadAndInstall()`, then `invoke('relaunch_via_open')`. It is **not host-aware**: `activeHost` is never read, and the plugin is hardwired to the running app binary at `/Applications/K2SO.app` on *this* Mac.
- **The daemon has zero self-update capability.** No `/cli/*` route downloads, verifies, swaps, or restarts a build. The only version-aware surface is read-only: `/boot-status` (version/protocol) and `/cli/whats_new`.
- So updating a remote is **not** "point the existing flow at the remote daemon" — the daemon half has to be built from scratch.

## 3. Goal & non-goals

**Goal:** From the app, while connected to a remote host, check the remote's version against the latest release and **download → verify → install → restart** the remote's K2SO (app bundle + bundled daemon), with progress + a safe rollback, gated to privileged users.

**Non-goals (v1):**
- Cross-platform. K2SO ships **darwin-aarch64 only**; remote hosts are Macs. Linux/Intel are out of scope until those builds exist.
- Auto-update of remotes (no silent background updates). v1 is **operator-initiated**.
- Updating the local machine — that already works via the Tauri plugin and stays as-is.
- Fleet/batch "update all remotes at once" — single active host in v1.

## 4. The risk that shapes the whole design

**A botched remote update can brick remote access**, and because it's remote you can't walk over to fix it. Every design choice below exists to make a failed update **non-fatal**: verify before touching anything, keep the old bundle, swap atomically, and confirm the new daemon actually came back before declaring success — otherwise roll back.

## 5. Release artifacts (already published by `scripts/release.sh`)

Per release on GitHub: `K2SO.app.tar.gz` (gzipped notarized app), `K2SO.app.tar.gz.sig` (**minisign** signature for the Tauri updater), `latest.json` (updater manifest), and the DMG. The daemon self-update consumes the **same** `K2SO.app.tar.gz` + `.sig` + `latest.json` the Tauri plugin uses. The minisign **pubkey is already in `src-tauri/tauri.conf.json`** (`plugins.updater.pubkey`) — the daemon must verify against that same key.

## 6. Design

### 6.1 Daemon routes (new, POST, owner/admin-gated, method-guarded)

All under `/cli/daemon/update/*`. Each is a mutating route → must include the `if !is_post { 405 }` guard (per `feedback_post_only_route_guards`) and the owner/admin authz check (per #629/#631 `can_act_on`).

1. **`POST /cli/daemon/update/check`** → `{ current, latest, available, notes, url }`
   Fetches `latest.json` from the configured endpoint, compares to the running version. Pure read; no side effects. (Renderer could also derive `available` from the cached `serverVersion` + the local Tauri `check()`'s `latest`, but a daemon-side check keeps the remote authoritative about what *it* can reach.)

2. **`POST /cli/daemon/update/start`** `{ version }` → `{ job_id }`
   Kicks off an async job: download `K2SO.app.tar.gz` + `.sig` to a temp dir under `~/.k2so/update/<job_id>/`, **verify the minisign signature against the bundled pubkey** (mandatory — abort hard on mismatch), then **stage** the extracted `.app` alongside (do NOT touch `/Applications` yet). Job runs on the blocking pool; never blocks the HTTP thread.

3. **`GET /cli/daemon/update/status?job_id=`** → `{ phase, progress, bytes, error? }`
   `phase ∈ downloading | verifying | staged | applying | restarting | done | failed | rolled-back`. Renderer polls this (mirrors the local plugin's progress events; no new WS needed).

4. **`POST /cli/daemon/update/apply`** `{ job_id }` → `{ ok }`
   Only valid when `phase === staged`. Spawns a **detached helper** (see 6.2) and returns immediately. The daemon expects to be killed + relaunched out from under this call, so the response is "handoff accepted", not "done".

> Splitting `start` (download+verify+stage) from `apply` (swap+restart) means the irreversible step is a separate, explicit click after verification has already succeeded — and the renderer can show "ready to install" exactly like the local flow.

### 6.2 The detached helper (the only thing that touches `/Applications` + restarts)

The daemon cannot cleanly replace itself while running, so `apply` writes + spawns a **detached** shell/helper (same pattern as `relaunch_via_open()` in `src-tauri/src/commands/settings.rs`) that:

1. Waits for a health token / for the current daemon to be reachable, snapshots the current version.
2. **Backs up** `/Applications/K2SO.app` → `/Applications/K2SO.app.bak-<version>` (rollback source).
3. **Atomic swap**: move the staged `.app` into place via rename on the same volume (`/Applications/K2SO.app.new` → `K2SO.app`), so there's never a half-written bundle.
4. Re-point + reload the LaunchAgent: ensure the plist records the canonical `/Applications/K2SO.app/Contents/MacOS/k2so-daemon` path (avoid the #626 DMG-mount-path staleness), then `launchctl kickstart -k gui/<uid>/com.k2so.k2so-daemon` (KeepAlive=true respawns it).
5. **Health-check**: poll the new daemon's `/boot-status` for up to N seconds; require `phase=ready` AND `version === target`.
   - **Success** → delete the `.bak`, write a `update-result.json` the new daemon surfaces.
   - **Failure** (won't come back, wrong version, or timeout) → **roll back**: swap `.bak` back into place, kickstart again, record `phase: rolled-back` + the reason.

Because the helper is detached and owns the swap+restart+rollback, a crash of the *daemon* mid-update can't leave `/Applications` half-updated, and a bad new build self-heals to the prior version.

### 6.3 Renderer changes (host-aware update flow)

- `useUpdateStore` / `GeneralSection.tsx` branch on `activeHost`:
  - **local** → unchanged (Tauri plugin).
  - **remote** → drive the daemon routes via `daemonCliPost`/`daemonCliGet`: `update/check` → show "update available (remote)" with the remote's current → latest; "Download" = `update/start` + poll `update/status`; "Install & restart" = `update/apply`, then show "restarting…" and reconnect/health-poll until the host returns on the new version (or surfaces a rollback).
- Gate the remote install UI behind `serverSupports('remote-self-update')` (add to the #638 `FEATURES` map at this feature's min version) so older remotes show a clean "update this host from its own machine" message instead of a dead button.
- Copy must make it unmistakable **which machine** is being updated ("Update **Hetzner box** to 0.39.X") — the current ambiguity is the whole bug.

### 6.4 Authorization

Owner/Admin only (connect-user roles, #629). Replacing a machine's binaries + restarting it is privileged; a Member connection must not be able to do it. Enforce in the route handlers via the existing `can_act_on` matrix; add a regression test (mirrors #631).

## 7. Security

- **Signature verification is mandatory and load-bearing.** The remote downloads over the TLS tunnel, but the relay is semi-trusted infrastructure — the daemon MUST verify the minisign `.sig` on `K2SO.app.tar.gz` against the **bundled** pubkey (the same key in `tauri.conf.json`) before extracting or swapping. A mismatch aborts with no filesystem change. This is the single most important control: without it, a compromised relay/MITM could push an arbitrary binary onto every remote.
- Pin the download host to the GitHub releases domain; reject redirects off it.
- The staged bundle must be notarized (it is — release.sh notarizes both the tar.gz and DMG); optionally re-verify the staged `.app`'s codesign/notarization on the remote before swap.

## 8. Phasing

- **Phase 0 (tiny, ship first / independent):** make the update UI **host-aware read-only** — when on a remote, show the *remote's* version + whether an update exists, and explicitly state remote install isn't available yet. Removes the misleading "it updated my laptop instead" behavior immediately, with zero daemon work. (This is option C from the design discussion.)
- **Phase 1:** daemon `update/check` + `update/start` + `update/status` (download + verify + stage) + the renderer download flow. No swap yet — ends at "verified & staged".
- **Phase 2:** `update/apply` + the detached helper (backup → atomic swap → kickstart → health-check → rollback) + the renderer "install & restart + reconnect" flow. This is the irreversible, brick-risk part — land it with the most care + a real two-Mac test.
- **Phase 3 (later):** batch "update all my remotes", and pre-flight ("N hosts on old version") once multi-host is common.

## 9. Open questions

- **Self-update vs. a tiny separate updater binary.** Using the existing `relaunch_via_open` detached-script pattern is the lowest-friction path. A dedicated, separately-versioned `k2so-updater` helper would be more robust (its own code doesn't get swapped mid-flight) but is more to build/ship. Lean: detached script for v1; revisit if it proves fragile.
- **Where does the remote get `latest.json`?** Daemon fetches directly from GitHub (simplest, remote is authoritative about reachability) vs. the client passing the manifest it already fetched (works even if the remote can't reach GitHub, but trusts the client). Lean: daemon fetches + verifies; client-supplied manifest as a fallback.
- **Disk + time on the remote.** ~tens of MB temp + a full `.app` copy; health-check timeout budget. Need sane defaults + cleanup of `~/.k2so/update/*` and stale `.bak`s.
- **Concurrent/last-writer:** refuse a second `update/start` while a job is non-terminal; one update at a time per host.

## 10. Success criteria

From the app, connected to a remote Mac running an older K2SO: see "update available" for **that host**, click through download → verify → install, watch it restart, and have it come back **on the new version** — with a verified failure path that **rolls back to the prior version and stays reachable** if the new build doesn't come up. Owner/Admin only. Local update behavior unchanged.

## 11. Scope expansion (2026-06-05, Rosson): headless CLI install + standalone daemon distribution

**New requirement:** the server will be installable via CLI on **headless servers** (no desktop, no Tauri app). So download / install / relaunch must work reliably for **two clients**: (a) the **`k2so` CLI tool** on a headless box, and (b) **people logged into a remote host over K2 Connect**. Both ultimately drive the same daemon primitives.

**The core gap this exposes:** today the daemon ships ONLY inside the macOS Tauri bundle (`K2SO.app/Contents/MacOS/k2so-daemon`); §5/§6 assume a `.app` to swap and `launchctl`. A headless server has no `.app`, no `/Applications`, and no launchd — it runs a **standalone `k2so-daemon` binary under systemd**. So the prerequisite is **standalone, per-platform daemon artifacts** published every release, independent of the Tauri app:

- `release.sh` must also build + publish `k2so-daemon-<os>-<arch>` (macos-aarch64, linux-x86_64, linux-aarch64) + a **minisign `.sig` per artifact** (same key as `tauri.conf.json` updater pubkey — Linux has no notarization, so minisign verification is the load-bearing control there too) + a `daemon-latest.json` manifest.

**Two install/update shapes (one shared relaunch primitive):**
- **Shape A — macOS desktop (Tauri app present):** the §6 flow (download `K2SO.app.tar.gz`, verify, atomic-swap `/Applications/K2SO.app`, relaunch). Unchanged.
- **Shape B — headless server (no app):** standalone binary under systemd. `k2so daemon install` = curl|sh installer (ties into #614: curl|sh + brew tap) that drops the verified `k2so-daemon` binary + writes a systemd unit (`Restart=always`) + first-run pairing to K2 Connect. `k2so daemon update` = download `daemon-latest.json` → fetch the matching `k2so-daemon-<os>-<arch>` + `.sig` → **verify minisign** → atomic-rename the binary in place → trigger restart. Backup the prior binary for rollback; health-check `/boot-status` for `version === target`, else roll back.

**Supervisor-agnostic relaunch primitive (#659, building now):** the restart route triggers **graceful shutdown → process exits → the supervisor respawns** (launchd `KeepAlive=true` on macOS, systemd `Restart=always` on Linux). It deliberately does NOT shell out to `launchctl`, so it serves BOTH shapes and is the shared foundation under "install/update → relaunch". This is Phase 0 of the whole effort.

**Authz / auth over Connect:** owner-only for restart (§6.4 owner/admin for update). A headless box paired to K2 Connect needs a way for the owner to authorize update/restart over the tunnel (owner token, or an owner-scoped connect session) — resolve alongside #629 roles.

**Revised phasing (supersedes §8 ordering):**
- **P0 — restart primitive** (`POST /cli/daemon/restart`, supervisor-agnostic) + `k2so daemon restart --host`. *(#659, in progress.)*
- **P1 — standalone daemon artifacts** in `release.sh` (per-OS binary + minisign `.sig` + `daemon-latest.json`). Nothing consumes them yet; just publish.
- **P2 — headless CLI install** (`k2so daemon install`, curl|sh + brew tap, systemd unit, pairing) — overlaps #614.
- **P3 — update path** (`update/check|start|status|apply`) shared by CLI (Shape B binary-swap) and the Connect-remote renderer (Shape A app-swap), reusing the P0 relaunch primitive.
- **P4 — Connect-remote "update this machine" UX** (the original §6.3 renderer flow) + batch/fleet (§8 Phase 3).

**New open questions:**
- Standalone Linux daemon: which feature set ships headless (no GPU terminal renderer / no local-LLM Metal path on Linux)? Likely a `--headless`/server build profile of `k2so-daemon` without the macOS-only crates.
- First-run pairing of a headless box to a K2 Connect account from the CLI (token bootstrap) — needs design.
- Brew tap + curl|sh hosting (k2.dev/install?) and how it verifies the download (minisign in the installer script).
