# Plan B — Canonical Renderer→Daemon Migration Pattern

> **Copy this recipe verbatim.** It is the one true pattern for moving a
> renderer store/module's daemon-data calls OFF the Tauri
> `invoke()`→`DaemonClient` proxy (hardcoded to localhost, with a
> global-state race) ONTO the host-aware HTTP layer
> `src/renderer/lib/daemon-cli.ts`. The pilot was `timer.*` — read
> `src/renderer/stores/timer.ts` + `src/renderer/stores/timer.test.ts`
> alongside this doc for a worked example.

## Why

The `invoke('projects_list')`-style commands route through Rust's
`DaemonClient`, which is pinned to `127.0.0.1`. K2 Connect lets the desktop
client target ANY daemon (local OR a remote host). `daemonCliGet` /
`daemonCliPost` read `useConnectHostStore.getState().activeHost` and resolve
the right base+token per call, so they work local OR remote. The proxy also
has an ordering race (fixed separately in connect-host.ts) that this
migration ultimately retires.

---

## 1. Import + call

```ts
import { daemonCliGet, daemonCliPost, daemonCliGetText } from '@/lib/daemon-cli'
```

- **GET (JSON response):** `await daemonCliGet<T>('domain/route', { snake_case_params })`
- **GET (raw text/CSV/JSON-string response):** `await daemonCliGetText('domain/route', { ...params })`
- **POST (mutation):** `await daemonCliPost<T>('domain/route', { camelCaseBody })`

`route` is the part AFTER `/cli/` (e.g. `timer/entries-list`). Do NOT include
`/cli/`, the leading slash, the `?token=`, or the host — the helper adds all
of that and resolves the active host's creds.

---

## 2. Route-name + param-casing mapping rule

Confirm the route name and param casing against
`crates/k2so-daemon/src/**` for EVERY call — never guess. The dispatch tables
live in `crates/k2so-daemon/src/cli.rs` (GET) and
`crates/k2so-daemon/src/db_routes.rs` / `routes/dispatcher.rs` (POST). The
handler signature tells you the exact contract.

| Direction | Casing | Why |
|---|---|---|
| **GET query params** | `snake_case` | The handler reads them with `params.get("project_id")` — see `str_param`/`opt_str`/`opt_i64` in `db_routes.rs`. `projectId` would silently read as absent. |
| **POST JSON body** | `camelCase` | The body struct is `#[derive(Deserialize)] #[serde(rename_all = "camelCase")]` — e.g. `TimerCreateBody { project_id, start_time }` deserializes from `projectId`, `startTime`. |
| **JSON response** | `camelCase` | Response structs are also `#[serde(rename_all = "camelCase")]` (e.g. `TimeEntry` → `projectId`, `startTime`, `createdAt`). The renderer's `interface` matches as-is; no remap needed. |

**Worked timer mapping (confirmed against the daemon):**

| Old Tauri invoke | New call | Daemon handler |
|---|---|---|
| `invoke('timer_entries_list', {start,end,projectId})` | `daemonCliGet<TimeEntry[]>('timer/entries-list', { start, end, project_id })` | `handle_timer_entries_list` (`cli.rs:1463`) |
| `invoke('timer_entry_create', {...})` | `daemonCliPost('timer/create', { id, projectId, startTime, endTime, durationSeconds, memo })` | `handle_timer_create` (`db_routes.rs:539`, `TimerCreateBody` camelCase) |
| `invoke('timer_entry_delete', {id})` | `daemonCliPost('timer/delete', { id })` | `handle_timer_delete` (`db_routes.rs:554`, `IdBody`) |
| `invoke('timer_entries_export', {format,start,end,projectId})` | `daemonCliGetText('timer/entries-export', { format, start, end, project_id })` | `handle_timer_entries_export` (`cli.rs:1464`, `CliResponse::ok_text`) |

> Note the route names are NOT a mechanical `_`→`/` of the old command:
> `timer_entry_create` → `timer/create` (not `timer/entry-create`). Always
> read the dispatch line.

`undefined`/`null` params are dropped from the query string by the helper
(`if (v !== undefined && v !== null)`), so pass `undefined` for "absent" —
do NOT pass `null` strings. (Old proxy code passed `?? null`; with the new
helper just pass the value or `undefined`.)

---

## 3. Response handling

| Daemon response | Renderer handling |
|---|---|
| Serialized value (`serialized(...)` → `ok_json`) | `daemonCliGet<T>(...)` returns the parsed `T` directly. |
| Unit success (`unit_ok(...)` → `{"success":true}`) | `daemonCliPost(...)` — ignore the body; keep the existing optimistic/local state update. Do NOT branch on `{success:true}`. |
| Plain text (`CliResponse::ok_text`, e.g. CSV/JSON export) | Use **`daemonCliGetText`** — it returns the raw body string and never `JSON.parse`es it. **This is the one response gotcha:** the default `daemonCliGet` parses JSON-shaped text into an object, which silently breaks callers that expect a string (e.g. `new Blob([data])` would emit `[object Object]`). If a route's body is text OR a JSON-string consumed verbatim, use `daemonCliGetText`. |

Errors: any non-2xx throws a clean `Error` (message prefers the daemon's
`{"error":"..."}`). Keep your existing `try/catch` — it works unchanged.

### The ONE transform case to watch

Most domains map 1:1, but `workspace_layouts` is the exception: its old Tauri
command did an `Into::into` transform between the DB row and the
renderer-facing shape. When you migrate that module, replicate the transform
on the renderer side (or confirm the daemon route already emits the
post-transform shape) — do NOT assume the raw row matches the old TS type.
Diff the old command's return against the daemon handler's serialized type.

---

## 4. Cross-window sync — emit `sync:*` after EVERY mutation

**Critical.** Several old Tauri mutation commands emitted a `sync:*` event
from Rust (`app.emit("sync:timer-entries", ())`) so OTHER windows (focus
windows, second main window) re-fetch. Bypassing the Tauri command means that
Rust-side emit no longer fires. After each migrated MUTATION, emit the SAME
event from the renderer:

```ts
import { emit } from '@tauri-apps/api/event'

// after a successful daemonCliPost mutation:
void emit('sync:timer-entries').catch((e) =>
  console.warn('[timer] sync:timer-entries emit failed:', e),
)
```

- Emit **only on success**, AFTER the `daemonCliPost` resolves (not in the
  catch). A failed mutation must not trigger a phantom cross-window refresh.
- The listener side already exists in `src/renderer/hooks/useWindowSync.ts`
  (`listen('sync:timer-entries', () => …fetchEntries())`). **Verify the
  listener for your channel exists there** before relying on it — every
  `sync:*` channel the old Rust commands emitted has a matching `listen` in
  that file. If yours is missing, the cross-window refresh is silently dead.
- Find which channel(s) a command emitted by grepping the old Tauri command
  in `src-tauri/src/commands/<domain>.rs` for `.emit(`.
- The ephemeral live-state broadcast (`invoke('broadcast_sync', …)` →
  `sync:timer`) is NOT a DB mutation and stays on `invoke` — only the
  DB-write commands move.

---

## 5. Test-mock swap

Swap the `invoke` mock for `daemonCli*` mocks. The store's import-time side
effects (e.g. `initFromSettings()`) mean `vi.mock` MUST precede the store
import (vitest hoists `vi.mock`).

```ts
const daemonCliGet = vi.fn()
const daemonCliGetText = vi.fn()
const daemonCliPost = vi.fn()
vi.mock('@/lib/daemon-cli', () => ({
  daemonCliGet: (...a: unknown[]) => daemonCliGet(...a),
  daemonCliGetText: (...a: unknown[]) => daemonCliGetText(...a),
  daemonCliPost: (...a: unknown[]) => daemonCliPost(...a),
}))

const emitMock = vi.fn(() => Promise.resolve())
vi.mock('@tauri-apps/api/event', () => ({ emit: (...a: unknown[]) => emitMock(...a) }))
```

Assert the route name + EXACT param object (snake_case for GET, camelCase
for POST), `set(...)` state changes, AND the `emit('sync:*')` call after
mutations. Tests must **fail loudly** — no `unwrap_or` defaults, no
skip-if-missing, no try/catch swallowing in assertions. Cover both the
success path and the failure path (rejected mock → error toast, NO sync
emit, state still resets if that's the contract).

---

## 6. DO-NOT-TOUCH list — these stay `invoke()`

These are **host-only / OS-integration / connection-control** commands that
have no daemon HTTP route and MUST keep going through Tauri `invoke` (they
operate on the *local machine running the desktop app*, not on the daemon's
data):

- `pick_folder`, `upload_icon`
- `open_focus_window`
- `open_in_finder`, `open_in_editor`, `open_in_terminal`
- `fs_watch`, `fs_unwatch`
- `daemon_ws_url`
- keychain: `k2_secret_set` / `k2_secret_get` / `k2_secret_delete`
- `connect_hosts_read` / `connect_hosts_write`
- `relaunch`, `cli_install`
- permissions commands
- `set_active_daemon` (the proxy override itself)
- `broadcast_sync` (ephemeral live-state fan-out) + the `emit('sync:*')`
  cross-window calls above — these ARE Tauri events by design.

If a command reads/writes the daemon's SQLite or daemon-owned state →
migrate it. If it touches the host filesystem, native dialogs, the keychain,
window management, or the connection substrate → leave it on `invoke`.

---

## Checklist per module

1. [ ] Grep the daemon for each route name + confirm GET vs POST + param casing.
2. [ ] Swap `invoke('x', …)` → `daemonCliGet/Post('domain/route', …)` (snake GET params, camel POST body).
3. [ ] Use `daemonCliGetText` for any text/CSV/JSON-string body.
4. [ ] Replicate any `Into::into` transform (only `workspace_layouts` so far).
5. [ ] After each mutation: `emit('sync:<channel>')` on success; verify the listener in `useWindowSync.ts`.
6. [ ] Keep the DO-NOT-TOUCH commands on `invoke`.
7. [ ] Swap test mocks (`daemonCli*` + `emit`); assert routes/params/state/emit; cover failure paths.
8. [ ] `npx tsc --noEmit` clean; `npx vitest run <module>.test.ts` green.
