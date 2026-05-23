# Phase 3: Contract Hardening + Mobile Companion + K2SO Connect

**Status**: Draft — Phase 2 is in flight (Wave A + B + 7b merged; Unit 7c + Phase 2.2 + Unit 4 still pending). Phase 3 begins when Phase 2 hits its "definition of done."
**Internal version markers**: 0.39.0g and beyond. **0.39.0 ships public** when Phase 3's Mobile Companion contract update lands and the companion app works against the hardened surface.
**Owner**: Rosson + pod-leader
**Date**: 2026-05-23

---

## tl;dr

Phase 2 made the daemon the source of truth. Phase 3 makes the daemon's contract surface **production-ready** for two new clients: the existing Mobile Companion (running on phones, connecting through ngrok) and the new K2SO Connect (Tauri thin-client on Machine A pointing at a daemon on Machine B).

Seven workstreams. None of them adds new features — they harden what Phase 2 built.

After Phase 3:
- Every `/cli/*` route's method, body shape, and auth scope are typed and enforced at the router level (no more per-handler `if !is_post { 405 }` backports).
- The ngrok tunnel becomes a mTLS-pinned channel; K2SO Connect can also skip ngrok entirely and use direct LAN/WAN with cert pinning.
- The auth model is per-client tokens with scopes (a Mobile Companion token can read but not exec; a K2SO Connect laptop token can do everything).
- `openapi.json` exports from the live route table; renderer + Mobile Companion + K2SO Connect codegen typed clients from it.
- Mobile Companion lives entirely on the new contract — every endpoint it hits is documented, versioned, and tested.
- K2SO Connect ships as a thin-client-only `.app` (no daemon binary in the bundle); pair it with any K2SO daemon on the network.

Public 0.39.0 ships when all seven workstreams land + Mobile Companion has been smoke-tested end-to-end against the hardened daemon.

---

## Entry criteria (Phase 2 must be done)

From `phase-2-daemon-headless-migration.md`'s "Definition of done":

1. ✅ Tauri can be force-killed and daemon continues to serve.
2. ✅ Daemon runs end-to-end without Tauri ever launched.
3. ⏳ `src-tauri/src/` ≤ 4,000 LoC (in progress; will hit target after Unit 4 + 7c).
4. ⏳ Every remaining `#[tauri::command]` is HOST-justified.
5. ⏳ `/cli/*` route table fully implemented + integration tested.
6. ⏳ Renderer has zero `invoke('...')` for workspace state.

Plus Phase 2.1 (CLI cleanup) and Phase 2.2 (schema hygiene) should land before Phase 3 starts so the surface being hardened is the final shape.

---

## Architectural goals

1. **One source of truth per concern.** The route table defines the contract. Method, body shape, response shape, auth scope all derive from one declaration.
2. **Client equality.** Renderer, Mobile Companion, K2SO Connect, `cli/k2so`, and any future third-party client all consume the same `/cli/*` surface. No client-specific endpoints.
3. **Network indifference.** A route behaves identically over local socket (Bundled K2SO), over LAN (K2SO Connect on the same network), over ngrok (Mobile Companion + K2SO Connect over the public internet). Auth + TLS handle the trust difference.
4. **Versioning is an explicit decision.** Breaking changes get new endpoints (`/cli/v2/<route>`); the old endpoint stays serving the old contract for at least one release cycle.

---

## Workstreams

### A. Method enforcement via typed router

**Problem**: Phase 2 shipped 30+ new `/cli/*` POST routes. Every one required a manual `if !is_post { 405 }` guard at the top of the handler. The guard got missed in Wave A (Units 2, 5, 7a all needed backports). The pattern doesn't compose — a typed router would.

**Solution**: replace `crates/k2so-daemon/src/main.rs`'s match-on-path dispatch with `axum` (or `poem`, or similar). Each route declares its method, body type, response type, and auth scope in the router definition. Method enforcement becomes structural: a GET on a POST-only route returns 405 before the handler is reached. No per-handler boilerplate.

**Migration path**: incremental. The current dispatch is 1,200+ lines of repetitive match arms. Move routes group-by-group (companion routes → axum module; LLM routes → axum module; etc.). Both dispatchers coexist during migration; final commit deletes the old match dispatch.

**Side benefits**:
- Body deserialization moves from manual `serde_json::from_slice(&body)` per handler to typed `Json<T>` extractor → cleaner error handling for malformed bodies (auto-400 instead of per-handler 500).
- Query parameter parsing moves to typed extractors.
- Tower middleware composes (auth, logging, rate limiting all become layers).
- Async-first by default; no `tokio::task::spawn_blocking` boilerplate per handler (F5 pattern moves to a `BlockingHandler` extractor).

**LoC estimate**: -500 (removing repetitive guards + dispatch boilerplate) + 200 (axum setup + typed handlers) = net -300.

**Risk**: medium. axum (or chosen alternative) is a load-bearing dep; pick one that's been stable for years.

---

### B. TLS + auth upgrade

**Problem**: today, the daemon listens on `127.0.0.1:<random>` with a bearer token. The ngrok tunnel adds TLS to the public hop, but ngrok terminates TLS — so traffic between ngrok and the local daemon is plaintext HTTP. Bearer tokens are global (one token = full access). No revocation. No per-client scoping.

**Solution**:

#### B.1 — End-to-end TLS

- Daemon generates a self-signed cert on first boot (`~/.k2so/daemon.crt` + `daemon.key`).
- Cert pinning instead of CA validation — clients verify against a known fingerprint (stored in `~/.k2so/connect-hosts.json` for K2SO Connect; provisioned via QR-code-style pairing for Mobile Companion).
- ngrok tunnel passes through TLS instead of terminating it (`ngrok http https://localhost:...` with `tls_termination=upstream`).
- Local socket path still HTTP (loopback is trusted).

#### B.2 — Per-client tokens with scopes

- Replace the global `~/.k2so/daemon.token` with a token DB table: `(token_hash, label, scopes_json, created_at, last_seen_at, revoked_at)`.
- Token formats: `k2so_local_<rand>` (full-scope, used by Tauri + cli/k2so on the same machine), `k2so_mobile_<rand>` (companion-scoped), `k2so_connect_<rand>` (per-laptop full-scope with cert pin).
- Scopes per route: read-only, mutating, terminal-write, admin (settings/scheduler control).
- Revocation: `k2so token revoke <label>` invalidates immediately; subsequent requests get 401.
- Token rotation: every client refreshes its token on demand; old tokens stay valid for an overlap window.

**Migration path**:
- Backwards-compat: the old global token keeps working until the first client refresh. New tokens are generated lazily as clients connect.
- TLS rollout is opt-in via setting (`security.require_tls`); disabled by default until at least one Mobile Companion build supports it.

**Risk**: medium-high. Cert pinning + token DB are real systems; need careful design + tests. Spike before committing.

---

### C. OpenAPI export + TS codegen

**Problem**: today, every client (renderer, Mobile Companion, K2SO Connect, cli/k2so) hand-rolls the same daemon HTTP calls. Renderer has `daemon-cli.ts`, `daemon-settings.ts`, `terminal-daemon.ts`, `llmDaemonClient.ts` — four hand-written shims, each with a partial duplicate of the contract.

**Solution**:
- Export `openapi.json` from the daemon at `/cli/openapi.json` (auto-generated from the axum router from Workstream A).
- TS codegen: `bun openapi-typescript /cli/openapi.json -o src/renderer/lib/daemon-api.ts` produces typed client.
- Renderer migrates from hand-written shims to the generated client. Mobile Companion does the same (Swift codegen via `openapi-generator`). K2SO Connect uses the same TS client.
- `cli/k2so` shell stays manual (no codegen for bash), but its commands directly mirror the OpenAPI paths.

**Side benefit**: contract changes get caught at compile time on every client. Add a route? Renderer types update. Remove a route? Renderer compile breaks (you knew it broke; now it tells you).

**LoC estimate**: -800 (delete hand-written shims) + 200 (codegen wiring) = net -600.

**Risk**: low. OpenAPI tooling is mature.

---

### D. Mobile Companion contract update

**Problem**: Mobile Companion was built against the pre-Phase-2 contract. After Phase 2, several endpoints moved or changed shape:
- LLM endpoints moved from Tauri `assistant_*` → daemon `/cli/llm/*` (companion was already daemon-side, but the shape might differ).
- PTY routes now route through daemon `/cli/terminal/*` + `/sessions/grid` + `/sessions/bytes` WS.
- Settings moved to `/cli/settings/*`.
- Companion management endpoints got new fields (`/cli/companion/set-password`, `/cli/companion/disconnect-session`).

**Solution**:
- Inventory: list every daemon endpoint Mobile Companion calls today.
- Per-endpoint: confirm the post-Phase-2 shape matches, or update the companion to the new shape.
- Add: typed Swift client generated from OpenAPI (Workstream C dependency).
- Test: end-to-end smoke against a real daemon over ngrok.

**Coordination**: Mobile Companion team gets a "contract changelog" doc covering every endpoint change in Phase 2 + Phase 3. They iterate in parallel with Phase 3's other workstreams; the public 0.39.0 release gate requires Mobile Companion's update to ship.

**Out of scope here** (future Mobile Companion features, not Phase 3):
- WebSocket-first companion protocol (separate planned project)
- Wake Agent from Mobile Companion (workspace inbox item)
- Structured chat protocol (workspace inbox item, inspired by pi-Mono RPC)

---

### E. K2SO Connect — thin-client-only `.app`

**Problem**: today, K2SO ships as a bundled `.app` (Tauri + daemon side-by-side). K2SO Connect needs to be installable as a standalone thin client that connects to a remote daemon.

**Solution**: a second build target.

#### E.1 — Build configuration

- `tauri.conf.json` gets a feature-flag for "no bundled daemon" — `K2SO_CONNECT_ONLY=1` builds without the daemon binary in `Contents/Resources/`.
- Release script produces two `.app` bundles: `K2SO.app` (bundled) and `K2SO Connect.app` (thin-client-only).
- Both share the same Tauri source; the only difference is the bundle contents + first-boot behavior.

#### E.2 — First-boot UX in Connect mode

- Bundled K2SO autostarts its daemon and connects to localhost.
- K2SO Connect shows a "Connect to K2SO daemon" screen: enter a hostname or scan a QR code, paste a token, verify a cert fingerprint.
- After successful pairing, the address book is populated; future launches reconnect automatically.

#### E.3 — Address book

- Stored at `~/.k2so/connect-hosts.json`: `[{label, hostname, port, token, cert_fingerprint, last_connected_at}]`.
- UI surface in Settings → Connections.
- Multi-daemon support: switch between connected daemons via a menu, similar to switching workspaces in a chat app.

#### E.4 — Online/offline state machine

- Connection state visible in tray + window chrome (connected, reconnecting, offline).
- Background retry with exponential backoff when daemon is unreachable.
- All renderer code already calls daemon HTTP; nothing else changes in Connect mode.

**LoC estimate**: ~500 LoC for the address book + pairing UI + state machine, all in the renderer + a small Rust shim for cert verification.

**Risk**: low for the build target (Tauri supports feature-flagged bundles natively). Medium for the pairing UX (security-sensitive flow; needs review).

---

### F. Versioning policy

**Problem**: Phase 2's `/cli/*` routes are unversioned. The first breaking change after Mobile Companion ships will force coordinated client updates.

**Solution**:
- All current routes are implicitly `v1`. The OpenAPI export labels them as such.
- Breaking changes (changed response shape, removed field, changed semantics) require a new endpoint at `/cli/v2/<route>`. The `v1` endpoint stays serving the old contract for **at least one release cycle**.
- Additive changes (new optional field, new endpoint, new query param) don't require a version bump.
- Deprecated `v1` endpoints emit a `Sunset:` HTTP header with a removal date.

**Communication**: a `CONTRACT-CHANGELOG.md` in the repo tracks every route addition, deprecation, and removal. Mobile Companion + K2SO Connect teams subscribe.

**LoC estimate**: minimal. Mostly a documentation + governance discipline.

---

### G. Rate limiting + observability

**Problem**: Mobile Companion + K2SO Connect are network-exposed clients. A misbehaving client (or attacker with a leaked token) could DoS the daemon. Today there's zero rate limiting or per-client observability.

**Solution**:

#### G.1 — Rate limiting

- Tower middleware (from Workstream A) gates per-token: N requests/min, burst M.
- Different scopes get different limits: read-only generous (60/min), mutating moderate (20/min), terminal-write strict (10/min).
- Exceeded → 429 with `Retry-After` header.

#### G.2 — Observability

- Per-request log line: `[cli] <method> <path> <status> <duration_ms> token=<label> ip=<addr>`.
- Aggregate metrics: requests/min, p50/p99 latency, error rate per route. Exposed via `/cli/metrics` (read-only, admin-scope).
- Audit trail for mutating routes: persisted in the existing `activity_feed` table (extend the actor column to include the token label).

**LoC estimate**: ~300 LoC. Mostly middleware setup + metrics aggregation.

**Risk**: low. Per-token state stays in memory; no DB schema changes.

---

## Dependency graph

```
       ┌─────────────────────────┐
       │ A. Typed router         │  ← unlocks middleware-based features
       └────────────┬────────────┘
                    │
       ┌────────────┼────────────┬─────────────┐
       ▼            ▼            ▼             ▼
┌──────────────┐ ┌─────────┐ ┌──────────┐ ┌──────────────┐
│ B. TLS +     │ │ C.      │ │ G. Rate  │ │ F. Versioning│
│   auth       │ │ OpenAPI │ │   limit  │ │   policy     │
└──────┬───────┘ └────┬────┘ └──────────┘ └──────────────┘
       │              │
       └──────┬───────┘
              ▼
    ┌──────────────────────┐
    │ D. Mobile Companion  │  ← needs typed client (C) + scoped auth (B)
    │   contract update    │
    └──────────────────────┘
              │
              ▼
    ┌──────────────────────┐
    │ E. K2SO Connect      │  ← needs cert pinning (B) + typed client (C)
    │   thin-client build  │     + pairing UX (own scope)
    └──────────────────────┘
              │
              ▼
    ┌──────────────────────┐
    │ Public 0.39.0 ship   │
    └──────────────────────┘
```

**Critical path**: A → B → D + E in parallel → public release.
**Parallelizable**: C and G can run alongside A/B; F is pure docs.

---

## Sequencing proposal

- **0.39.0g** — Workstream A (typed router). One big refactor, no behavior changes. Lands as a single PR.
- **0.39.0h** — Workstream G (rate limiting + observability) + Workstream F (versioning policy + CONTRACT-CHANGELOG). Both bolt on top of A's middleware model.
- **0.39.0i** — Workstream C (OpenAPI + TS codegen). Replaces hand-written daemon shims; large diff but mostly mechanical.
- **0.39.0j** — Workstream B (TLS + auth upgrade). Spike first; ship the cert/token DB next.
- **0.39.0k** — Workstream D (Mobile Companion contract update). Coordinated with companion team. Companion app version bump alongside.
- **0.39.0l** — Workstream E (K2SO Connect build). Separate `.app` target; pairing UX; address book.
- **0.39.0** — public release. Final smoke + release notes + sign + notarize.

Total: ~6 internal markers between Phase 2 done and public 0.39.0.

---

## Public 0.39.0 release gate

All of these MUST be true before tagging 0.39.0:

1. ✅ Phase 2 done (Units 1–7c + 2.1 + 2.2 merged; src-tauri ≤ 4,000 LoC; rusqlite removed from src-tauri).
2. ✅ Typed router live; zero per-handler method guards.
3. ✅ End-to-end TLS works over ngrok (smoke-tested).
4. ✅ Per-client tokens work; revocation works; old global token still accepted for graceful migration.
5. ✅ `/cli/openapi.json` exports cleanly; renderer + Mobile Companion both use codegen'd clients.
6. ✅ Mobile Companion app updated, tested end-to-end against the hardened daemon, ready to ship.
7. ✅ K2SO Connect `.app` builds, pairs with a remote daemon, exercises companion + LLM + PTY + settings flows end-to-end.
8. ✅ CONTRACT-CHANGELOG.md committed and accurate.
9. ✅ Rate limits live in production; observability metrics flowing.

After 0.39.0 ships public, the daemon's `/cli/*` surface is the official K2SO API.

---

## Out of scope (explicit non-goals for Phase 3)

- **WebSocket-first companion protocol redesign** — separate planned project; uses Phase 3's surface as the starting point.
- **Wake Agent from Mobile Companion** — feature on the workspace inbox; uses Phase 3's auth/contract but is a separate ship.
- **Structured chat protocol (pi-Mono RPC-inspired)** — workspace inbox item; lives on top of Phase 3.
- **R&D: Shadow terminal for mobile-native rendering** — research workstream; orthogonal to contract hardening.
- **Background terminal spawn endpoint** — already exists from Phase 2; UI exposure is companion-side work.
- **CLI verb redesign** — that's Phase 2.1, not Phase 3.
- **Schema changes** — Phase 2.2 finalizes the schema. Phase 3 doesn't touch tables.
- **TLS for the local socket** — loopback stays trusted; cert pinning is for the remote hops only.
- **Multi-tenant daemon** — one daemon per machine per user remains the model. K2SO Connect lets you reach OTHER people's daemons, but you don't run a daemon shared between users.
- **gRPC** — REST + WS over HTTPS is the contract. No gRPC because Mobile Companion (Swift) and renderer (TS) both have first-class HTTPS support and gRPC's tooling is more pain than gain at this scale.

---

## Open questions

1. **Typed router choice**: axum vs. poem vs. actix-web vs. roll-your-own with tower. Recommend axum (most ecosystem momentum, best ergonomics in 2026); spike before committing.
2. **TLS cert lifecycle**: self-signed forever, or rotate annually? Annual rotation requires re-pairing clients. Spike when designing Workstream B.
3. **Token DB schema**: rolls into Phase 2.2 schema hygiene (one more table) or after, in Phase 3? Recommend after — keeps Phase 2.2 pure hygiene.
4. **Mobile Companion build process**: who owns the Swift codegen + companion test infra? Need to confirm with companion team before scheduling Workstream D.
5. **K2SO Connect address book sync**: per-device only, or sync via iCloud/etc.? Recommend per-device for v1; sync is a follow-up.
6. **Rate limit defaults**: numbers are pulled from thin air above (60/min read, 20/min mutating, 10/min terminal). Need real-world telemetry before locking in.

---

## References

- Phase 2 PRD: `.k2so/prds/phase-2-daemon-headless-migration.md` (the source of the contract surface being hardened)
- Workspace inbox items deferred to post-Phase-3: WebSocket-first companion protocol, structured chat protocol, Wake Agent, R&D shadow terminal
- Memory: `project_websocket_companion_plan`, `feedback_daemon_first`, `feedback_post_only_route_guards` (Workstream A makes this rule structurally enforced rather than per-handler)
