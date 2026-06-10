# PRD: Unified Remote Update (headless daemon + bundled app)

**Status:** FINAL 2026-06-06 — ready to execute. **Decision (Rosson):** build the COMPLETE unified updater (the 2–3-month "final shape": Shape B fix + Shape A + detection + gating) into **0.39.35** — *not* a P0-only band-aid. Manual fleet install of 0.39.35, then a small **0.39.36** release exists purely to **validate self-update end-to-end** (a self-updater can only be tested across two published versions). Root cause confirmed; deep-dive folded in.
**Owner:** Rosson + pod-leader.
**Supersedes/extends:** `.k2so/prds/remote-daemon-self-update.md` (Shape B only), #651, roadmap Shape-A follow-up.
**GH:** sig-contract bug filed (manifest/code mismatch on the signature step).
**One-liner:** One "Update host" button that correctly updates **both** install topologies — headless standalone `k2so-daemon` (Shape B, built) and bundled `K2SO.app` desktop hosts (Shape A, by remote-triggering the host's *existing* Tauri updater) — with automatic host-type detection and the real failure reason always surfaced.

---

## 1. Problem (observed + diagnosed 2026-06-06)

First real e2e of the remote self-updater (the 0.39.33→0.39.34 cycle — needs two published versions to exercise). From an **M1 desktop app**, connected to a remote **M5 desktop-app host** (`z3mbp` / `rosson.k2.dev`, on 0.39.33). Check correctly showed `0.39.33 → 0.39.34`; **Download failed before staging** — no "Install & Relaunch" ever appeared; UI showed only "Update failed — z3mbp is still on 0.39.33."

Captured on z3mbp (local owner token vs localhost): binary downloaded **fully** (`bytes: 30203904, progress: 1.0`), then `phase: failed, error: "download sig: request failed: builder error"`.

**Three confirmed defects:**

1. **CONFIRMED ROOT CAUSE — sig manifest/code contract mismatch (fleet-wide).** `release.sh` Step 8.5 writes the **inline** base64 signature into `daemon-latest.json`'s `sig` field (`MAC_SIG=$(cat …​.sig)` → `"sig": "${MAC_SIG}"`, release.sh:323/333) — copying the Tauri `latest.json` *inline-signature* convention. But the daemon **downloads** `artifact.sig` as a **URL** (`update_routes.rs` "download sig" step; the test `SAMPLE_MANIFEST` uses a URL). So the daemon hands the base64 blob to `reqwest::get()` → invalid-URL **builder error**, before verify. The real `.sig` asset exists and is reachable (416 B, HTTP 200) — it's just not referenced. The manifest is identical for every client ⇒ **this breaks daemon self-update (Shape B) for all 0.39.34 clients**, not host-specific. (Arch mismatch RULED OUT — M1 + M5 both `aarch64`.)
2. **The real error is discarded by the UI.** The daemon job carries `error` (set by `fail_job`, serialized in `/status`), but `UpdateStatusResult` (update-host.ts) lacks the field and `GeneralSection` renders only the generic phase copy. Every remote-update failure is a black box — the only reason we couldn't see #1.
3. **Wrong mechanism for app hosts (second wall behind #1).** Even with #1 fixed, the remote updater is **Shape B** — it swaps the *standalone* `k2so-daemon` (`current_exe()` rename + supervisor relaunch). A bundled host runs the daemon inside `K2SO.app/Contents/MacOS/`; that one-binary swap breaks the notarized bundle. Remote-updating a `.app` is **Shape A**, an `unimplemented!()` stub. So download/stage would now succeed on a desktop host and then dead-end at apply. The button must be **gated** for app hosts until Shape A ships.

## 2. Goal / end state

A single **Update host** control that:
- **Auto-detects the host's install topology** (standalone daemon vs bundled app) and picks the mechanism — the user never chooses.
- **Headless / standalone daemon** → **Shape B** (built): signed `k2so-daemon-<os>-<arch>` binary swap + supervisor relaunch.
- **Bundled `K2SO.app` host** → **Shape A done right**: the daemon **remote-triggers the host app's own Tauri updater**, which already downloads the notarized `K2SO.app.tar.gz`, minisign-verifies, swaps the bundle, and relaunches — *locally, today*. Relay its progress back over the tunnel. **No hand-rolled `.app` swap.**
- **Always surfaces the real failure reason** in the UI.
- Covers every shipped arch for both artifact families.

**Design principle:** reuse proven, notarized machinery per topology. The daemon binary swap and the Tauri app updater both already exist and are signature-verified; the work is *routing, detection, cross-process triggering, progress relay, and a manifest-contract fix* — not new swap logic.

## 3. Architecture

### 3.1 Host-type detection (daemon-reported)
- `installKind: "standalone" | "bundled-app"` from `std::env::current_exe()` — path contains `.app/Contents/MacOS/` ⇒ bundled, else standalone (corroborate with a build marker if path-sniffing proves brittle).
- Surface on `/boot-status` (dispatcher.rs:339, versioned handshake) **and** return from `/cli/daemon/update/check`, so the renderer's `serverSupports` layer can gate the UI.

### 3.2 Shape B — standalone daemon (BUILT; fix the manifest)
- Flow (unchanged, works): `check → start(download + minisign + sha256 + stage) → apply(atomic swap + supervisor relaunch) → /boot-status health-check → rollback`.
- **FIX (the unblocker):** `release.sh` must write `sig` = the `.sig` **asset URL** (`…/k2so-daemon-<key>.sig`), not the inline content — matching the daemon's downloader + the test contract. The daemon code is already correct (it expects a URL); the manifest was wrong.
  - *Forced by deployment:* the 0.39.33/0.39.34 fleet's downloader expects a URL and can't be changed without self-update (the broken thing). So the manifest must match the **deployed** code. Do NOT switch to "consume inline" — that only helps *future-fixed* clients, not the fleet.
  - Make `fetch_bytes`' redirect policy explicit (`Policy::limited(10)`) while here — robustness, not the bug.
- Add **INFO logging** to the download/verify/stage path (today it logs nothing) so future failures are visible server-side too.
- Confirm CI publishes all server arches; confirm the macOS standalone daemon launches post-download (Gatekeeper/quarantine xattr under launchd).

### 3.3 Shape A — bundled app (NEW = remote-trigger the Tauri updater)
The daemon and the bundled app run on the **same host**; the initiating client only watches status over the tunnel. So Shape A is a **daemon→co-located-app** signal, not a network swap:
- The daemon does **not** swap anything. It signals its co-located app to run the Tauri updater (download `K2SO.app.tar.gz` from `latest.json` → minisign verify → bundle swap → relaunch).
- **Channel (no daemon→app push exists today — core P1 build):** a new `POST /cli/daemon/app-update/*` the app subscribes/polls (or a daemon-events/WS event, or a job-file the app reads) + a new Tauri command `trigger_app_update_check()` driving `update.ts`, **relaying phases back into the daemon job** so the existing `/status` poll surfaces them through the same phase vocabulary.
- **App-not-running edge case:** if only the daemon is up, either (a) the daemon launches the app first, or (b) report "host app not running — start it or update on the machine." → **Decision (§6).**
- Windows: same model (Tauri updater handles NSIS/MSI) — forward, validate later.

### 3.4 Unified routes + UI
- `/cli/daemon/update/check` → `{current, latest, available, installKind}`.
- `/cli/daemon/update/start` + `/status` branch on `installKind`: Shape B stages a binary; Shape A kicks the app updater and relays phases through the **same** vocabulary (`downloading | verifying | staged | applying | restarting | done | failed | rolled-back`).
- UI (`GeneralSection` `UpdateHostRow` + `update-host.ts`): **surface `status.error`**; show `installKind` context; keep the host-named, "not this Mac" framing.

## 4. Rollout / fleet plan

The self-update path can only be *proven* across two published versions, so the rollout is: **build the whole thing in 0.39.35 → manual fleet install → validate on the 0.39.35→0.39.36 hop.**

1. **Now:** Rosson manually updates every fleet device to 0.39.34 (he has access to each). Sidesteps the broken 0.39.33→0.39.34 self-update; **0.39.34's broken manifest is NOT republished** (moot).
2. **0.39.35 = the complete unified updater** (everything in §5 A–F, incl. the `release.sh` sig→URL fix). **Rosson manually installs 0.39.35 on every machine** (so the whole fleet is on the fixed daemon + fixed manifest convention + Shape A-capable app).
3. **0.39.36 = a small validation release** whose entire purpose is to exercise self-update e2e:
   - **Headless / standalone daemon host** → remote "Update host" 0.39.35→0.39.36 (Shape B, fixed manifest). Must complete + health-check.
   - **Desktop / app host** (e.g. z3mbp) → remote "Update host" 0.39.35→0.39.36 (Shape A: remote-trigger the app's Tauri updater). Must complete + relaunch.
   - **Desktop host local** → the app's own "check for updates" still works (Tauri updater, `latest.json`).
   - This is the e2e coverage gap that let the original bug ship — it becomes a permanent gate.

## 5. Scope for 0.39.35 (the full final shape — build all of A–F)

- **A. Shape B manifest fix** — `release.sh` writes `sig` = the `.sig` **asset URL** (not inline); explicit `reqwest` redirect policy; **INFO logging** across download/verify/stage. *(The deployed downloader expects a URL; do NOT switch to inline.)*
- **B. Error surfacing** — add `error` to `UpdateStatusResult`; render the daemon's real error in `GeneralSection` on `failed`/`rolled-back`.
- **C. Host-type detection** — `installKind` from `current_exe()`, surfaced on `/boot-status` + `update/check`.
- **D. Routing + gating** — `update/start`/`status` branch on `installKind`; UI offers the right mechanism per host (no dead-ends).
- **E. Shape A** — daemon→co-located-app trigger (new `/cli/daemon/app-update/*` or WS event) + a `trigger_app_update_check()` Tauri command driving `update.ts`, **relaying phases back into the daemon job** so `/status` is uniform; resolve app-not-running (§6). **Verify `tauri-plugin-updater` is enabled in the 0.39.35 build — enable it if absent** (resolves the old open question by construction, since Rosson installs 0.39.35 manually).
- **F. Capability gate** — `serverSupports('remote-update-app', '0.39.35')` so a client only offers Shape A to hosts that support it.

**Deferred to follow-ups (P2/P3, not blocking 0.39.35→36 validation):**
- Arch/artifact coverage: add **mac x86_64** daemon (currently aarch64-only); Windows.
- Hardening: Gatekeeper/quarantine for the standalone mac daemon launched post-download; re-notarization edge cases; soak tests.

## 6. Open questions (all resolvable in-build — no external blocker)

1. **`tauri-plugin-updater` presence** — RESOLVED BY CONSTRUCTION: we **verify it's enabled in the 0.39.35 build and enable it if absent** (§5E). Since Rosson installs 0.39.35 manually fleet-wide, every machine ends up updater-capable regardless of what 0.39.33/34 shipped. No need to block on the team's machine inspection.
2. **App-not-running on a bundled host:** launch the app vs refuse-with-guidance — decide during §5E (lean: refuse with clear copy in v1; auto-launch is a follow-up).
3. **`installKind` robustness:** `current_exe()` path-sniff alone, or add a build marker (§3.1) — start with path-sniff, add marker only if flaky.
4. **Shape A channel choice:** new `/cli/daemon/app-update/*` route vs WS event vs job-file (§3.3) — pick during §5E from what the app↔daemon link already does reliably.
5. **Gatekeeper/quarantine** for the standalone mac daemon launched post-download (§3.2) — verify during validation; P3 if it needs work.

## 7. Tests

- Host-type detection: standalone path vs `.app` path → correct `installKind`.
- Shape B manifest contract: a generated `daemon-latest.json` has `sig` as a **URL**; daemon downloads + verifies it (regression lock for the root-cause bug).
- Shape B: download→verify→stage→apply→health→rollback seams (unchanged).
- Shape A: daemon emits the app-updater trigger; app runs updater; phases relay back; rollback on failure; app-not-running branch.
- UI: failed phase renders the real `status.error`; bundled host shows the right affordance pre-Shape-A.
- e2e: a real N→N+1 cycle on **both** a headless daemon and a bundled host (the coverage gap that let this ship).

## 8. Related
`.k2so/prds/remote-daemon-self-update.md` (Shape B origin), #651, #659/#660/#661 (P0 reboot). Code: `update_routes.rs` (sig download + `swap_shape_a_followup` stub), `scripts/release.sh` (Step 8.5 manifest gen, lines 323/333), `routes/dispatcher.rs` (`/boot-status` + update-route auth), `GeneralSection.tsx`/`update-host.ts` (UI + dropped `error`), `src/renderer/stores/update.ts` + `src-tauri/src/commands/updater.rs` (Tauri updater), `src/renderer/lib/server-capabilities.ts` (FEATURES gate). Tauri updater artifacts: `latest.json` + `K2SO.app.tar.gz` + `tauri.conf.json` plugins.updater.pubkey. Memory: [[project_0.39.33_remote_update_system]].
