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
3. **"You're on a remote" cue** — a `🌐 <host>` chip / subtle top-bar tint whenever the active host is remote. Remote = a *different filesystem*; this prevents "why did my files change?" confusion. The chip carries the live latency readout (below).
4. **Graceful expiry → targeted re-auth.** A rejected remembered token drops into the full-screen sign-in for *just that one server*, place preserved.
5. **Live latency readout in the top bar.** Show round-trip ping to the active remote (e.g. `42 ms`) right in the top bar beside the host chip, color-coded (green < ~80 ms · amber ~80–200 ms · red > ~200 ms). This is the at-a-glance answer to "**why does it feel slow?**" — a bad tunnel/route shows up as a red ping rather than a mysteriously sluggish app, so users blame the network, not K2. Also surface the dot + latency per-server in the switcher and Settings (connected / connecting / offline). **This Mac** = sub-ms, shown as "local" or hidden.
   - **Measurement:** prefer a lightweight **WS ping frame** RTT (reuses the live socket, no HTTP overhead, and reflects the *actual* data-path latency the user feels); fall back to periodic `/boot-status` RTT. Cadence every few seconds; show a rolling value, not jittery per-sample. The tunnel path (frpc→frps→Caddy) adds real hops, so this readout genuinely reflects the hosted-tier experience.
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

## 6. The "K2 Connect" Settings section — the HOST / expose side (NEW, Rosson 2026-06-03)
This is the OTHER direction from the switcher/Connections (§1–5, which is the *client* — connecting OUT to remote servers). **"K2 Connect" is where a device logs into its K2 Connect account and exposes ITS OWN server at a subdomain it owns** — the GUI front-end for the daemon tunnel connector (`k2so tunnel start/stop/status`, `~/.k2so/tunnel.json`, routes `GET /cli/tunnel/status` · `POST /cli/tunnel/start?subdomain=` · `POST /cli/tunnel/stop`).

- **New Settings section "K2 Connect"** (sibling of "Connections"). Nav id stable.
- **Account login** to K2 Connect → yields the bearer token used in the frpc `[metadatas].token`. Users may **own MULTIPLE subdomains** under `k2.dev` (e.g. `rosson`, `rosson-work`).
- **Subdomain selector** — pick which owned subdomain this server registers as. Server canonicalizes to `{user}` today; multi-subdomain ownership requires the control plane to map a token → set of allowed labels (server work, below).
- **Configure the connection layer** — server addr (default `178.156.232.105:7000`), exposed local port (the daemon's), **Start / Stop tunnel**, live **status** + the public URL `https://<sub>.k2.dev`.
- **MVP (buildable now, no server changes):** manual **token + subdomain** entry written to `tunnel.json`, + Start/Stop/Status wired to the existing `/cli/tunnel/*` routes. Surfaces the "frpc not installed" error from the connector with an install hint.
- **Full (DEPENDS ON PROPRIETARY CONTROL-PLANE API — not built):** real account login + **list of owned subdomains** needs control-plane `/account` + `/subdomains` endpoints (multi-subdomain ownership, billing). Flag as the server-side follow-up; the MVP token+subdomain form stands in until then.
- **Companion rename (done):** "Mobile Companion" → **"K2 Companion"** (prep for the LocalXpose BYO migration).

### 6.1 One expose plane — K2 Companion collapses to "download + pair" (Rosson 2026-06-03)
K2 Companion (mobile) and K2 Connect (desktop-remote) reach the **same daemon over the same tunnel** (`<sub>.k2.dev`). So the **server/tunnel config lives ONCE** on the K2 Connect host page (§6). Therefore:
- **K2 Companion settings page → collapses to "Get the mobile app" (App Store / Google Play links) + a pairing affordance** (the `<sub>.k2.dev` + token already set in K2 Connect; QR/code to pair). **No separate tunnel config on the Companion page.**
- **Retire the legacy standalone ngrok Companion tunnel** (`CompanionSection.tsx` "Enable Companion" ngrok lifecycle) — the mobile app rides the unified K2 Connect tunnel.
- **BYO / LocalXpose** = a tunnel-*provider* option on the same expose plane, NOT a separate Companion tunnel.
- Mental model: **expose once → reach it from desktop (K2 Connect switcher) or phone (K2 Companion app).** Task #615. (Separate from the in-flight Connections + K2-Connect-host-page build.)

### 6.2 Account / billing model + the TWO-layer auth split (Rosson 2026-06-03)
**Hosted-tier account flow:** (1) sign up for a profile on **k2.dev (web)**; (2) purchase **one or many subdomains @ $2.99/mo/subdomain/tunnel**; each issues a token. (3) In the app's **K2 Connect page**, log into the k2.dev account → see your **purchased subdomains** → **select which subdomain binds this device's tunnel** → enable (writes token+subdomain to `tunnel.json`, starts frpc).

**Two auth layers, two owners (locked):**
- **Control plane (k2.dev, proprietary):** account, billing/Stripe, **subdomain ownership**, and **tunnel authorization** (frpc Login: do you own/pay for `<sub>.k2.dev`). Needs `/account`, `/subdomains`, billing endpoints — the host page's account-login + purchased-subdomain picker depends on this API (MVP manual token+subdomain stands until it lands).
- **K2 Daemon (the user's server):** the **allowed users/usernames + their tokens/scopes that may CONNECT to & drive the daemon** — daemon-local access control ("who can use my agent server"). NOT the control plane's job. Today = single `daemon.token`; evolves to a daemon-managed user/token table (Phase 3 per-client tokens: label·scopes·revocable), surfaced in the app as a daemon **"Users / Access"** area, distinct from the K2 Connect account page.
- **Why the split:** the tunnel only transports bytes to the daemon; authorizing API access is the daemon's responsibility → keeps the access list private AND works identically for self-hosted/BYO tunnels (no k2.dev account required to manage your own daemon's users).
- **Pluggable transport, constant access (Rosson 2026-06-03):** the **TunnelProvider is swappable** — hosted K2 Connect (`k2.dev`), **BYO LocalXpose** subscription, or self-hosted FRP — but the **daemon-owned user/access list is CONSTANT across all of them.** Bring your own LocalXpose → skip the k2.dev account/billing entirely; the daemon still owns the users. Hosted-tier billing applies only when using the hosted tunnel. This is the existing pluggable-`TunnelProvider` seam.
