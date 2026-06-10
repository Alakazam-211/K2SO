---
title: "K2 Connect — test hardening plan"
status: draft
owner: Rosson
created: 2026-06-04
source: comprehensive coverage audit (read-only) of the daemon/auth/connect surface
---

# K2 Connect — Test Hardening Plan

## The core finding

K2 Connect added a **critical authorization boundary** (remote daemon access +
multi-user sessions + Owner/Admin/Member roles). Coverage is strong at the
**unit** level (connect_users.rs helpers, version-policy logic, daemon_lifecycle
path classifiers, login_path::merge_path) but the **dispatcher-integration**
layer — the HTTP request → `200`/`403`/`405`/`503` decision through
`crates/k2so-daemon/src/routes/dispatcher.rs` — is ~94% untested. The two bugs
that bit us this session live exactly there and are now **unprotected against
regression**:
- **0.39.20** (commit 847a830): the generic `/cli/*` catchall gated owner-token-only
  instead of `token_ok` → a remote connect-user session couldn't read any data
  ("connected but shows local workspaces").
- **#629 roles**: Member/Admin/Owner gates on `/cli/users/*` exist only as unit
  helpers (`can_manage_users`/`can_change_roles`/`can_act_on`) — no test drives
  e.g. `POST /cli/users/remove?token=<admin-session>` → 403.

Risk register highlights: connect-user-denied-read (HIGH, no regression test),
GET-on-POST-only-route silently succeeding (HIGH, no 405 test), file 0600 perms
on connect-users.json (MEDIUM, untested), WS token gates (MEDIUM, untested).

## The one high-leverage investment

Almost every P0/P1 gap is unlocked by a single piece of **test infrastructure**:
a daemon-route integration harness — `test_daemon_state_with_owner_token()`,
`create_test_session(username, role)` (seed the in-memory connect-users store),
`mock_stream("POST /cli/... ?token=...")`, and a `dispatch()` driver returning
the response — plus a `with_temp_home` for store-write tests. Build it once
(likely `crates/k2so-daemon/tests/auth_routes_integration.rs`), and all the
authorization regression tests become cheap. Serialize the in-memory
connect_users state across tests (mirror the existing agents_routes integration
pattern).

## Prioritized plan

### P0 — Authorization regression locks (build first)
The harness + ~15–20 dispatcher-integration tests:
1. Generic `/cli/*` catchall accepts a connect-user SESSION (projects/list,
   fs/read-dir → 200; no/garbage token → 403) — locks 847a830.
2. Role matrix on `/cli/users/*`: Member → 403 on manage routes; Admin → 200 on
   list/add (added user defaults Member) + set-disabled(non-owner) but 403 on
   remove/set-role/acting-on-Owner; Owner/owner-token → 200 on all.
3. POST-only method gates: GET on a mutating route → 405 (not silent 200).
4. `/cli/tunnel/*` owner-only: any session role → 403.
5. `/cli/users/policy`: GET owner-or-session 200; POST owner-only (session 403).
6. `/cli/auth/login` PUBLIC + generic 401 on bad creds + fixed delay; lockout
   after 3 fails on login AND change-password.
7. `/cli/auth/whoami` returns `isOwner` + `role` for owner vs each session role.

### P1 — Role + connect-users security
`can_act_on` 3×3 matrix unit test; connect-users.json **0600** perms on write;
pre-#629 JSON (no `role`) deserializes → Member; lockout persistence across
mixed login/change-password calls.

### P2 — Data-plane + host-switch (renderer)
New `daemon-cli.test.ts`: requests hit the ACTIVE host + carry its token;
`handleRemoteUnauthorized` actually clears the session on 401; conn-retry on
ECONNREFUSED; secure/port-443 URL building. Strengthen `host-switch-reset.test.ts`
so re-fetches are asserted to use the NEW host's creds (today the spies only check
"loader ran", not "talked to the new host") + a per-store boot+reload assertion so
a store missing its `onActiveHostChange` wiring is caught.

### P3 — Lifecycle + WS
boot-status shape; **503 on non-ready routes while migrating**; ConnectionGate
policy integration (local exact-match accept, version-mismatch wait, remote
protocol-range); WS token gate before upgrade (`/events`, sessions/subscribe).

### Cross-repo (coordinate, out of this repo)
Control-plane↔Supabase token sync, RLS, frpc tunnel, two-device host↔client
auth, Companion — integration/live-smoke, tracked against `../k2-connect`.

## Effort

Human estimate ~100–150h total; agent-driven this compresses heavily and is the
classic Workflow shape (one harness, then fan-out one test-group per route
family with adversarial verification). Recommend building **P0 first** (it's the
regression lock for the shipped bugs and the harness unblocks everything else),
then P1, then P2/P3 incrementally — each independently shippable.
