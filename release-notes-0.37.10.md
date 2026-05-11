## Hotfix: `k2so work send` no longer silently misroutes to the local inbox

A CLI argument-validation bug that's been around since `work send` shipped: if the user omitted `--workspace`, the CLI sent no `workspace` param to the daemon and the daemon silently fell back to the source workspace's `project_path`. The work item landed in the **source's** local inbox with a normal-looking success JSON response — no error, no warning, just a misroute.

Filed in C3PO as *"Issue: k2so work send silently misroutes to local inbox (intermittent, no error)"*. Reproduced 100% deterministically:

```bash
# Old behavior — silent misroute
$ k2so work send --title "send this to Cortana"
{"filename":"send-this-to-cortana.md","title":"send this to Cortana",...}
# ↑ no error, but item is in the CURRENT workspace's inbox, not Cortana's
```

The "intermittent" framing in the report is the user not always remembering `--workspace`. Identical command syntax to `work create`; easy to slip up.

## Fix

CLI now requires `--workspace` for `work send` and points users at `work create` when they meant the local inbox:

```bash
$ k2so work send --title "x"
Error: --workspace is required for 'work send' (it targets another workspace).

Usage: k2so work send --workspace <path-or-name> --title "..." [--body "..."|--body-file <path>]

To add work to YOUR OWN workspace's inbox, use:
  k2so work create --title "..." --body "..."
```

Exit code 1, no daemon round trip, no misroute possible.

## Scope

- **One line changed** in `cli/k2so` — the `cmd_work_send` argument validator now checks `--workspace` before `--title`.
- No daemon change. No DB change. No renderer change.
- Existing scripts that pass `--workspace` continue to work exactly as before.
- Existing scripts that *don't* pass `--workspace` will start erroring with a clear message instead of misrouting.

## Tests

759 daemon + core tests still passing (same baseline as 0.37.9). Manual diagnostic in the K2SO repo: from one workspace, ran `k2so work send --workspace <other-path> --title "x" --body "y"` → item correctly landed in target's `.k2so/work/inbox/`, NOT in source's. With `--workspace` omitted → CLI errors with exit 1 and helpful usage hint.
