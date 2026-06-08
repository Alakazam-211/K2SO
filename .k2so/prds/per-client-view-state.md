# Per-Client View State — selected tab & focused workspace are local, never canonical

**Status:** DRAFT — **Explore-validated 2026-06-08** (see §10). Renderer-only;
the daemon needs NO change. Key simplification: **focused-workspace is already
per-client** — only the **selected tab** leaks. Recommended release: **0.39.42**
(bundle with the already-verified loop `2c7abf5` + in-place-reorder `5657e93`).
**Author:** pod-leader (with Rosson)
**Related:** #676/#677 (daemon-canonical tab titles/order + broadcasts), #672
(daemon-canonical Active set), the 0.39.42 tab-order echo + in-place-reorder
fixes (`2c7abf5`, `5657e93`). This is the design correction Rosson identified
while testing two Tauri clients on one daemon.

---

## 1. Problem

The daemon-canonical work (#676/#677) correctly made workspace/tab **structure**
shared across clients — but it **also dragged per-client VIEW state into the
canonical plane.** Specifically, `activeTabId` (the user's selected/focused tab)
is serialized **into the workspace layout** that is saved to the daemon
(`workspace-layouts/save`) and adopted by other clients on a `TabOrderChanged`
broadcast (or on cold-load). Consequences:

- **Multi-client hijack:** when client A reorders a tab (or any layout save
  fires), client B re-fetches A's layout and adopts it **including A's
  `activeTabId`** → **B's view jumps to A's selected tab** (`tabs.ts:4389`
  in-place path; `tabs.ts:2994` restoreLayout path). Two users on one server
  cannot explore independently.
- **Cold-load hijack:** a client opening a workspace restores **whoever saved
  last**'s selected tab, not its own.
- **Root:** `activeTabId` lives in the *shared* layout at all. View state and
  structure are conflated.

This violates the intended shape. **A reorder/rename is shared truth; "which
tab/workspace I'm looking at" is mine alone.**

## 2. The two planes (the rule)

| Plane | What | Home | Shared? |
|---|---|---|---|
| **Canonical** | tab existence, **order**, **titles**, splits/pane layout, canonical session bindings; the **Active set** (#672, for reaping) | daemon / `workspace-layouts` / `workspace_sessions` | yes — broadcast + adopted |
| **Per-client** | **selected/active tab**, **focused/foregrounded workspace**, scroll position, cursor, split focus | local to each client/session | **no — never sent as shared truth, never adopted from a peer** |

> The **Active set** (#672) is "which workspaces stay alive for reaping" — a
> union across users, correctly canonical. It is NOT "which workspace I'm
> viewing." Those two got conflated; this PRD separates them.

## 3. Goal / Non-Goals

**Goal.** Make selected-tab and focused-workspace **per-client local state**.
The shared layout stops carrying `activeTabId`. Adoption never changes local
selection or focus. Each client restores **its own** last selection on cold-load.

**Non-Goals.**
- Active **set** stays canonical (reaping) — untouched.
- Tab order / titles / splits stay canonical — untouched.
- No change to the 0.39.42 echo-suppression / in-place reorder mechanics beyond
  the selection handling.

## 4. Design

### 4.1 Strip view state from the canonical layout
- The payload to `workspace-layouts/save` **omits `activeTabId`** (and any other
  pure view-state field). The daemon stores/echoes only structure. If the daemon
  currently persists `activeTabId`, it either drops the column or treats it as
  ignored/legacy.
- **Back-compat:** old stored layouts (and older daemons) may still contain
  `activeTabId`; on load the renderer **ignores** it for selection purposes and
  stops writing it going forward.

### 4.2 Per-client selected-tab store (local)
- Add a **local, per-(client, workspace) selected-tab store** (e.g. a small
  zustand store persisted to `localStorage`, keyed by `projectId:workspaceId`).
  This is the SAME machine/app instance's memory — never transmitted.
- **Cold-load:** restore the client's own last selected tab for the workspace;
  if none (brand-new client, or the tab no longer exists), default to the
  workspace's first tab / the pinned chat (preserve #658 cold-boot pinned-chat
  behavior).
- **Selection write:** when the user selects a tab, update only the local store
  (no daemon round-trip for selection).

### 4.3 Adoption never moves selection (Tier-1, folded in here)
- `tryReorderTabsInPlace` (`tabs.ts:4388-4392`): **drop the `layout.activeTabId`
  override** — keep `state.activeTabId` (valid because the in-place reorder
  preserves tab ids).
- `restoreLayout` adoption fallback (`refetchLayoutForRemoteReorder`): after a
  structural rebuild, **re-anchor selection to the client's local store** (match
  by paneGroupId signature since `restoreLayout` re-mints tab ids), NOT to the
  serialized `activeTabId`.

### 4.4 Focused workspace is per-client
- Audit `projects.ts` `setActiveProject`/`activateProject`: the `projects/activate`
  POST is for the **Active set** (correct, canonical) and must **not** cause the
  daemon to broadcast "focus workspace X" to other clients. Confirm no
  broadcast/event forces a peer's foreground workspace. If one exists, make
  foreground/focus strictly local.

## 5. Edge cases
- Brand-new client, no local selection for a workspace → first tab / pinned chat.
- A locally-selected tab that was **closed on a peer** (no longer exists after a
  structural adoption) → fall back to first tab / pinned chat.
- Single-client (local-only) behavior unchanged: your selection persists locally
  exactly as before, just sourced from the per-client store instead of the
  shared layout.
- Pinned chat default: the canonical pinned tab still exists for everyone
  (structure); whether it's *selected* is per-client.

## 6. Wire / schema impact
- `workspace-layouts/save` request: drop `activeTabId` from the serialized
  layout (or daemon ignores it). No new routes. The `TabOrderChanged` broadcast
  already carries no selection — unchanged.
- New **local** store only; no daemon schema addition required. (If the daemon
  has an `active_terminal_id`/selected column that's per-client, confirm it
  isn't being treated as canonical — see §8.)

## 7. Test plan
- **Multi-client (the core lock):** client A reorders / renames / saves → client
  B's **selected tab is unchanged** (and vice-versa). Two clients hold different
  selections simultaneously.
- **Cold-load per-client:** client restores ITS OWN last tab regardless of a
  peer's last save.
- **Focused workspace:** A focusing workspace X does not move B's foreground.
- **Regression:** 0.39.42 echo-suppression + in-place reorder tests stay green;
  single-client selection persistence across reload still works (#658).

## 8. Open questions for the Explore validation pass
1. **Where does `activeTabId` flow today** — enumerate every read/write:
   `serializeCurrentLayout`/`restoreLayout`, `workspace-layouts/save`+`/load`,
   the daemon's layout storage, and any `workspace_sessions.active_terminal_id` /
   `workspace_tab_sessions` columns. Which are canonical vs incidental?
2. Is there an **existing local/per-client store** for selected tab, or must we
   add one? Where is selection currently sourced on render (TabBar/TerminalArea)?
3. Does the daemon **broadcast anything that forces focused-workspace** across
   clients (beyond the Active set)? (`projects/activate`, session_events.)
4. How does **cold-boot pinned-chat restore (#658)** pick the active tab — will a
   per-client store conflict with it?
5. Does `active_terminal_id` on `workspace_sessions` represent a **per-client**
   notion that's wrongly shared, or is it the canonical live-PTY pointer (then
   it's fine)? Don't confuse "active terminal (PTY handle)" with "active tab
   (user selection)".
6. Any **host-switch** (#625) reset logic that already treats selection as
   per-machine — reuse it?

## 10. Validation findings (Explore, 2026-06-08 — resolved)

- **`activeTabId` flow:** written in `serializeCurrentLayout` (`tabs.ts:2851`,
  + per-split-group at `:2846`) → POSTed in `workspace-layouts/save`
  (`:3128`) → on adoption applied at `tabs.ts:4389` (in-place) and `:2993-2999`
  / `:3082` (restoreLayout). The **daemon stores the JSON blob but never reads
  `activeTabId`** (`db_routes.rs:588-631`) and the `TabOrderChanged` broadcast
  carries no selection (`session_events.rs:268-274`) — so **no daemon change is
  needed**; it's incidental round-tripped JSON.
- **No per-client store today:** selection is 100% sourced from the shared
  layout's `activeTabId` (`useTabsStore.activeTabId`; read at `TabBar.tsx:22`,
  `TerminalArea.tsx:150`). We must add one.
- **Focused workspace is ALREADY per-client:** `activeProjectId`/
  `activeWorkspaceId` are local to each window; `projects/activate` only mutates
  the canonical Active set (reaping) and broadcasts nothing that moves a peer's
  foreground (`projects.ts:67-78,478-487`). **§4.4 is already satisfied** — just
  add a regression test, no code change.
- **`active_terminal_id` (canonical live-PTY pointer) ≠ `activeTabId` (UI
  selection)** — correctly separate, not conflated.
- **#658 cold-boot pinned-chat:** unaffected — the per-client store's empty-case
  fallback (first tab / pinned chat) preserves it.
- **Host-switch:** `tabs.ts` has NO `onActiveHostChange` reset today; the new
  per-client store should subscribe (`connect-host.ts:170`) and reset on switch.

### Locked implementation (renderer-only, 3 phases)
1. **Strip selection from the canonical layout** — remove `activeTabId` from
   `serializeCurrentLayout` (`tabs.ts:2846,2851`). Old layouts with it are
   harmless (ignored). On `restoreLayout`, stop reading `layout.activeTabId`.
2. **Per-client selected-tab store** — new `src/renderer/stores/selected-tabs.ts`
   (zustand + localStorage, keyed `projectId:workspaceId`); source selection
   from it on restore (fallback: first tab / pinned chat); write it from
   `setActiveTabInGroup`; reset on `onActiveHostChange`. (Requires threading
   `projectId`/`workspaceId` into the tabs store, set in `loadLayoutForWorkspace`.)
3. **Adoption never moves selection** — delete the `layout.activeTabId` override
   in `tryReorderTabsInPlace` (`tabs.ts:4389-4395`); after the restoreLayout
   fallback, re-anchor to the local store (not the serialized id). All within the
   existing 0.39.42 save-suppression window.

### Tests
Multi-client: A reorders → B's selection unchanged; A & B hold different
selections at once. Cold-load: each client restores its OWN last tab. Single-
client: select → reload → same tab (localStorage). Focus isolation: A's
foreground change doesn't move B. Keep the 0.39.42 echo + reorder suite green.

## 9. Rollout
- Single renderer-focused change (plus possibly dropping a daemon field).
- Ships as its own release after the 0.39.42 loop+flicker fix (which is already
  verified). Recommend **0.39.43**. Explore-validate §8, then implement +
  multi-client live-verify (two Tauri clients on one daemon — the same harness
  that surfaced this).
