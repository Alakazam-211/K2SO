---
title: "Bug: delegate does not launch Claude session inside the worktree workspace"
priority: high
assigned_by: pod-leader@SarahAI
created: 2026-03-31
type: task
source: field-report
---

## Summary

`k2so delegate` creates the worktree correctly but does not launch a Claude CLI session (terminal tab) **inside** the worktree's workspace. When the user navigates to the worktree directory, there is no active Claude terminal session running in it. The agent is effectively assigned work with no one doing it.

## Expected Behavior

When `k2so delegate` runs, it should:

1. Create the worktree (this works)
2. Open a **new terminal tab inside the worktree's workspace/directory**
3. Launch the user's default CLI LLM tool (Claude) **within that tab**, with CWD set to the worktree path

This is critical because:
- Opening the tab **inside** the worktree workspace guarantees the Claude session's CWD is the worktree path — not the parent repo or some other directory
- The user can visit the worktree workspace and **see Claude actively working** on the assigned task
- The worktree contains a full copy of the app, so Claude has access to the complete codebase from the correct context
- The CLAUDE.md in the worktree contains the agent identity and task instructions, which Claude will read on launch

## Current Behavior

The delegate command output says `Claude session launching...` but when the user visits the worktree workspace, there is no terminal session with Claude running in it. The worktree sits idle with zero commits.

## Impact

This is effectively the same as the worktree placement bug in terms of outcome — delegation appears to succeed but no work gets done. The worktree is created, the work item is moved to active, but no agent is actually executing.

## Implementation Notes

The terminal tab should be opened **in the worktree directory** so that:
```
Tab CWD:     /path/to/project/.worktrees/integrations-eng--fix-something/
Claude CWD:  /path/to/project/.worktrees/integrations-eng--fix-something/
```

Claude will then automatically read the CLAUDE.md at the worktree root, pick up its agent identity and task context, and begin working. This is the mechanism that makes the whole pod-mode delegation model function — without it, delegation is just file shuffling.
