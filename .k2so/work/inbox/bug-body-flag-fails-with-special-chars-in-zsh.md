---
title: Bug: --body flag fails with special chars in zsh
priority: normal
assigned_by: user
created: 2026-04-12
type: task
source: manual
---

k2so work create --body fails with special characters in zsh

When passing markdown-like text (backticks, parentheses, arrows) via --body in a heredoc or command substitution, zsh throws:
  (eval):26: unmatched "

Workaround: write body to a temp file and use $(cat /tmp/file).

Suggested fix: add a --body-file <path> flag that reads the body from a file, avoiding shell quoting issues entirely. This is especially important for agents that programmatically create work items with rich text descriptions.

Repro:
  k2so work create --title "test" --body "$(cat <<'EOF'
  Text with backticks like WorkspaceRail component and parens (optional)
  and arrows -> plus more text
  EOF
  )"

Result: (eval):26: unmatched "
