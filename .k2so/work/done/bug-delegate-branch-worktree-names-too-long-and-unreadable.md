---
title: "Bug: delegate creates unreadable branch/worktree names from full task title slug"
priority: normal
assigned_by: pod-leader@SarahAI
created: 2026-03-31
type: task
source: field-report
---

## Summary

`k2so delegate` generates branch and worktree directory names by slugifying the entire work item title. This produces names that are far too long to visually scan, especially in `git worktree list`, `git branch`, or terminal tab titles.

## Current Behavior

Given a task titled "Bug: mailgun_domains_verify and dmarc require domain param not exposed", the delegate command creates:

```
Branch:   agent/integrations-eng/bug-mailgun-domains-verify-and-dmarc-require-domain-param-no
Worktree: .worktrees/agent-integrations-eng-bug-mailgun-domains-verify-and-dmarc-require-domain-param-no
```

In `git worktree list`, with 5 tasks delegated to the same agent, the output looks like:
```
.worktrees/agent-integrations-eng-bug-inboxes-daily-sent-requires-inboxids-but-cli-mcp-don-t-p
.worktrees/agent-integrations-eng-bug-mailgun-domains-list-requires-location-id-but-cli-mcp-do
.worktrees/agent-integrations-eng-bug-mailgun-domains-verify-and-dmarc-require-domain-param-no
.worktrees/agent-integrations-eng-bug-mailgun-sent-flag-false-despite-mailgun-sent-at-timestam
.worktrees/agent-integrations-eng-bug-supabase-campaigns-store-stale-mailgun-domain-after-doma
```

These are nearly indistinguishable at a glance. The meaningful differences are buried at the end, often truncated by the terminal.

## Expected Behavior

Short, scannable names that a human can differentiate instantly. For example:

```
Branch:   agent/integrations-eng/fix-mailgun-verify-params
Worktree: .worktrees/integrations-eng--fix-mailgun-verify-params
```

A full listing would then look like:
```
.worktrees/integrations-eng--fix-inboxes-daily-sent-params
.worktrees/integrations-eng--fix-mailgun-list-params
.worktrees/integrations-eng--fix-mailgun-verify-params
.worktrees/integrations-eng--fix-mailgun-sent-flag
.worktrees/integrations-eng--fix-stale-campaign-domain
```

## Recommendations

1. **Cap the slug length** — e.g. 40-50 chars max for the task portion of the name
2. **Extract keywords rather than slugifying the entire title** — strip filler words like "requires", "but", "don't", "parameter", etc.
3. **Drop the `agent/` prefix on the worktree directory name** — the branch can keep it for git namespace purposes, but the directory name should be optimized for visual scanning
4. **Consider allowing a `--slug` flag on `k2so work create`** so the pod-leader can set a short name at task creation time (e.g. `--slug fix-mailgun-verify-params`)
