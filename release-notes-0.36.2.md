# K2SO 0.36.2 — Default-preset overhaul, real LLM provider icons, Heartbeats settings polish

This release replaces every default-preset emoji with the real provider mark (Claude, Codex, Gemini, Cursor, Pi, OpenCode, Goose, Aider, Ollama, Copilot, Open Interpreter), reorders the built-in list to reflect actual usage, drops Code Puppy as a built-in (you can still add it as a custom preset), and lands a long backlog of Settings UI polish — most visibly the Wake Scheduler page renamed and re-themed as "Heartbeats."

## What's new

### Real LLM provider icons everywhere

Every spot that surfaces an agent — the launch bar, terminal tab headers, the saved-chats drawer, and the Editors & Agents settings page — now renders the actual brand mark instead of a llama emoji or a generic prompt glyph. We vendored ten SVGs locally (no runtime URL fetches, fully offline-safe) sourced from Lobe Icons (MIT), Simple Icons (CC0), and the official `pi.dev` and `openinterpreter.com` logos. Color marks (Claude, Codex, Gemini, Copilot) keep their brand fills; monochrome marks (Cursor, Goose, Ollama, OpenCode, Pi, Open Interpreter) use `currentColor` so they adapt to the app theme. Aider keeps its custom green-A glyph because Aider's official mark is a horizontal wordmark, illegible at icon sizes.

The previous behavior was that the seed planted a 🦙 / 🌐 / 🦢 emoji as the icon for Ollama, Open Interpreter, and Goose, which beat the SVG renderer. Those overrides are now cleared.

### Default presets reordered, Code Puppy removed, Pi added

The new canonical order for fresh installs:

1. Claude
2. Codex
3. Gemini
4. Cursor Agent
5. Pi
6. OpenCode
7. Goose
8. Aider
9. Ollama
10. Copilot
11. Open Interpreter

Code Puppy was removed as a built-in. The CLI is still supported as a custom preset — add it manually if you use it. Pi is now a default built-in (it wasn't in the Rust seed before, only in the Tauri "reset to defaults" list), so fresh installs and "Reset Built-ins" both surface it.

**On upgrade**, existing users keep their re-orderings and customizations untouched. Two automatic things happen on first launch:

- Code Puppy is removed from your built-in list (one-shot DELETE migration).
- If you didn't already have Pi, it gets inserted at its canonical position. Users who already have a Pi entry from a previous "Reset to defaults" keep theirs as-is.

### Settings → Heartbeats (was: Wake Scheduler)

The settings page that controls how `launchd` fires heartbeats has been renamed and re-themed to match the rest of the settings shell:

- Square corners, theme-matching font sizes, beta pill instead of `(BETA)`.
- Width constrained to one third of the page so future right-side panels (fire history, preview) have somewhere to live.
- The "Wake system from sleep" toggle now matches the Mobile Companion toggle exactly (`w-7 h-3.5` outer, `w-2.5 h-2.5` thumb).
- Conditional dividers: the bottom border on the Mode block only renders when "Heartbeat every N minutes" is selected, so the page no longer shows doubled separator lines above the Apply button.
- Dirty-state tracking rewritten as a deep-equality check against the last-applied snapshot instead of a manual flag, fixing the bug where flipping a setting back to its applied value still showed "Unsaved changes" or where Apply seemed to no-op.

### Reset Built-ins is now a real reset

The **Reset Built-ins** button on Editors & Agents now wipes every `is_built_in = 1` row before re-seeding, instead of only deleting the IDs in its current list. This drops Code Puppy, repairs any stale rows from the older bug where two seed lists disagreed on the Pi/Goose/Ollama/Interpreter ID-to-name mapping, and gives you a guaranteed-clean canonical state. Custom presets you've added are not touched.

### Settings nav reordered

In the left sidebar of the Settings panel: Timer dropped to the bottom, Keybindings sits above it, Mobile Companion and Heartbeats moved under Editors & Agents to keep the agent-related sections grouped.

### Mobile Companion deprecation notice

The Mobile Companion settings page now states explicitly that 0.29.x is the last K2SO version that supports the legacy mobile app, and that mobile-app support will return in a future version. The companion HTTP/WS endpoints are still in the daemon for that future work; only the user-facing app pairing is on pause.

### Beta pill standardized

Every place that used `(BETA)` text — Heartbeats section, Mobile Companion, Agentic Systems toggle, Agent Settings — now renders the same square `bg-[var(--color-accent)]/15` pill.

### Heartbeats panel polish

The workspace-tab Heartbeats list now shows a count badge next to the section header (omits archived), and rows sort with disabled heartbeats pushed to the bottom so the live ones stay near the top.

## Internal cleanup

`AGENT.md` was previously labeled as "Code Puppy" in the harness-discovery comments, the diagram on Workspaces → Agent Settings, and the file-collision preview. AGENT.md is now a multi-tool standard (per the `agent.md` spec), so all those references say "agent.md spec" instead. The plumbing didn't change — only the wording.

## Upgrade

Use the in-app updater (Settings → General → Check for updates) or download the DMG from the GitHub release page below.
