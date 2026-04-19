## 0.32.12 — Companion security pass

A defense-in-depth pass on the companion HTTP+WS server, benchmarked against three reference implementations — **code-server**, **jupyter_server**, and **vaultwarden** — cloned side-by-side and audited for the patterns most applicable to K2SO's shape (local Rust app + ngrok tunnel + mobile companion + argon2 password auth + bearer sessions).

Nothing in the threat model changed. The companion surface has always been: a tunnel URL on the public internet, a single operator-chosen password, a pool of bearer tokens, and a set of HTTP + WS endpoints. What changed is that each of the layers now follows the pattern the reference projects have been running in production for years — and the most damaging endpoint is opt-in rather than opt-out.

Rollback: `v0.32.11` is tagged and its notarized DMG is still live on GitHub Releases. Skip the auto-update to remain on 0.32.11, or `git reset --hard v0.32.11` + rebuild for a source-level revert.

### Brute-force + timing

- **Per-IP rate limit on `/companion/auth`** — 5 attempts/minute, 20 attempts/hour, tracked by `X-Forwarded-For` (which ngrok reliably sets) with a fixed-window counter. Hitting the threshold returns `429` with `retryAfterSeconds` so the mobile app can back off cleanly. Mirrors code-server's token-bucket login limiter.
- **Constant-time bearer token compare**. The session HashMap lookup is replaced with an O(n) scan using `subtle::ConstantTimeEq` against each active token's bytes. `n` is bounded to a handful of sessions in practice; the theoretical timing channel from hash-bucket collisions + byte-wise `String` equality on the fallback compare is closed. Matches vaultwarden's `ct_eq` pattern.

### Origin, Host, and CORS — three different controls, explicitly

- **CORS allowlist replacing the wildcard.** The old `Access-Control-Allow-Origin: *` is gone — every response now reflects a specific Origin only if it matches an entry in the new `companion.corsOrigins` allowlist (Settings → Companion). Empty by default: native mobile apps don't enforce CORS so they're unaffected, and any rogue webpage the user opens in a browser is now blocked at the CORS layer. Matches vaultwarden's strict-allowlist + `Vary: Origin` + `Access-Control-Allow-Credentials: true` pattern.
- **WebSocket Origin check on upgrade.** Before handing the stream to `tungstenite`, the upgrade request's `Origin` is validated against the tunnel URL + allowlist + loopback. Missing Origin is allowed (native iOS/Android clients don't set one). `https://attacker-example.com` gets a plain HTTP 403 before the handshake ever starts. Covers the WS surface that CORS enforcement famously doesn't. Adds a subdomain near-miss test so a future naive `.contains()` regression can't slip through.
- **Host header validation (DNS-rebinding defense).** New `host_allowed()` runs as the first gate in every request: `Host` must be loopback, the live tunnel URL, or an allowlist entry. An attacker DNS-pointing `evil.com` at the user's ngrok IP to smuggle requests under a spoofed `Host` now gets a 403. Port stripping handles `:443`/`:8080` cleanly; bracketed IPv6 hosts work end-to-end. Same pattern Jupyter ships by default (`ServerApp.local_hostnames`).

### Session lifecycle

- **Real logout surface.** New `POST /companion/auth/revoke` plus WS method `auth.revoke`, both authenticated, both purge the caller's session and kick any WS clients bound to that token. Idempotent (safe to retry). code-server and jupyter both only clear the client cookie; vaultwarden has real JWT revocation — we land somewhere in between: the HashMap is authoritative, tokens die immediately on revoke.
- **Password rotation invalidates all sessions.** `settings_update` now diffs `companion.username` and `companion.passwordHash`; on any change, every active bearer token is nuked and every WS client disconnected. Same on `settings_reset`. Closes the window where a rotated password would leave old 24h tokens valid.

### Secrets at rest

- **Password hash → macOS Keychain.** The argon2 hash is now stored in the user's login keychain under service `K2SO-companion-auth`. `companion_set_password` writes to Keychain first; `settings.json` retains only a `passwordSet: true` flag so the UI keeps working without a keychain read on every render. Legacy on-disk hashes from pre-0.32.12 installs are read once, copied into Keychain, then cleared from disk — fully automatic, no migration dialog. A `settings_reset` also wipes the Keychain entry so a reset can't leave an undiscoverable credential behind.
- **`~/.k2so/settings.json` chmod'd to `0o600`.** Every write goes through an atomic tempfile + rename, then `set_permissions(0o600)`. `read_settings` tightens the mode on any pre-0.32.12 file that was wider. Matches Jupyter's `secure_write` pattern. The ngrok auth token still lives in settings.json (Keychain for secondary secrets adds round-trip cost), but now behind owner-only read.

### Privileged endpoint gating

- **`/companion/terminal/spawn` is now opt-in.** The two arbitrary-command endpoints (`terminal/spawn`, `terminal/spawn-background`) and their WS equivalents (`terminal.spawn`, `terminal.spawn_background`) are gated behind a new Settings toggle: **Companion → Allow Remote Spawn**, default OFF. With the toggle off, requests return 403 with a message pointing at the setting. The rationale: these endpoints give a bearer-token holder arbitrary code execution on the Mac. No reference project (code-server, jupyter, vaultwarden) has an equivalent unrestricted-exec endpoint — they all scope execution to specific contexts (IDE terminals, kernel cells, vault CRUD). Default-off caps the blast radius of a token compromise to the read-only surface. Users who rely on the endpoint flip it on once; users who don't are now protected without having to know the endpoint existed.

### Defense in depth

- **Security response headers on every response.** `X-Frame-Options: DENY`, `X-Content-Type-Options: nosniff`, `Referrer-Policy: no-referrer` on every HTTP response from the companion. The API serves JSON and is never meant to be framed; `no-referrer` ensures the tunnel URL doesn't leak via outbound `Referer` headers. Vaultwarden's fairing pattern, trimmed down — full CSP isn't meaningful for a JSON API.
- **Prominent startup warning when the tunnel activates.** Log line and `companion:tunnel_activated` event now include the public URL, `remote_spawn` state, and CORS allowlist contents. Operators can `grep '[companion]'` to verify what's actually exposed without reading source.

### Threat-model deltas

| Attack | 0.32.11 | 0.32.12 |
|---|---|---|
| Brute-force password over tunnel | ~10 guesses/sec (argon2 delay only) | 5/min + 20/hr per IP, 429 with Retry-After |
| Stolen bearer token → read data | works for 24h | works for 24h (unchanged) |
| Stolen bearer token → run arbitrary shell | works if companion enabled | blocked unless `allow_remote_spawn` explicitly on |
| DNS rebinding via attacker domain | no Host check | blocked at the first gate |
| Malicious browser tab reading companion JSON | blocked by Authorization header, but CORS wildcard gave it a window if cookies ever appeared | CORS allowlist empty by default |
| Cross-origin WS connection | accepted, token in query string | Origin validated against tunnel + allowlist + loopback |
| Rotated password, old token still valid | works for ~24h until natural expiry | session invalidated at the moment of rotation |
| `settings.json` read by another user's process | world-readable (umask-dependent) | `0o600`, argon2 hash no longer on disk |
| Unauthorized logout surface | none | `POST /companion/auth/revoke` + WS `auth.revoke` |

### Reference projects

Side-by-side audit against three open-source servers with similar shape:

- **coder/code-server** — TypeScript, remote IDE over HTTP+WS. Adopted the login rate-limit pattern and the pattern of rejecting cross-origin WS upgrades at the Express middleware layer before the handshake. code-server stores its config in umask-dependent mode; we do better with explicit `0o600`.
- **jupyter/jupyter_server** — Python, the gold standard for local-server auth + DNS-rebinding defense. Adopted `check_host()`-equivalent as the first handler gate, `secure_write`-equivalent chmod pattern, and the idea of emitting a prominent startup warning on public bind.
- **dani-garcia/vaultwarden** — Rust, argon2id + JWT + mobile clients. Adopted `subtle::ConstantTimeEq` for token comparisons, reflected-origin CORS with `Vary: Origin`, and the fairing-style security-response-header pass. Notable non-adoption: we did *not* migrate sessions to JWT (vaultwarden needs JWT because they run distributed workers; K2SO is single-process and JWT buys complexity without a corresponding security win in our model).

### Tests

- **184 Rust unit tests** pass (↑25 from 0.32.11) — 19 CORS/Origin/rate-limit tests, 6 Host-header tests.
- **424 tier3 source assertions** pass.
- **111 CLI integration tests** pass.
- Clean build, no new compiler warnings.
