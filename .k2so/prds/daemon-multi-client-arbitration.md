# Daemon Multi-Client Arbitration — active-viewer dimensions + explicit session selection

**Status:** DRAFT (proposed for 0.39.43) — pending Explore validation pass (Issue B)
**Author:** pod-leader (with Rosson)
**Related:** #672 (canonical Active), #683 (daemon-owned pinned chat), #679
(SQLite-canonical session selection), #691 (remote clips bottom ~3 rows), the
0.39.41 SSOT resolver, the 0.39.42 multi-client tab fixes + per-client view
state. Found by Rosson dialing in two-client behavior post-0.39.42.

Two nuanced multi-client issues, both **daemon-side** (the renderer already
sends the right signals). Theme: **the daemon must arbitrate between clients** —
size the PTY for the *active viewer*, and honor an *explicit* session choice.

---

## Issue A — PTY size must follow the active viewer (and remember their dims)

### Problem
When ≥2 clients attach to one PTY, the wrong client's terminal size can drive
the PTY → the active viewer sees a mis-sized grid (the #691 "remote clips
bottom ~3 rows"). Concretely: the **local window keeps winning** — on a tab
refresh / re-mount the renderer re-fires `set_active:true` and re-claims the
slot; the local window (no tunnel latency) re-claims fastest, so the remote
viewer's resizes are dropped. (The 0.39.42 tab-order loop fix already cut the
*constant* thrash, but any refresh still re-claims.)

### Current mechanism (validated)
- The active viewer is stored on the session map as
  `DaemonPtySession.active_subscriber` (`AtomicU64`).
- A `Resize{cols,rows}` is applied **only if** the sender is the active
  subscriber (`active == 0 || active == subscriber_id`) — `sessions_grid_ws.rs:
  461-483`.
- `SetActive{active:true}` → **most-recent-claim-wins**: stores `subscriber_id`
  (`sessions_grid_ws.rs:500-518`, `decide_set_active`).
- Renderer claim predicate: `computeDesiredActive = visible && paneFocused &&
  windowFocused` (`terminal-v2/activeViewer.ts:33`); claim sent from
  `TerminalPane.tsx` (~1482-1602) as `{action:'set_active', active}`.

### Gaps
1. **Dims aren't recorded with the viewer.** The map stores only the subscriber
   **id**, not their `cols/rows`. So when the active viewer changes, the PTY is
   NOT resized to the new viewer's size until that viewer happens to send a
   `Resize`. Rosson's ask — *save the active viewer's dimensions on the session
   map and consume those* — is unmet.
2. **Bare re-mount re-claims.** A refresh/re-mount re-asserts `set_active:true`
   even when focus didn't truly change, letting the local window re-steal the
   slot from a remote viewer.

### Design (daemon-side, small renderer addition)
1. **Carry dims in the claim.** Extend the grid-WS inbound `SetActive` to
   `{action:'set_active', active:true, cols, rows}` (the renderer already knows
   its current viewport — it sends a `Resize` right after today). Back-compat:
   `cols/rows` optional; absent → behave as today.
2. **Record + apply on the session map.** On a real claim, store the active
   viewer's dims on `DaemonPtySession` (e.g. `active_cols`/`active_rows`
   `AtomicU16`) AND immediately `session.resize(cols, rows)` so the PTY snaps to
   the active viewer's size the instant they become active — no waiting for a
   follow-up `Resize`. This makes "most-recent viewer drives size" true.
3. **Don't re-claim on a bare re-mount.** Renderer: only emit `set_active:true`
   on a genuine focus transition, not on every mount (guard via the existing
   `computeDesiredActive` + a "last-sent" ref so a re-mount with unchanged focus
   inputs doesn't re-claim). A window that isn't the focused viewer must
   `set_active:false` (release) so a remote viewer's claim sticks.

### Edge cases
- Single client (no `set_active` ever) → `active==0` first-resize-wins, unchanged.
- A viewer claims, then its window blurs → it releases; the remaining viewer (if
  any) reasserts and the PTY resizes to them.
- Headless / no viewer → PTY holds its last size (harmless).

---

## Issue B — explicit dropdown session selection must win over auto-converge

### Problem
On the pinned chat tab, picking a different chat session from the dropdown does
nothing — the same chat stays loaded. SSOT works; the explicit selection isn't
taking. Happens local **and** remote.

### Current flow (validated)
- **Renderer (correct):** `AgentChatPane.handleSwitchSession` (`:517-540`) →
  `GET workspace/set-chat-session {project, session_id:B}` →
  `WorkspaceSession::update_session_id(B)` (`workspace_routes.rs:102`) → then
  `ensure(forceRespawn:true)`.
- **Daemon:** `ensure_pinned_chat(force_respawn=true)` (`pinned_chat.rs:125`) →
  `resolve_resume_chat_args` (`resume_chat.rs:79`).

### Suspected root cause (CONFIRM in Explore pass)
The **0.39.41 converge fallback** (`newest_claude_session_on_disk`,
`resume_chat.rs:~134-155`) overrides the just-selected id: if
`claude_session_file_exists(B)` returns false at resolve time, the resolver
resumes the *newest* on-disk session instead **and re-persists it** —
overwriting the user's explicit choice back to the old session. The converge
fallback (a GH#24 *auto-recovery* mechanism) and an *explicit user gesture* must
be distinguished — **explicit selection must win.**
> Must confirm: does `claude_session_file_exists(B)` actually fail for a
> dropdown-listed (on-disk) B (path/slug mismatch?), or is the no-op elsewhere
> (force_respawn not tearing down → reused PTY still on A; or renderer not
> re-attaching to the new session after `ensure`)? The fix differs accordingly.

### Design (daemon-side)
Make an **explicit selection** authoritative:
- `set-chat-session` marks the choice explicit (e.g. a transient
  `workspace_sessions.explicit_session_override` flag, or a param threaded into
  the next `ensure`).
- The resolver, when the selection is explicit, **honors the chosen id directly
  and skips the converge fallback** (it still verifies the id exists on disk; if
  it genuinely doesn't, surface an error rather than silently resuming a
  different session). The converge fallback remains for the *auto* path (stale
  saved id, no explicit gesture) — GH#24 stays fixed.
- Clear the flag after the respawn lands on the chosen id.
- Ensure `force_respawn=true` truly tears down the old PTY and the renderer
  re-attaches to the new session id returned by `ensure`.

### Edge cases
- Brand-new workspace (`session_id` NULL, nothing on disk) → still mints
  (#681), explicit flag is irrelevant there.
- Explicit pick of a session whose `.jsonl` is genuinely gone → error/toast, do
  NOT silently swap to a different one.

---

## 3. Tests
- **A:** active-viewer claim with dims resizes the PTY immediately; a second
  client claiming active re-sizes to *its* dims; a bare re-mount with unchanged
  focus does NOT re-claim; release on blur. Live: two clients, the focused one's
  size is consumed (the other no longer "wins").
- **B:** pick session B from the dropdown → daemon respawns `claude --resume B`,
  `workspace_sessions.session_id == B` (not reverted), renderer renders B —
  local AND remote. Auto-recovery (no explicit pick, stale saved id) still
  converges (GH#24 regression stays green). Brand-new workspace still mints.

## 4. Open questions for the Explore validation pass (Issue B)
1. Does `claude_session_file_exists(B)` pass for a dropdown-listed B? If not,
   why (slug/path resolution vs the dropdown's session source)?
2. On `force_respawn=true`, does the old PTY actually tear down, or does the
   reused-PTY idempotency path keep the old session live?
3. After `ensure`, does `AgentChatPane` re-attach the grid-WS to the NEW
   session id from the response, or stay on the old one?
4. Cleanest place for the "explicit" signal — a DB flag on `workspace_sessions`,
   or a param on `ensure-pinned-chat` / `resolve_resume_chat_args`?

## 5. Rollout
Daemon-side core for both; small renderer addition for A (dims in the claim +
release-on-blur) and B (signal explicit). Ships as **0.39.43**. Explore-validate
Issue B's exact override point, implement, then the two-Tauri-client live verify
(the harness that's been working), then release.
