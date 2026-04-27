# K2SO 0.36.6 — Codex chat resume joins the Big 4

The Chats drawer now lists, click-resumes, and auto-restores Codex CLI sessions alongside Claude, Cursor, Gemini, and Pi. With this release, every major coding-CLI provider has parity: chat list filtered to the current workspace, one-click resume from the drawer, on-quit save of the live session id, and on-relaunch auto-restore via the saved id.

## What's new

### Codex sessions in the Chats drawer

Click any Codex session to spawn a tab running `codex resume <uuid>` — picking up exactly where you left off. Sessions are read from `~/.codex/sessions/YYYY/MM/DD/rollout-*.jsonl` and filtered to the active workspace via the literal `cwd` recorded in each rollout's `session_meta` header (line 1). Worktrees collapse back to the parent project the same way Claude sessions do.

Titles are pulled from `~/.codex/history.jsonl` indexed by session id rather than from the rollout file directly. The rollout's first user message is polluted by an injected `AGENTS.md` blob; the flat history is the clean source — one line per real user prompt.

### Subcommand-style resume

Codex is the first provider to use a subcommand for resume rather than a flag. K2SO's data model now distinguishes `resumeFlag` (Claude/Cursor/Gemini/Pi) from `resumeSubcommand` (Codex). Click handler, on-relaunch deserialize, and the launch-default-agent path all branch on which is set. For Codex specifically, preset args are dropped on resume because `codex resume` only accepts a small subset of options — the saved session already carries its own model and permissions from when it was first started.

### On-quit save + on-relaunch auto-restore

Same flow that's been working for Claude/Cursor (and gained Gemini/Pi in 0.36.5). When you Cmd+Q with a live `codex` tab open, K2SO walks the layout and stamps the live session id onto the terminal item before serializing. On next launch, the deserialize path re-spawns the tab as `codex resume <id>` automatically.

## Why we waited

Codex CLI's resume API was labeled `experimental_` for most of late 2025 — the only options were `codex resume --last` (no specific id) or `codex -c experimental_resume="<absolute-path>"` (path-based escape hatch). Codex 0.125 stabilized the contract: `codex resume <SESSION_ID>` (UUIDs take precedence; thread names also work) plus `codex resume --last` and `codex resume --all`. K2SO targets that stable shape — earlier Codex versions won't get one-click resume.

## Upgrade

Use the in-app updater (Settings → General → Check for updates) or download the DMG from the GitHub release page below.
