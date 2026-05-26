# K2SO 0.39.5 — Versioned daemon handshake: no more blank window on update

Bug-fix release. Closes the "blank/black window after auto-update"
class of failures for good — including the worst case, updating from
an old version whose first boot has a lot of migrations to grind
through. The renderer now refuses to bind to anything but the daemon
**paired with this exact app build**, and the daemon **advertises its
migration progress** so the user sees "Setting up K2SO — applying
updates…" instead of a blank screen.

---

## The symptom

User updated 0.38.13 → 0.39.4 via auto-update; the app relaunched to a
blank window and looked crashed. There was **no OS crash report** and
**no daemon panic** — the process was alive, the window just never
rendered. A manual right-click → Reload fixed it.

This is the same race 0.39.2 and 0.39.3 each took a swing at. They
narrowed it; they didn't close it.

## Root cause (the part 0.39.2/0.39.3 missed)

Two independent facts combined into the bug:

1. **The daemon ran every first-boot migration BEFORE it bound its
   port.** In `k2so-daemon`'s `async_main`, the 0.37/0.39 migration
   sweep (workspace–agent unification across every workspace, the
   skills-consolidation pass, `legacy_agent_types_v1`, the 0.39.0
   auto-pin migration, layout migrations) ran *first*; only afterwards
   did it `claim_port` + bind the TCP listener. On a real install
   that's a multi-second window during which the **new** daemon answers
   nothing.

2. **The renderer's `ConnectionGate` trusted any 200 from `/ping`.**
   `/ping` is an unauthenticated liveness probe that returns the daemon
   banner with HTTP 200. The gate's check was literally `resp.ok`.

Now play the auto-update handoff forward, remembering the daemon port
is **stable across restarts** (persisted in `~/.k2so/daemon.port` and
re-used on the next boot):

1. 0.38.13 daemon is running, bound to the stable port, answering
   `/ping` 200.
2. App relaunches as 0.39.4. Tauri's `check_daemon_version_and_restart`
   detects the version mismatch and kickstarts a daemon restart (kills
   the old daemon, launchd brings up the new one).
3. **Race:** the gate polls `/ping` and gets a 200 — from the *outgoing
   0.38.13 daemon that is about to be killed*. `resp.ok` is true, so the
   gate dismisses, mounts `<App />`, and 0.39.3's deferred store imports
   fire their initial daemon fetches.
4. Meanwhile the old daemon is gone and the **new** 0.39.4 daemon is
   grinding through the heavy one-time 0.39.0 first-boot migration —
   and per fact (1) it hasn't rebound the port yet. The app's fetches
   land in the gap: connection refused / failed silently. Stores end up
   stuck in empty/broken state. The window renders black.
5. Eventually the new daemon finishes migrating and binds the port, but
   the already-mounted stores are stuck — hence "Reload fixes it."

The fatal assumption was **`/ping` reachability == ready to serve**, and
the stable port let a green ping come from the daemon that was on its
way out. Advertising progress alone wouldn't have helped — the app was
talking to the *wrong* (dying) daemon. It had to stop binding to the
old one first.

We confirmed the discriminator against the live v0.39.4 daemon: it
returns **HTTP 404 for `/boot-status`** (the route doesn't exist yet)
while still answering `/ping` with 200 — exactly the false-positive the
old gate trusted.

## The fix

A small, durable contract instead of another point-patch.

### 1. The daemon binds its listener FIRST, then migrates

`async_main` now claims the port, binds the listener, writes
`daemon.{port,token}` + `heartbeat.{port,token}`, and **spawns the
accept loop** — all *before* the migration sweep runs. The migrations
run on the boot thread (the runtime is multi-threaded, so the accept
loop keeps serving on another worker). A `boot_status` phase tracks
where we are: `starting` → `migrating` → `ready`.

The pre-0.39.5 invariant — "route handlers never observe half-migrated
state" — is **preserved**, but now enforced by a readiness gate instead
of by ordering: while `phase != ready`, the dispatcher answers only
`/ping`, `/health`, and `/boot-status` and returns **503
`{"state":"migrating"}`** for every real route. So the daemon is
reachable and can describe itself the entire time it's migrating, but
nothing can read partial state.

### 2. A versioned readiness handshake: `GET /boot-status`

New unauthenticated endpoint:

```json
{ "version": "0.39.5", "protocol": 1, "phase": "migrating",
  "detail": "Applying updates…" }
```

- **`version`** — exact build string. The local/auto-update path
  requires this to equal the app's bundled version, so the renderer
  can never bind to an outgoing old daemon.
- **`protocol`** — daemon↔client API compatibility integer, bumped
  *only* on a breaking contract change (not every release). This is
  what **K2 Connect** range-checks for remote daemons, decoupled from
  the marketing version. Starts at `1`.
- **`phase`** — `starting | migrating | ready` (+ reserved `error`).
  Clients treat anything but `ready` as not-ready (forward-compatible).
- **`detail`** — free-text for the UI only; never parsed for logic.

A pre-0.39.5 daemon has no such route and 404s it, so an outgoing old
daemon fails the gate **without any special-casing**.

### 3. The gate is version-aware and policy-driven

`ConnectionGate` now polls `/boot-status` and decides via a **pluggable
acceptance policy** so we don't paint ourselves into a corner:

- **`localPairedPolicy(expectedVersion)`** (this release): mount iff
  `version === expectedVersion && phase === 'ready'`. `expectedVersion`
  comes from Tauri's `getVersion()`. This is what kills the bug:
  - old daemon 404s → unreachable → keep waiting;
  - a future old-but-not-ancient daemon reports the wrong `version` →
    keep waiting;
  - correct daemon, still migrating → render "Setting up K2SO —
    applying updates… `<detail>`";
  - correct daemon, `ready` → mount.
- **`Remote { protocolRange }`** (documented seam, K2 Connect): a
  remote daemon is legitimately a different marketing version, so it
  will range-check `protocol` instead of requiring exact `version`
  equality. The gate core is version-agnostic; this logic lives only
  in the policy. **Do not** reuse `localPairedPolicy`'s exact-equality
  for remote daemons.

This also fixes *future* updates (0.39.5 → 0.39.6 …): the outgoing
0.39.5 daemon now *has* `/boot-status` and reports `version: "0.39.5"`,
so the 0.39.6 app rejects it on the version field until the kickstarted
0.39.6 daemon comes up. The bug can't recur regardless of how old the
version being updated from is.

### Code changes

- **`crates/k2so-daemon/src/boot_status.rs`** (new): process-global
  phase (`AtomicU8`) + detail (`RwLock<String>`) + the `PROTOCOL`
  constant. Single source of truth for `/boot-status` and the
  dispatcher gate. Unit-tested.
- **`crates/k2so-daemon/src/main.rs`**: `async_main` restructured —
  bind + accept-loop spawn + `set_migrating` now precede the migration
  sweep; providers/companion/sinks register after; `set_ready()` flips
  the gate at the end; the process then awaits shutdown (the accept
  loop runs as its own task). Ctrl-C during migration is buffered.
- **`crates/k2so-daemon/src/routes/dispatcher.rs`**: the
  `/boot-status` route + the readiness gate (503 for non-liveness
  routes until `boot_status::is_ready()`).
- **`src/renderer/components/ConnectionGate.tsx`**: poll `/boot-status`;
  `localPairedPolicy`; render `Connecting…` (waiting) vs `Setting up
  K2SO… <detail>` (migrating) vs mount (ready). Reload escape hatch
  shows after ~10s while waiting, ~60s while migrating (a big upgrade's
  migration can legitimately take a minute — don't nag).

## Verification

- `boot_status` unit tests + all 89 `k2so-daemon` bin unit tests pass.
- Renderer typecheck baseline preserved (47 errors, none in
  `ConnectionGate.tsx`).
- Headless (throwaway `$HOME`): new daemon serves
  `{"version":…,"protocol":1,"phase":"ready","detail":""}` on
  `/boot-status`; boot log confirms the new order — `Listening on …`
  **before** the migration sweep, `boot complete — phase=ready` last;
  zero panics.
- Live v0.39.4 daemon: `/boot-status` → 404, `/ping` → 200 — the exact
  old false-positive, now correctly rejected by the gate.

## Also observed during diagnosis (not fixed here)

Two pre-existing issues surfaced in the daemon logs while
investigating. Filed for follow-up; neither caused the crash:

1. **Non-idempotent triage-heartbeat scaffold.** First-boot migration
   logs repeated `Failed to insert heartbeat: UNIQUE constraint failed:
   workspace_heartbeats.project_id, workspace_heartbeats.name` for
   several workspaces. Caught (`errors=0`), but the scaffold insert
   should be `INSERT OR IGNORE`/upsert.
2. **Phantom `nobody` pending-live accumulation.** Every boot logs
   `N pending-live signals queued for agent nobody (will deliver on
   next spawn)`; the signals address an agent that never spawns, so
   they never drain (~45 persisted; thousands of log lines across
   boots). A slow leak, not a hang.
