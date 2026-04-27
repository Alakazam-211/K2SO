# K2SO 0.36.5 — Chats drawer: full Big-4 support (Gemini + Pi added)

The Chats drawer now lists, click-resumes, and auto-restores-on-relaunch sessions from all four major coding-CLI providers: Claude, Cursor, Gemini, and Pi. Previously only Claude and Cursor were wired up.

## What's new

### Gemini sessions are first-class citizens

Click any Gemini session in the Chats drawer to spawn a tab running `gemini --resume <uuid>` with your preset args carried through. Sessions are read from `~/.gemini/tmp/<slug>/chats/*.jsonl` and filtered to the current workspace via Gemini's own `~/.gemini/projects.json` slug map (so worktrees collapse back to the parent project the same way Claude sessions do). The first user message becomes the chat title.

A subtlety worth knowing: Gemini's filename only contains an 8-character prefix of the session UUID, but resume needs the full UUID. The parser reads the full id from line 1 of the JSONL header — that's the one place a naive implementation would break.

### Pi sessions are first-class citizens

Same shape as Gemini, with one important difference: **Pi's resume flag is `--session <uuid>`, not `--resume <uuid>`**. (Pi's `--resume` is its interactive session picker — no id arg.) Sessions live at `~/.pi/agent/sessions/<cwd-slug>/<iso-ts>_<uuidv7>.jsonl`. Each session file's line-1 header carries the literal `cwd`, which we use for project filtering — that's more robust than reverse-engineering Pi's slug encoding across worktrees.

### On-quit save + on-relaunch auto-restore for Gemini and Pi

The same flow that's been retiring Claude/Cursor tabs gracefully now extends to Gemini and Pi. When you Cmd+Q with a live `gemini` or `pi` tab open, K2SO walks the layout before serializing and stamps the live session id onto each terminal item. On next launch, the deserialize path re-spawns the tab as `gemini --resume <id>` or `pi --session <id>` so you pick up exactly where you left off — no manual hunt through the Chats drawer to re-resume what you were already working on.

## What's fixed

### Click-to-resume sent the UUID as a chat message for Pi

Pre-0.36.5, the Chats-drawer click handler hardcoded `--resume` as the resume flag for every provider. Claude and Cursor tolerate that fine. Pi parses `--resume` as "open the picker (no id)" and treats the uuid as a positional message argument — so clicking a Pi session opened the picker, then once you selected something, the uuid string got sent as your first chat message. Fixed: the click handler now reads `config.resumeFlag` from the per-provider config (`--session` for Pi, `--resume` for the others).

### Preset args stripped more aggressively

If your Pi (or any) preset had `--resume`, `-r`, `--continue`, `-c`, or `--session` baked into the command string, those would carry through into the resume command and shadow our explicit session selection. The preset-arg filter now drops every session-selection flag before appending the explicit `<resumeFlag> <uuid>`, so a preset like `pi --resume` no longer breaks click-to-resume.

## Known unknown — Codex deferred

Codex CLI's session-resume API is still labeled `experimental_` and hasn't stabilized — `codex resume --last` (no specific-id support) and `codex -c experimental_resume="<path>"` are the only options today. We'll add Codex support once OpenAI ships a stable `--resume <id>` (or equivalent). Planned for a follow-up release.

## Upgrade

Use the in-app updater (Settings → General → Check for updates) or download from the GitHub release page below.
