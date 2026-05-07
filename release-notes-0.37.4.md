## Highlights

**Agent display name & daemon-owned tab labels.** Two coordinated changes that retire the long-running tab-title fight between the renderer and the PTY:

- **Phase A — Agent display name:** the workspace's primary agent now has a friendly, user-editable label that lives in `.k2so/agent/AGENT.md` frontmatter as `display_name:`. Renaming via Settings or the CLI updates one file; pinned tab headers and persona prompts read it via a single helper. The technical agent name (which keys infrastructure layers like `v2_session_map` and `terminal_id`) stays stable, so live PTYs aren't dropped on rename.
- **Phase B — Daemon-owned session labels:** every v2 session now carries an authoritative label string the daemon owns. PTY title events (e.g. `claude --resume` emitting "Claude Code") are absorbed daemon-side via a `LabelSource` state machine (`Pty | Seed | Locked`) — tabs spawned with a known label and `lock=true` are immune to PTY clobber. The new WS protocol broadcasts `LabelInitial` (on connect) and `LabelChanged` (mid-session) events; multiple Tauri windows of the same workspace stay in sync.

The renderer-side `TabTitleSource` band-aid from earlier 0.37.x dev iterations is gone; tab labels flow daemon → WS → renderer cleanly.

## What you'll see

- Pinned **Chat tab** in-pane header shows the agent's friendly display name (not `__lead__` or whatever technical key)
- Pinned **Inbox tab** header reads the same display name
- **Settings → workspace card** has a single-row "Agent display name" input + Save button + Manage Persona button (was previously stacked, locked, or revert-on-edit)
- **Restored chat-history tabs** (e.g. a session named "Cortana") keep their title — `claude --resume` no longer overwrites with "Claude Code"
- **Manager card** in Settings has a display name input — internal `__lead__` routing key is unchanged

## CLI additions

```bash
# Read the workspace agent's friendly name
k2so workspace agent-name <workspace>

# Rename it (atomic AGENT.md frontmatter rewrite + live session label push)
k2so workspace set-agent-name <workspace> <new-name>

# Set an explicit label on any v2 session (locks by default; --no-lock allows PTY updates)
k2so sessions set-label <session-id> <label> [--no-lock]
```

## Architecture

- New helper: `k2so_core::agents::display::agent_display_name(path)` (mtime-cached, total — always returns a string)
- New endpoint: `POST /cli/sessions/label?id=<uuid>&label=<text>[&lock=true|false]`
- New endpoint: `GET /cli/workspace/agent-display-name?project=<path>`
- New endpoint: `GET /cli/workspace/set-agent-display-name?project=<path>&name=<text>`
- `DaemonPtySession` carries `label: RwLock<String>` + `label_source: RwLock<LabelSource>` + `label_tx: broadcast::Sender<String>`
- `SpawnWorkspaceSessionRequest` accepts optional `label` + `label_locked` fields
- WS protocol gains `LabelInitial` + `LabelChanged` events; `Title` events still flow for activity-marker detection (idle/working hints) but no longer mutate tab labels
- Tauri commands route through the daemon HTTP API (daemon-first); `k2so-core` writes to AGENT.md atomically with safe line-anchored frontmatter parsing

## Tests

754 tests passing across the workspace. New coverage:
- 13 frontmatter-rewrite tests including dashes-in-values, body-preservation, CRLF, comments-with-shadow-prefix, indentation preservation
- 5 daemon-pty label state-machine tests covering Pty/Seed/Locked transitions
- Two pre-existing canonical-key tests in `session_stream_egress.rs` / `session_stream_ingress.rs` repaired to match post-0.36.15 prefixed-key behavior

## Notes

- Workspaces opened before 0.37.4 with no `display_name:` field fall back to AGENT.md `name:`, then `projects.name`, so the experience degrades gracefully — no migration required
- The `label_locked` flag on the canonical workspace+agent session is the architectural fix for the "Claude Code" tab-title clobber; the older renderer-side `TabTitleSource` field has been retired
