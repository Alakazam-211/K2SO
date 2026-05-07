# PRD — Session labels owned by the daemon

**Status:** draft, 2026-05-06
**Author:** rosson@alakazamlabs.com (with implementation help)
**Related:** agent-display-name PRD (companion piece — agent's
display name is the LABEL of the workspace's pinned session),
0.37.4 tab title source guard (the band-aid this PRD retires),
daemon-first architecture invariant
(`feedback_daemon_first.md`)

## Problem

The thin client (renderer) currently owns each tab's title as
local React/Zustand state. It also tries to gate PTY title-change
events from clobbering it via the new `Tab.titleSource` field
shipped 0.37.4. That's the wrong layer:

1. The daemon **owns the PTY**. It sees every title change first.
2. The daemon **owns the session lifetime** — it knows when a
   session is born, when it transitions, when it dies.
3. The daemon **owns the multi-client surface** — multiple
   renderer windows, the mobile companion, CLI tools, and any
   future MCP exposure all want to see the same label for "what
   am I looking at."

When the label lives in the renderer:

- Two Tauri windows can disagree on the same session's label.
- The companion app can't show the label at all (the daemon
  doesn't know it).
- `k2so agents running` and `k2so sessions list` print the
  canonical key, not the friendly name the user sees.
- Every renderer mount has to fight PTY title events with a
  client-side guard (the `titleSource` band-aid).
- localStorage is the persistence layer — survives reload, but
  not daemon-driven changes (rename via `set_primary_agent`
  doesn't propagate).

The daemon-first invariant we set in 0.34 says: **logic lives
in daemon/core; Tauri/CLI are thin triggers; test headless
before shipping.** The label is logic.

## Goal

Every session the daemon hosts has an authoritative label string
the daemon owns. Every consumer (Tauri tabs, mobile companion,
CLI `agents running` / `sessions list`, future MCP server) reads
the same label from the same source. PTY title events are
absorbed in the daemon — they never round-trip through the
renderer and back.

## Non-goals

- Removing the renderer's `Tab` concept. Tabs are the right UI
  abstraction; what changes is what the tab *displays*.
- Cross-platform UI work. This is purely a state-ownership
  refactor.
- Persisting labels in SQLite (the label is session-scoped, lives
  with the session in `v2_session_map`; outlives a renderer
  reload but not a daemon restart — and the rename helper from
  the agent-display-name PRD reconstructs labels on respawn).

## Design

### Each `DaemonPtySession` carries a label

Today `DaemonPtySession` holds the PTY config, the alacritty
`Term`, the broadcast channels, and the canonical args. Add
two fields:

```rust
// crates/k2so-core/src/terminal/daemon_pty.rs
pub struct DaemonPtySession {
    // ... existing fields ...

    /// Human-friendly label shown in tab bars, agents-running
    /// listings, mobile companion, etc. Initialized at spawn
    /// (caller-supplied or derived); mutated by PTY title events
    /// unless `label_source == Locked`.
    label: Arc<RwLock<String>>,
    label_source: Arc<RwLock<LabelSource>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelSource {
    /// Default — PTY title events freely update the label
    /// (e.g., a Cmd+T tab where vim's filename or claude's
    /// progress is informative).
    Pty,
    /// Caller (renderer or CLI) supplied an explicit label at
    /// spawn time and chose not to lock it. PTY can still
    /// update — caller is just providing the seed.
    Seed,
    /// Locked by the spawn caller or an explicit
    /// `set_session_label` write. PTY events are ignored for
    /// label purposes (still drive activity tracking, just not
    /// the visible label).
    Locked,
}
```

### Spawn API: caller can supply a label + lock policy

`SpawnWorkspaceSessionRequest` gains two optional fields:

```rust
pub struct SpawnWorkspaceSessionRequest {
    // ... existing fields ...
    pub label: Option<String>,         // None → derive from agent name
    pub label_locked: Option<bool>,    // None → false (PTY can update)
}
```

`/cli/sessions/v2/spawn` accepts the same fields in the JSON
body. Heartbeat fresh-fires pass `label: Some("<heartbeat_name>")`,
`label_locked: Some(true)` — heartbeat sessions never want their
label rewritten by Claude Code's "Claude Code" PTY title.

The **canonical workspace+agent session** uses the agent display
name from the agent-display-name PRD's helper as the seed label
(`label: Some(agent_display_name(project_path))`,
`label_locked: Some(true)`). Renaming the agent (edit AGENT.md
`name:` field) → daemon updates the live session's label →
emits `LabelChanged` event → renderer re-reads.

### Daemon-side PTY title interception

Today `sessions_grid_ws::emit` (the path that pushes title bytes
through to the renderer's WS subscriber) is where titles flow
out. Hook it: when alacritty's `WindowTitleChanged` event fires,
strip spinner glyphs (already done client-side; move to daemon),
and:

```rust
match *session.label_source.read() {
    LabelSource::Pty | LabelSource::Seed => {
        *session.label.write() = cleaned_title;
        broadcast_label_changed(session.session_id, &cleaned_title);
    }
    LabelSource::Locked => {
        // Activity tracking still fires (idle/working glyph
        // detection), but label stays put.
    }
}
```

The renderer's existing `case 'title':` handler retires entirely.
Activity tracking stays where it is on the renderer side (it's
already cheap), or moves to the daemon as a follow-up.

### Renderer reads labels via the existing WS protocol

The grid WS already pushes `Snapshot` and `Delta` events. Add
two more event types to the protocol:

```rust
// crates/k2so-core/src/terminal/grid_snapshot.rs
pub enum SessionEvent {
    Snapshot(TermGridSnapshot),
    Delta(TermGridDelta),
    Title(String),           // existing — PTY title raw
    LabelChanged(String),    // NEW — authoritative label
    LabelInitial(String),    // NEW — sent on connect with current label
}
```

On WS connect, the daemon emits `LabelInitial` once. On every
label change, `LabelChanged` is broadcast to all subscribers of
that session.

### Renderer state shape

`Tab` loses its `title` field as a stored property:

```typescript
// before
interface Tab {
  id: string
  title: string                   // ← stored, persisted to localStorage
  titleSource?: TabTitleSource    // ← band-aid we shipped 0.37.4
  // ...
}

// after
interface Tab {
  id: string
  // title is no longer stored. A derived selector reads it from
  // the daemon-owned session label via the new
  // `useSessionLabel(sessionId)` hook.
  sessionId: string | null         // ← daemon session this tab is bound to
  // ...
}
```

A new `session-labels` Zustand store mirrors what the daemon has
told us (one entry per live session). Subscribed via the WS
handler. Tab components render `{useSessionLabel(tab.sessionId) ?? fallback}`.

### Persistence

localStorage no longer stores the title. On Tauri reload:

1. Tab state restores with `sessionId` only.
2. Renderer mounts the tab, opens the WS subscription.
3. Daemon's `LabelInitial` fires immediately on connect.
4. Tab title appears.

For sessions whose daemon entry is gone (daemon restart, exit), the
tab falls back to a derived label — the same agent-display-name
helper queried via Tauri command. No empty tabs.

### User rename → daemon write

Tab right-click → Rename — calls a new endpoint:

```
POST /cli/sessions/<id>/label
  body: { label: "...", lock: true }
```

Daemon updates the session, emits `LabelChanged`, every subscriber
re-reads. Locked = true so the next PTY title event can't undo it.

For the canonical workspace+agent session specifically, "rename"
through this endpoint is just a label change — the agent's
display name stays in AGENT.md (PRD: agent-display-name). The
two are orthogonal: AGENT.md is "what the agent is called
contextually," the session label is "what's on this tab right
now." They normally agree (the canonical session's spawn used
the agent display name as the seed), but a per-tab user override
is a UI affordance.

### CLI surfaces benefit too

`k2so agents running` becomes:

```
$ k2so agents running
NAME              SESSION                                  WORKSPACE
fast-test wake    8845ba89-...                             TestingK2SO
scout             dbfebabd-...                             nsi-checkin/dannon
Claude Code       17cee2a5-...                             ad-hoc
```

The label column comes from the daemon's authoritative label.
Companion app + future MCP get the same.

## What we're NOT doing

- **No SQL persistence of labels.** They're session-scoped. When
  the canonical session respawns after a daemon restart, the
  spawn helper re-derives the label from the agent display name
  helper. Heartbeat sessions re-derive from the heartbeat name.
  Ad-hoc tabs re-derive from PTY title.
- **No retroactive rename of localStorage tabs.** First Tauri
  open after upgrade resubscribes and pulls the daemon-owned
  label.

## Migration

Boot-time, idempotent:

1. Existing v2_session_map entries don't have labels yet — on
   the first WS subscriber connect, derive a label from the
   spawn args (canonical key suffix) and seed it.
2. Renderer's persisted `Tab.title` strings are ignored on load
   (we render from the WS-derived label). The persisted strings
   stay in localStorage as harmless dead state for one release,
   then we drop them in 0.39.

## Implementation phases

| phase | scope | files | effort |
|---|---|---|---|
| **1** | Add `label` + `label_source` to `DaemonPtySession` | `daemon_pty.rs` | ~1h |
| **2** | Wire spawn-time label seed (callers pass it; default derives from agent name) | `spawn.rs`, `v2_spawn.rs`, `wake_headless.rs`, `canonical_session.rs` | ~2h |
| **3** | Daemon-side PTY title → label update with `LabelSource` gate | `sessions_grid_ws.rs` (alacritty event hook) | ~2h |
| **4** | New WS protocol events `LabelInitial` + `LabelChanged` | `grid_snapshot.rs`, `sessions_grid_ws.rs` | ~1h |
| **5** | New endpoint `/cli/sessions/<id>/label` for explicit set | `terminal_routes.rs` (or new module) | ~1h |
| **6** | Renderer: new `session-labels` Zustand store, `useSessionLabel` hook, WS subscriber wiring | `src/renderer/stores/session-labels.ts`, `terminal-v2/TerminalPane.tsx` | ~3h |
| **7** | Renderer: drop `Tab.title` storage; tab components consume `useSessionLabel` | `tabs.ts`, every tab-rendering component | ~3h |
| **8** | Retire `Tab.titleSource` band-aid + the gated `setTabTitleFromPty` setter | revert 0.37.4 changes | ~30min |
| **9** | CLI: `agents running` / `sessions list` / `companion` print the label | `terminal_routes.rs`, companion routes | ~1h |
| **10** | Tests: unit (label_source state machine), integration (end-to-end label flow over WS) | new test files | ~2h |
| **11** | Manual smoke: open 2 windows, rename tab in one, verify label syncs to other | manual | — |

Total: ~16-17h focused work. Larger than the agent-display-name
PRD; pairs with it. **Both ship together as 0.38.0** — the agent
display name is the *label seed* for the canonical session, so
they're tightly coupled.

## Risks

| risk | mitigation |
|---|---|
| Mobile companion / older clients don't understand `LabelChanged` event | tag the event type; clients that don't recognize it ignore. They keep showing whatever they had |
| Activity tracking depends on PTY title events the renderer used to see | Either keep the existing renderer-side title event flow alongside new label flow, or move activity tracking to daemon (recommend the latter as a follow-up) |
| WS event volume — labels change often during agent work | label change debounced 100ms in daemon; only emit on actual change, not on every tick |
| Race during spawn: WS subscriber connects before label is initialized | label is set BEFORE `v2_session_map::register`; subscriber's `LabelInitial` always sees the seeded value |
| Two windows simultaneously rename via UI | last write wins; daemon broadcasts `LabelChanged` to everyone, both windows converge |

## Acceptance criteria

1. **Multi-window sync**: open K2SO, open second Tauri window of
   the same workspace, rename the pinned tab in window A → label
   updates in window B within 200ms.
2. **PTY title clobber gone**: open the pinned chat tab, watch
   it survive 5 minutes of Claude Code spinner ticks without
   flipping to "Claude Code". (Today's `titleSource` band-aid
   does this; new path achieves the same without renderer-side
   gating.)
3. **`k2so agents running`** prints labels not canonical keys.
4. **Companion**: mobile companion shows the same label as the
   tab bar.
5. **Daemon restart**: kill k2so-daemon, relaunch. Open Tauri.
   Tabs that previously had labels reappear with the same labels
   (re-derived from spawn helpers).

## Connection to agent-display-name PRD

The two PRDs are siblings:

- **agent-display-name** answers *"what is the agent called?"*
  with one source of truth (`AGENT.md` `name:` field, fallback
  `projects.name`). One helper.
- **session-label-daemon-owned** answers *"what does the tab
  show?"* with the daemon owning the label string per session.

The canonical workspace+agent session's label = the agent
display name (Locked). The two converge: rename via AGENT.md
edit → display helper sees new value → daemon updates the
live session's label → all clients re-read.

Ship together. Or even cleaner: agent-display-name first
(quick), then session-label-daemon-owned (the bigger refactor).

## What today's renderer-side fix becomes

The 0.37.4 `Tab.titleSource` band-aid retires entirely:

- `Tab.titleSource` field — removed
- `setTabTitleFromPty` action — removed
- `addTabToGroup`'s automatic `titleSource: 'derived'` — removed
- `TerminalPane.tsx`'s `case 'title':` handler — removed (the
  daemon absorbs PTY titles now)

The fix isn't wasted — it confirmed the symptom and the
filter logic. The architecture just lives in the right layer
this time.
