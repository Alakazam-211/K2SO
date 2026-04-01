---
title: Feature: add agent delete/remove CLI command
priority: low
assigned_by: user
created: 2026-03-30
type: task
source: issue
---

## Summary

There is no CLI command to delete an agent from a workspace. When an agent is created by mistake or becomes obsolete, the only option is to manually `rm -rf` the directory.

## Proposed

```bash
k2so agents delete <name>          # Remove agent and its work directories
k2so agents delete <name> --force  # Skip confirmation
```

## Acceptance Criteria

- Refuses to delete if agent has active work items (unless --force)
- Removes .k2so/agents/<name>/ directory
- Updates any generated CLAUDE.md that references the agent
- Cannot delete pod-leader
