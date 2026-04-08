// ── Layout constants ──────────────────────────────────────────────────
export const TOPBAR_HEIGHT = 38
export const SIDEBAR_MIN_WIDTH = 180
export const SIDEBAR_MAX_WIDTH = 400
export const SIDEBAR_DEFAULT_WIDTH = 240

// ── Terminal font size constants ──────────────────────────────────────
export const TERMINAL_FONT_SIZE_MIN = 10
export const TERMINAL_FONT_SIZE_MAX = 24
export const TERMINAL_FONT_SIZE_DEFAULT = 13

// ── App constants ─────────────────────────────────────────────────────
export const APP_NAME = 'K2SO'

// ── File size limits ──────────────────────────────────────────────────
export const MAX_FILE_SIZE = 10 * 1024 * 1024  // 10MB
export const MAX_ICON_SIZE = 256 * 1024  // 256KB

// ── Polling intervals ────────────────────────────────────────────────
export const GIT_POLL_INTERVAL = 5000  // 5 seconds
export const FILE_POLL_INTERVAL = 2000  // 2 seconds

// ── Terminal ─────────────────────────────────────────────────────────
export const TERMINAL_SCROLLBACK = 5000

// ── Update check ────────────────────────────────────────────────────
export const UPDATE_CHECK_INTERVAL = 3 * 60 * 60 * 1000  // 3 hours

// ── UI timing ────────────────────────────────────────────────────────
export const CONTEXT_MENU_DISMISS_DELAY = 50  // ms

// ── Resumable CLI tools ─────────────────────────────────────────────
// Tools that support session resume via a --resume flag.
// Used to detect active sessions before app close and restore on reopen.
export const RESUMABLE_CLI_TOOLS: Record<string, { resumeFlag: string; provider: string }> = {
  'claude': { resumeFlag: '--resume', provider: 'claude' },
  'cursor-agent': { resumeFlag: '--resume', provider: 'cursor' },
  'pi': { resumeFlag: '--resume', provider: 'pi' },
}

// ── Agent activity ────────────────────────────────────────────────
export const AGENT_IDLE_THRESHOLD_MS = 5000  // 5 seconds without grid output → idle

// ── Known agent commands ────────────────────────────────────────────
// CLI tools considered "active agents" for close-warning purposes.
// When one of these is the foreground process in a terminal, the user
// is warned before closing the tab or quitting the app.
export const KNOWN_AGENT_COMMANDS = new Set([
  // Cloud CLI agents
  'claude', 'cursor-agent', 'codex', 'gemini', 'copilot',
  'aider', 'opencode', 'codepuppy', 'gpt', 'goose', 'pi',
  // Local/on-device LLM tools
  'ollama', 'llamafile', 'llama-cli', 'interpreter',
  'tenere', 'llm-tui-rs',
])
