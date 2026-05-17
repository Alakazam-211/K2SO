# PRD — Daemon-authoritative tabs

**Status:** drafted 2026-05-17 by Rosson.
**Target release window:** 0.38.0 series.
**Predecessor:** `canonical-lane-restore.md` (0.37.12) — fixed pinned-chat / heartbeat tab restoration. This PRD generalises that work to **every** tab.

---

## TL;DR

Today each renderer (Zustand `TabsStore`) keeps its own private tab list
and persists it to `workspace_layouts.layout_json`. Multiple Tauri
windows for the same workspace each maintain their own copy. The
copies drift, sync-on-mount creates duplicates, and the resulting
state gets saved back, **corrupting `workspace_layouts` over time**.

Concrete evidence: TestingK2SO's `workspace_layouts` row contains 6
tab entries, but 4 of them are the **same** Claude tab (identical
`mosaicTree`/paneGroup ID `15521cbd-...`, identical
`sessionId` `acef6309-...`). The daemon correctly reports 3 PTYs.
Every restore faithfully recreates the 6 corrupt tab entries.

Flip the source of truth: **the daemon's `v2_session_map` is the tab
list.** Renderers query the daemon, render exactly that many tabs,
and overlay `workspace_layouts` only for **positioning / ordering /
titles**. The thin clients (main window, focus window, "New Window",
mobile companion) become read-projecting viewers of one canonical
list.

This is the same architectural move that 0.37.0 made for sessions
(daemon-first) and 0.37.11 made for resize (active-viewer claim) —
extended to tab identity.

## Conceptual model (the invariant we're enforcing)

1. **Daemon owns sessions.** For workspace W, the daemon's
   `v2_session_map` has N entries. That number is canonical.
2. **Tabs are a 1:1 projection of sessions.** N sessions ⇒ exactly N
   tabs in every viewer. Never N+1, never N×2.
3. **Thin clients are viewers.** Each one (main, focus, new window,
   mobile) queries the daemon for the session list and renders it.
   No private tab inventory. No tab-list saves.
4. **Multiple viewers, one PTY.** Two viewers can render the same
   tab; the PTY itself doesn't fork. Same grid, same scrollback,
   same input.
5. **Active viewer drives grid dimensions.** Exactly one viewer at a
   time is "active" (mouse / focus / keystroke on desktop;
   tap / app-foreground / keystroke on mobile). Its (cols, rows)
   apply to the PTY. Already implemented in 0.37.11 — this PRD
   keeps it untouched.
6. **`workspace_layouts` stores metadata only.** Tab order, split
   positions, active-tab pointer, custom titles. Never "does this
   tab exist." Existence is the daemon's question.

If a renderer ever produces a tab that doesn't correspond to a
daemon session, **that is the bug.** Today the code permits it; the
new model makes it structurally impossible.

## Phased plan

Each phase ships independently and is releasable. Later phases
build on earlier ones. Estimated 4 patch releases under the
0.38.x series.

### Phase A — 0.38.0 — Heal-on-read + plug the leak

The smallest change that stops the bleeding and recovers existing
corrupt workspaces.

- **`restoreLayout`** (`src/renderer/stores/tabs.ts:2287`) gains a
  dedup pass: tabs sharing the same `mosaicTree` paneGroup ID or
  the same `agentName + section` tuple collapse to the first
  occurrence. The cleaned layout is immediately re-saved so the
  fix is permanent for the workspace.
- **`useWindowSync`** (`src/renderer/hooks/useWindowSync.ts`)
  generalises the focus-window guard introduced in this session to
  **all non-main windows** (any label that isn't `main`).
  Mount-time `sync:tabs-request` is gone for new/focus windows
  entirely. Runtime `sync:tabs` listener stays so live add/remove
  events still propagate.
- **One-time DB cleanup pass at daemon boot.** A SQL migration
  (or a CLI subcommand `k2so doctor heal-layouts`) iterates
  `workspace_layouts`, parses each JSON, runs the same dedup, and
  re-saves. Logs `N → M` per workspace.

**Touches:** `tabs.ts`, `useWindowSync.ts`, a new migration
under `crates/k2so-core/drizzle_sql/`.
**Ships:** "Stops new corruption + heals existing rows."

### Phase B — 0.38.1 — Renderer queries daemon at mount

The architectural step. Tab existence flows from the daemon.

- **New helper** `buildTabsFromDaemon(projectPath)` in `tabs.ts`:
  - Call `k2so_sessions_list_for_workspace` (already exists,
    added in 0.37.11 for A9 adoption).
  - For each daemon session, construct a Tab whose `paneGroup.id`
    matches the session's canonical paneGroup key (`tab-<X>` for
    terminals; bare project_id for pinned chat; agentName for
    heartbeats).
  - Annotate `kind` (`chat | inbox | terminal | heartbeat`) so the
    renderer picks the right component.
- **`loadLayoutForWorkspace`** rewrites:
  1. `buildTabsFromDaemon` → returns N tabs (the canonical set).
  2. Load `workspace_layouts` JSON.
  3. **Overlay**: re-order tabs per layout's `order: [paneGroupId,...]`;
     apply custom titles; restore split positions; restore
     `activeTabId` by looking up the layout's pointer in the
     daemon set. Drop layout entries with no matching daemon
     session.
  4. If a daemon session is missing from layout overlay, append at
     the end with default title.
- **Ensure-system-tabs** for agent-mode workspaces remains: if
  daemon has no pinned-chat session yet, spawn one. Same idempotent
  logic as today.
- **No layout schema change yet** — the JSON keeps its current
  shape; we just stop trusting the `tabs[]` array for existence
  and read it as if it were a positioning hint.

**Touches:** `tabs.ts` (large rewrite of mount path), `projects.ts`
(no change expected), `AgentPane.tsx` (no change), `TerminalPane.tsx`
(no change — still attaches by canonical agent_name).
**Ships:** "Tab count is now identical across every viewer. Daemon
authoritative."

### Phase C — 0.38.2 — Layout schema migration

Make the metadata-only contract explicit.

- New `workspace_layouts.layout_json` shape:
  ```json
  {
    "version": 2,
    "groups": [
      {
        "order": ["<paneGroupId>", "<paneGroupId>", ...],
        "activeIndex": 0,
        "splitCount": 1,
        "titles": { "<paneGroupId>": "Custom title" }
      }
    ],
    "activeGroupIndex": 0
  }
  ```
- Migration on read: v1 → v2 collapses tab entries to their paneGroupIds
  in order; titles preserved.
- `serializeCurrentLayout` emits v2. v1 stops being written.

**Touches:** `tabs.ts` (`serializeCurrentLayout`, `restoreLayout`,
new migration helper).
**Ships:** "Layout schema is metadata-only by design. Corrupt
state is no longer expressible."

### Phase D — 0.38.3 — Daemon session events + remove cross-window tab sync

Final cleanup. Cross-window sync now flows through the daemon.

- **Daemon emits session events** on a single WS channel:
  - `session_added { workspace, paneGroupId, kind, agent_name }`
  - `session_removed { workspace, paneGroupId }`
  - `session_renamed { workspace, paneGroupId, title }`
- **Every viewer subscribes.** Reacts by re-querying or
  incrementally patching its tab list.
- **Retire:** `sync:tabs-request`, `sync:tabs`, `broadcastAllTabs`,
  `applyRemoteTabChange`. Tauri event-bus tab plumbing comes out.
- This is the wire format the mobile companion uses too — same
  channel, same events. **Mobile parity = free.**

**Touches:** daemon WS layer (new event stream), `useWindowSync`
(remove tab paths), `tabs.ts` (new subscription).
**Ships:** "Every viewer (desktop, focus, mobile) listens to one
canonical event stream."

## Out of scope

- Persisting renderer-specific layout per thin-client (mobile vs
  desktop ordering). Mobile likely wants its own layout column,
  not addressed here.
- Migrating the legacy `session_map` (Kessel) sessions. Out of
  scope per A9 — still Kessel-as-explicit-opt-in.
- Active-viewer resize protocol changes. 0.37.11 protocol stands.
- Manager (inbox) tabs that have no PTY backing. Stays
  ensure-on-open in the renderer; not daemon-driven.
- File-viewer tabs (markdown/PDF/DOCX). Those aren't PTY-backed —
  they stay renderer-local (could move later but no leak today).

## Definition of done

After all four phases ship:

1. Opening a workspace in main, focus window, "New Window", or
   mobile companion shows **identical tab counts**, always equal
   to the daemon's `v2_session_map` for that workspace.
2. `workspace_layouts` JSON contains no tab existence data, only
   positioning. Reading the JSON yields zero tabs by itself.
3. Closing a tab in main makes it disappear in focus window
   within ~50 ms via daemon event, not via Tauri broadcast.
4. Mobile companion can list / focus / spawn / close tabs by
   talking to the daemon over WS, with no special code paths
   different from desktop.
5. No code path in the renderer can produce a tab without a
   matching daemon session — structurally enforced by
   `buildTabsFromDaemon` being the only entry point.

## Open questions

- **Q1:** Should `paneGroup.id` BE the canonical agent_name
  (e.g. `tab-15521cbd-...`), or stay a separate UUID with the
  agent_name as a property? Today it's the latter, and the
  agent_name is reconstructed in `TerminalPane.tsx`. Phase B
  could canonicalise by making `paneGroup.id` ≡ `agent_name`,
  eliminating one indirection. Worth doing if cheap.
- **Q2:** What happens if daemon is restarting or briefly
  unreachable at mount? Phase B should render an empty state
  with a retry, not fall back to layout JSON (that would
  reintroduce the failure mode).
- **Q3:** Pinned-chat sessionId-stamping (0.37.12) currently
  lives in layout JSON. Does it move to daemon side?
  `workspace_sessions.session_id` already holds it on the daemon
  — the layout just mirrors it. Probably drop the layout
  mirror in Phase C.
