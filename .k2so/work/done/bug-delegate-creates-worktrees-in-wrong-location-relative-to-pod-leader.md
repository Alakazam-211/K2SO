---
title: "Bug: k2so delegate creates worktrees relative to pod-leader agent dir, not project root"
priority: high
assigned_by: pod-leader@SarahAI
created: 2026-03-31
type: task
source: field-report
---

## Summary

`k2so delegate` always creates worktrees under the pod-leader agent's directory (`.k2so/agents/pod-leader/.worktrees/`) instead of relative to the project root. This makes worktrees invisible to the workspace, unreviewable, and causes agent sessions to produce zero work. Additionally, work files are moved to a phantom nested `.k2so/` path that doesn't correspond to the real workspace structure.

## Reporter Context

This bug was discovered by the **pod-leader agent** in the **SarahAI workspace** at:
```
/Users/z3thon/DevProjects/Peliguard/SarahAI/.k2so/agents/pod-leader/
```
The pod-leader's CLAUDE.md sets CWD to this directory. Every `k2so` command resets the shell CWD back to this path after execution.

## Reproduction

From the SarahAI workspace, as pod-leader:

```bash
# Even when explicitly cd'ing to project root first:
(cd /Users/z3thon/DevProjects/Peliguard/SarahAI && k2so delegate integrations-eng .k2so/agents/integrations-eng/work/inbox/some-task.md)
```

### What happens

1. **Worktree created at wrong path:**
   ```
   ACTUAL:   .k2so/agents/pod-leader/.worktrees/agent-integrations-eng-<task>/
   EXPECTED: .worktrees/agent-integrations-eng-<task>/  (relative to project root)
   ```

2. **Work files moved to phantom nested path:**
   ```
   ACTUAL:   .k2so/agents/pod-leader/.k2so/agents/integrations-eng/work/active/<task>.md
   EXPECTED: .k2so/agents/integrations-eng/work/active/<task>.md
   ```

3. **`k2so reviews` cannot find completed work** because it looks under the project root, not the nested pod-leader path.

4. **Agent Claude sessions produce zero commits** — likely because the session context is wrong when launched from within the nested worktree path.

### Additional CWD-related bugs observed

- `k2so work create --agent <name>` creates files at `.k2so/agents/pod-leader/.k2so/agents/<name>/work/inbox/` instead of `.k2so/agents/<name>/work/inbox/`
- `k2so work create` (workspace-level) creates files at `.k2so/agents/pod-leader/.k2so/work/inbox/` instead of `.k2so/work/inbox/`
- `k2so agents list` intermittently returns "No agents found" — likely searching for `.k2so/agents/` relative to pod-leader CWD
- After every `k2so` command, the shell CWD resets to `.k2so/agents/pod-leader/`

## Impact

**All 5 delegated tasks in the SarahAI workspace produced zero agent work.** The pod-leader delegated 5 bugs to integrations-eng. All 5 worktrees were created, branches were made, Claude sessions were launched — but no code was written. The worktrees were invisible to `k2so reviews` and the user could not see or review them.

This is a **complete blocker for pod-mode operation** — delegation appears to succeed but silently fails.

## Root Cause Analysis

The `k2so delegate` command (and other path-resolving commands) appear to resolve paths relative to `process.cwd()` or the calling shell's CWD, rather than finding the project root first. Since the pod-leader agent's CLAUDE.md is located at `.k2so/agents/pod-leader/`, and the CLI resets CWD there after each command, all path resolution is anchored to the wrong directory.

## Recommended Fix

The k2so CLI should resolve the **project root** on startup by walking up the directory tree to find the `.k2so/` directory — similar to how `git` finds `.git/` regardless of where you invoke it. All internal path operations (worktree creation, work file moves, agent lookups) should be relative to this discovered root.

Specifically:
1. Add a `findProjectRoot()` function that walks up from CWD looking for `.k2so/`
2. Use this root as the base for all worktree paths, work file operations, and agent lookups
3. Ensure the CLI does not change the caller's CWD (use subshells or restore CWD on exit)
4. Worktrees should be created at `<project-root>/.worktrees/<branch-name>/` or a configurable path at the project level

## Test Verification

After fix, this should pass:
```bash
# From any subdirectory within the project
cd /path/to/project/.k2so/agents/pod-leader/
k2so delegate some-agent .k2so/agents/some-agent/work/inbox/task.md

# Worktree should appear at:
#   /path/to/project/.worktrees/agent-some-agent-task/
# NOT at:
#   /path/to/project/.k2so/agents/pod-leader/.worktrees/agent-some-agent-task/

# And work file should move to:
#   /path/to/project/.k2so/agents/some-agent/work/active/task.md
# NOT to:
#   /path/to/project/.k2so/agents/pod-leader/.k2so/agents/some-agent/work/active/task.md
```
