<p align="center">
  <img src="resources/icon.png" width="128" height="128" alt="K2SO">
</p>

<h1 align="center">K2SO</h1>

<p align="center">
  <strong>Your AI Workspace IDE</strong>
</p>

<p align="center">
  <a href="https://github.com/Alakazam-211/K2SO/blob/main/LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue.svg" alt="MIT License"></a>
  <a href="https://k2so.sh"><img src="https://img.shields.io/badge/download-k2so.sh-8B5CF6.svg" alt="Download"></a>
  <img src="https://img.shields.io/badge/platform-macOS-lightgrey.svg" alt="macOS">
  <img src="https://img.shields.io/badge/built_with-Tauri_v2-24C8D8.svg" alt="Tauri v2">
</p>

---

K2SO is **not a traditional IDE**. It is a workspace for orchestrating AI coding agents through CLI tools, managing git worktrees, and reviewing documents -- all from a terminal-first interface built with Tauri and Rust.

Think of it as a command center: you add your projects, launch AI agents (Claude, Codex, Gemini, Copilot, Aider, and more) in GPU-accelerated terminals, review markdown and PDFs in dark-themed viewers, and organize everything with focus groups and workspace layouts.

<!-- TODO: Add screenshot -->
<!-- ![K2SO Screenshot](docs/screenshot.png) -->

## Features

### AI Workspace Assistant (Cmd+L)
A local LLM (GGUF via llama.cpp with Metal acceleration) translates natural language into workspace operations -- split panes, open files, launch terminals, arrange layouts -- without leaving the keyboard.

### CLI Agent Integration
Quick-launch buttons for the agents you already use:
- **Claude** -- `claude --dangerously-skip-permissions`
- **Codex** -- `codex` with configurable reasoning effort
- **Gemini** -- `gemini --yolo`
- **Copilot** -- `copilot --allow-all`
- **Aider**, **Cursor Agent**, **OpenCode**, **Code Puppy**

Add your own custom agent presets or edit the built-ins.

### Document Review
View `.md`, `.pdf`, and `.docx` files inline with a dark-themed viewer. Markdown renders with GFM support; PDFs use pdf.js; Word docs convert via mammoth.

### Terminal-First
GPU-accelerated terminals via xterm.js + WebGL renderer with Unicode 11 support. Split into up to 3 independent tab group columns, each with their own tab bar. Drag tabs between columns. Resize columns freely. Natural text editing (macOS-style Opt+Arrow word navigation, Cmd+Arrow line navigation) enabled by default.

### Chat History & Session Resume
View Claude and Cursor chat history in the sidebar. Click a session to resume it. When the app closes, terminal sessions are saved and resumed on reopen with `--resume` flags. Fresh chat tabs auto-rename to match the conversation title.

### Terminal Persistence
Terminal PTYs survive tab switches via a scrollback buffer architecture. Switch tabs freely without losing terminal state -- output is buffered and replayed on reattach.

### Git Worktree Management
First-class support for git worktrees. Create worktrees from new or existing branches. Automatic workspace record creation. Projects can run in worktree mode or standard mode.

### Focus Groups & Pinned Workspaces
Group related projects together with focus groups. Pin specific workspaces above the focus group filter so they're always accessible regardless of which group is active.

### Layout Persistence
Workspace layouts (tab groups, open documents, terminal sessions) save and restore automatically when you switch between workspaces.

### Icon Cropping
Upload custom workspace icons with a built-in crop dialog -- drag to position, scroll to zoom, apply to save.

### Built with Tauri + Rust
~5MB binary. Native PTY management via `portable-pty`. SQLite database via `rusqlite`. Git operations via `git2`. Local LLM inference via `llama-cpp-2`. No Electron, no bloat.

## Installation

### Download
Get the latest release from [k2so.sh](https://k2so.sh) or the [GitHub Releases](https://github.com/Alakazam-211/K2SO/releases) page.

### Build from Source

**Prerequisites:**
- [Rust](https://rustup.rs/) (stable)
- [Node.js](https://nodejs.org/) 18+ (or [Bun](https://bun.sh/))
- cmake (for llama.cpp compilation)
- Xcode Command Line Tools (macOS)

```bash
git clone https://github.com/Alakazam-211/K2SO.git
cd K2SO
npm install
cargo tauri dev
```

For a release build:

```bash
cargo tauri build
```

## Architecture

```
┌─────────────────────────────────────────────────────┐
│                   React 19 Frontend                 │
│  Zustand stores  │  xterm.js terminals  │  Viewers  │
├─────────────────────────────────────────────────────┤
│                  Tauri v2 IPC Bridge                │
├─────────────────────────────────────────────────────┤
│                    Rust Backend                     │
│  portable-pty │ rusqlite │ git2 │ llama-cpp-2       │
└─────────────────────────────────────────────────────┘
```

| Layer | Tech | Purpose |
|-------|------|---------|
| Frontend | React 19, TailwindCSS v4, Zustand, xterm.js | UI, state, terminals |
| IPC | Tauri commands + events | Frontend-backend communication |
| Backend | Rust, Tauri v2 | Terminal PTY, database, git, LLM |
| Database | SQLite (rusqlite, WAL mode) | Projects, workspaces, presets, settings |
| AI | llama-cpp-2 (Metal) | Local LLM for workspace assistant |
| Git | git2 | Worktree and branch management |
| Layout | react-mosaic-component | Tiled pane management |

For the full technical architecture, see [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, project structure, and how to add new agent presets, document viewers, and workspace primitives.

## License

[MIT](LICENSE)
