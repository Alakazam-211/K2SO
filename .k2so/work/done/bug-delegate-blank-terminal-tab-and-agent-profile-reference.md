---
title: "Bug: delegate opens blank terminal tab (no visible Claude session) + agent.md not referenced"
priority: high
assigned_by: pod-leader@SarahAI
created: 2026-03-31
type: task
source: field-report
---

## Bug 1: Terminal tab opens but Claude session is not visible

### What happens

`k2so delegate` creates the worktree correctly and opens a new terminal tab inside the worktree workspace. However, the tab is **blank** — there is no visible Claude CLI session running in it. The user navigates to the workspace tab and sees an empty terminal.

The delegate output says `Claude session launching...` but the session either:
- Never starts (the `claude` CLI command isn't executing)
- Starts in a detached/background mode that doesn't render to the terminal
- Launches and immediately exits

### Expected behavior

The terminal tab should show Claude actively running — the user should see the Claude CLI interface with the agent already working on its assigned task. This is the whole point of the delegation model: the user visits the worktree workspace and can observe the agent at work.

### Suggested investigation

- Check how the `claude` CLI is being invoked after the tab opens. It may need to be launched as a foreground process attached to the terminal's TTY.
- If using something like `osascript` or terminal APIs to open the tab, the command that runs Claude may need to be passed as the tab's shell command rather than executed after the tab opens.
- Verify the launch command is something like: `cd <worktree-path> && claude` (or whatever the user's default CLI LLM tool is).

---

## Bug 2: agent.md profile not referenced in worktree CLAUDE.md

### What happens

The delegate command writes a CLAUDE.md into the worktree with the agent's identity and task context. However, it does not include a reference to the agent's full profile at `.k2so/agents/<name>/agent.md`. The `agent.md` file contains the agent's detailed profile, standing orders, and accumulated knowledge that may go beyond what the CLAUDE.md contains.

### What NOT to do

Do **not** copy the `agent.md` file into the worktree. The source of truth should remain at `.k2so/agents/<name>/agent.md` in the main workspace. Copying creates drift — if the agent profile is updated, worktree copies become stale.

### Recommended fix

Add a reference line to the CLAUDE.md that the delegate command writes into the worktree. Something like:

```markdown
## Agent Profile
For your full agent profile, standing orders, and accumulated knowledge, see:
`/absolute/path/to/project/.k2so/agents/integrations-eng/agent.md`
```

This way the Claude session knows where to find the deeper context without duplicating it. The absolute path is important since the worktree is a different directory — relative paths back to `.k2so/agents/` won't resolve correctly from the worktree root.
