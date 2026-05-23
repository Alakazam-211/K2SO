---

title: "Bug: Opening same file from file tree creates duplicate tab"
priority: normal
assigned_by: delegated
created: 2026-04-03
type: bug
source: manualworktree_path: /Users/z3thon/DevProjects/Alakazam Labs/K2SO/.worktrees/agent-frontend-eng-duplicate-file-tab-on-reopen
branch: agent/frontend-eng/duplicate-file-tab-on-reopen
---

## Description

When a file is already open in a tab and the user clicks on that same file again in the file tree, it opens in a new tab instead of jumping to/focusing the existing tab. Expected behavior is that it should activate the already-open tab.
