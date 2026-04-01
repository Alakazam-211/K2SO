---
title: Bug: em dash (â) in agent role text stored as mojibake (Ã¢)
priority: normal
assigned_by: user
created: 2026-03-30
type: task
source: issue
---

## Summary

When creating agents via `k2so agents create <name> --role "..."`, em dash characters (â, U+2014) in the role string are stored as `Ã¢` in agent.md files. This is a UTF-8 encoding issue â the three bytes of the em dash (E2 80 94) are being interpreted as separate characters.

## Reproduction

```bash
k2so agents create test-agent --role "Test â just testing"
cat .k2so/agents/test-agent/agent.md
# Shows: Test Ã¢ just testing
```

## Affected Files

All agent.md files created via CLI that contain em dashes:
- .k2so/agents/rust-eng/agent.md
- .k2so/agents/frontend-eng/agent.md
- .k2so/agents/cli-eng/agent.md
- .k2so/agents/qa-eng/agent.md

Also affects workspace inbox display and CLAUDE.md generation.

## Expected Fix

Ensure the CLI and/or the K2SO server handle UTF-8 strings correctly when writing agent profiles. The issue may be in the Bash CLI's HTTP encoding, the Rust server's request parsing, or the file write path.
