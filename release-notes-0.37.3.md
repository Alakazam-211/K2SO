# K2SO 0.37.3 — `terminal read` works on canonical workspace+agent PTYs

Closes the read-side gap on v2 sessions. Filed by the nsi-checkin
Scout deployment as their second priority for the SMS bridge:
operators had no way to peek at a workspace's live PTY state
between turns, so anything that happened in the TUI between
"agent received input" and "agent wrote to conversation.md" was
invisible — including permission prompts, tool-call stalls, and
error states.

## What changed

`k2so terminal read <id> --lines N` now reads from the canonical
workspace+agent PTY's live alacritty `Term` grid. Three id forms
all work and produce the same response shape:

```bash
# 1. Canonical workspace+agent key — name-keyed lookup
k2so terminal read <project_id>:<agent_name> --lines 30

# 2. v2 session UUID — direct
k2so terminal read <v2_session_id> --lines 30

# 3. Sub-terminal SessionId — for `terminal spawn --command` results
k2so terminal read <subterminal_session_id> --lines 30
```

All three return `{"lines": ["row1", "row2", ...]}`.

## What you can do with it

The intended use cases from the original issue:

- **Health probe before inject** — peek the last few lines to
  confirm the agent is at a prompt, not stuck on a permission
  dialog or a runaway tool call.
- **Operator visibility** — when `conversation.md` hasn't moved
  in N minutes, capture the PTY tail for diagnostics without
  opening the K2SO sidebar.
- **Closing the loop on automation** — webhook integrations
  can verify agent state between turns.

## Architecture note

This is *not* a fallback to the old read path. The v2 grid IS the
primary read surface for any canonical workspace+agent PTY (every
post-0.37.0 workspace's pinned chat tab is one). The
session_stream registry path remains for sub-terminals spawned via
`terminal spawn --command "..."` — those are a separate facility,
not "old."

The render path locks the session's `Term`, calls the existing
`snapshot_term` projection (the same one the WS streaming protocol
uses), and joins each row's `CellRun.text` values. Trailing-empty
rows are trimmed so an idle terminal doesn't return dozens of
blank lines.

## Tests

3 new integration tests in
`crates/k2so-daemon/tests/terminal_routes_integration.rs`:

- `read_returns_grid_lines_for_v2_session_by_session_id` — spawns
  a v2 session running printf, asserts the printf output surfaces
  in the read response.
- `read_resolves_canonical_workspace_agent_key` — same coverage
  via the `<pid>:<agent>` lookup form.
- `read_unknown_canonical_key_returns_clear_error` — pins the
  error contract for missing keys so callers can detect
  "no live session" vs. genuine errors.
