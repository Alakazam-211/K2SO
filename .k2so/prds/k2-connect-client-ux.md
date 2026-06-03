# PRD: K2 Connect — desktop client UX (server switcher + remote sign-in)

**Status:** Design / single source of truth — no feature code yet (planning doc).
**Author:** pod-leader (design w/ Rosson, 2026-06-03)
**Repo boundary:** This is **client** code → it lives in the **public K2SO (MIT)** repo. The desktop client can target ANY daemon (local, self-hosted, or hosted K2 Connect). Only the server control plane is the proprietary `../k2-connect` repo. This PRD is the open-core boundary working as designed.
**Depends on / unblocked by:** nothing in Cloudflare — develop + test against a second local daemon as a fake "remote"; the live `<sub>.k2.dev` connection slots in once the tunnel is up.

---

## 1. The UX model (locked w/ Rosson 2026-06-03)

- **Settings → Connections is the address book.** Users set up / edit / remove the K2 servers they connect to here. Each server entry has a **"Remember password"** toggle.
- **Top-bar server switcher** (the always-visible control): lists **"This Mac"** (local bundled daemon, always first, never needs auth) + every saved server + **"Add a server…"**. "Add a server…" routes the user to **Settings → Connections** (we do NOT add inline in the dropdown).
- **Selecting a saved server:**
  - Token **remembered** → **silent auto-sign-in** + reconnect, *even after the previous session expired*.
  - Token **not remembered** (or remembered-but-rejected/expired) → **full-screen sign-in** for that specific server (pre-filled label + address, focus the password field), then connect.
- **"Remember password"** → the token is stored in the **OS keychain** (macOS Keychain via Tauri), NOT in plaintext `connect-hosts.json`. The address-book JSON holds everything *except* the secret. "Don't remember" → token held in memory for the session only; re-prompt (full-screen) on reselect or expiry.

## 2. Smoothness layer (the polish)

1. **Keychain-backed remember** (above) — secure auto-sign-in.
2. **Soft reconnect, not a blank screen.** The current ConnectionGate hard-blanks while (re)connecting (correct for the local auto-update race). For a **remote** that briefly drops, render a dimmed "Reconnecting to `<host>`…" overlay over the last view, with backoff — don't blank the app.
3. **"You're on a remote" cue** — a `🌐 <host>` chip / subtle top-bar tint whenever the active host is remote. Remote = a *different filesystem*; this prevents "why did my files change?" confusion.
4. **Graceful expiry → targeted re-auth.** A rejected remembered token drops into the full-screen sign-in for *just that one server*, place preserved.
5. **Per-server status dot + latency** in the switcher and Settings (connected / connecting / offline).
6. **Forward-compat for real accounts.** Token-paste now; "Sign in with K2 account" slots into the same sign-in page once the k2.dev dashboard/Stripe exists (issues per-device tokens). Optional "reconnect to last server on launch" setting; default = start on This Mac.

## 3. Architecture seams (from the connection-layer map, verified)

- **Host is hardcoded `127.0.0.1`** in `getDaemonWs()` — `src/renderer/kessel/daemon-ws.ts:52` — and WS URLs at `src/renderer/stores/session-events.ts:162`. → make host-aware (read from the active-host store).
- **`AcceptancePolicy` is already pluggable** — `src/renderer/components/ConnectionGate.tsx:64,150` (today injects `localPairedPolicy` = exact-version match). → inject a **remote policy** that range-checks `protocol` instead of exact version.
- **`boot_status` ships `PROTOCOL = 1`** — `crates/k2so-daemon/src/boot_status.rs:36` — built for exactly this range-check. Route: `routes/dispatcher.rs:361`.
- **Top-bar mount** — `src/renderer/components/TopBar/TopBar.tsx` (~`:88`, after the logo).
- **`ConnectHost` schema** — specced in Phase 3 PRD §E.3: `{label, hostname, port, token, cert_fingerprint, last_connected_at}` → `~/.k2so/connect-hosts.json`. **Correction:** the `token` does NOT live in the JSON — it goes to the keychain; the JSON keeps a `remember: bool` + keychain ref.
- **Auth** — single global token rides as a `?token=` query param (`routes/http.rs:16`). TLS-safe over the tunnel (Caddy terminates `*.k2.dev`). Direct-IP non-TLS remotes are LAN/trusted-only until per-client tokens + TLS land (Phase 3 Workstream B).

## 4. Build order (all testable now vs a 2nd local daemon)

1. **Foundation** — `ConnectHost` type (TS + Rust) + `connect-host` zustand store (`activeHost: 'local' | ConnectHost`, `hosts[]`, `selectHost()`) + make `getDaemonWs()` host-aware. **`activeHost === 'local'` behaves identically to today — purely additive, shippable alone.**
2. **Top-bar switcher** — This Mac + saved servers + "Add a server…" → Settings.
3. **Settings → Connections** — address book; add/edit/remove; "Remember password" → keychain.
4. **Full-screen sign-in** (unsaved/expired) + **remote `AcceptancePolicy`** + **soft-reconnect overlay** + remote cue.

## 5. Sequencing note
Build starts **after the canonical-agents Wave 2 frontend is cherry-picked** — both touch the Settings sections area (Wave 2 renames AgentSkillsSection → "Canonical Agent Flow"; this adds a Connections section). Avoid concurrent-frontend cherry-pick conflicts.
