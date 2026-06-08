// Plan B (Bulk-2) — host-aware client for the workspace primary-agent
// display-name + resume-chat-args routes. These were the 3 daemon-proxy
// commands in `commands/k2so_agents.rs` (each `cli_get`-ed the daemon then
// fell back to an in-process k2so-core call if the daemon was unreachable).
//
// Moving them onto `daemonCli*` makes them work against ANY daemon (local
// OR a remote K2 Connect host) instead of the localhost-pinned Tauri proxy.
// The in-process fallback is intentionally dropped — the daemon is the
// single source of truth (daemon-first); a renderer talking to a remote
// daemon has no local k2so-core to fall back to anyway.
//
// Routes (confirmed against `crates/k2so-daemon/src/cli.rs`):
//   GET /cli/workspace/agent-display-name?project=<path>      → {display_name}
//   GET /cli/workspace/set-agent-display-name?project=&name=  → mutation (echo)
//   GET /cli/workspace/resume-chat-args?project=<path>        → ResumeChatArgs
//
// NB: all three are GET routes in the daemon (set-agent-display-name is a
// GET that mutates, matching the old `cli_get` proxy). The display-name body
// field is snake_case `display_name`; resume-chat-args is camelCase.

import { daemonCliGet } from '@/lib/daemon-cli'

/** Resolve the workspace's primary-agent display name. Total — the daemon
 *  always returns a string (display_name → name → project name fallback).
 *  Returns '' if the daemon read fails so callers degrade gracefully. */
export async function agentDisplayName(projectPath: string): Promise<string> {
  const r = await daemonCliGet<{ display_name?: string }>(
    'workspace/agent-display-name',
    { project: projectPath },
  )
  return r?.display_name ?? ''
}

/** Set the workspace's primary-agent display name. The daemon rewrites
 *  AGENT.md frontmatter, invalidates its cache, emits SyncProjects, and
 *  pushes the new label to any live canonical session. (GET-with-mutation,
 *  mirroring the old `cli_get` proxy — body is ignored.) */
export async function setAgentDisplayName(
  projectPath: string,
  name: string,
): Promise<void> {
  await daemonCliGet('workspace/set-agent-display-name', {
    project: projectPath,
    name,
  })
}

/** #657 — persist the pinned chat tab's canonical Claude session id to
 *  the daemon DB (`workspace_sessions.session_id`). Called on the
 *  dismiss-reap path BEFORE the chat PTY is closed so `claude --resume`
 *  has a target when the workspace is re-opened. (GET-with-mutation,
 *  mirroring `setAgentDisplayName` — the daemon route reads query
 *  params.) Resolves to the daemon's update result. */
export async function setChatSession(
  projectPath: string,
  sessionId: string,
): Promise<void> {
  await daemonCliGet('workspace/set-chat-session', {
    project: projectPath,
    session_id: sessionId,
  })
}

export interface ResumeChatArgs {
  command: string
  args: string[]
  cwd: string
  resumeSession?: string
  /** `true` when the daemon resolved an EXISTING `workspace_sessions.session_id`
   *  whose JSONL is on disk (resumable). `false` when it pre-allocated a fresh
   *  UUID because no usable saved session existed. Cold-boot revive uses this
   *  to decide whether the daemon's canonical session should override the
   *  renderer's layout hint (GH#679). */
  resumedExisting?: boolean
}

/** Resolve the `claude --resume <session>` (or fresh `claude`) launch args
 *  for the workspace's pinned chat tab. camelCase response. */
export async function resumeChatArgs(projectPath: string): Promise<ResumeChatArgs> {
  return daemonCliGet<ResumeChatArgs>('workspace/resume-chat-args', {
    project: projectPath,
  })
}

/** GH#679 — cold-boot revive reconciliation. Given the layout-restored
 *  sessionId hint and the daemon's canonical `resume-chat-args` response
 *  (which reads `workspace_sessions.session_id` from SQLite), decide which
 *  session the pinned chat should actually resume.
 *
 *  SQLite is the source of truth: when the daemon resolved an EXISTING
 *  resumable session (`resumedExisting`) that DIFFERS from the layout hint,
 *  the daemon's session wins. Otherwise (no canonical response, a freshly
 *  pre-allocated UUID, or an identical session) the layout hint is kept —
 *  preserving the renderer-canonical offline / DB-race resilience the
 *  canonical-lane-restore PRD was built for.
 *
 *  Pure + side-effect-free so the decision is unit-testable without
 *  rendering AgentChatPane. */
export function reconcileColdBootSession(
  layoutHint: string,
  canonical: ResumeChatArgs | null,
): string {
  if (
    canonical?.resumedExisting &&
    canonical.resumeSession &&
    canonical.resumeSession !== layoutHint
  ) {
    return canonical.resumeSession
  }
  return layoutHint
}
