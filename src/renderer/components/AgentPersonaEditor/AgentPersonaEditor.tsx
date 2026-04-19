import { useState, useCallback, useEffect, useMemo } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { AIFileEditor } from '../AIFileEditor/AIFileEditor'
import { useSettingsStore } from '@/stores/settings'
import { usePresetsStore, parseCommand } from '@/stores/presets'
import ReactMarkdown from 'react-markdown'
import remarkGfm from 'remark-gfm'
import { CodeEditor } from '../FileViewerPane/CodeEditor'

// ── Helpers ─────────────────────────────────────────────────────────────

/** Strip YAML frontmatter (--- delimited) for clean markdown preview */
function stripFrontmatter(content: string): string {
  if (content.startsWith('---')) {
    const end = content.indexOf('---', 3)
    if (end !== -1) return content.slice(end + 3).trim()
  }
  return content.trim()
}

// ── Types ──────────────────────────────────────────────────────────────

interface EditorContext {
  agentName: string
  role: string
  agentType: string
  isCoordinator: boolean
  agentMd: string
  agentMdPath: string
  agentDir: string
}

/**
 * Shape returned by the `k2so_agents_preview_agent_context` Tauri
 * command. `contextPath` is the new canonical field; `claudeMdPath`
 * is emitted alongside for back-compat during the 0.33.0 rename
 * window — both point at the same `<agent>/CLAUDE.md` file.
 */
interface AgentContextPreview {
  generated: string
  onDisk: string | null
  /** CLAUDE.md path — canonical field going forward. */
  contextPath?: string
  /** @deprecated use `contextPath` — kept for 0.33.0 back-compat. */
  claudeMdPath: string
}

type PreviewTab = 'profile' | 'wakeup' | 'claude-md' | 'workspace-claude-md'

interface AgentPersonaEditorProps {
  agentName: string
  projectPath: string
  onClose: () => void
}

// ── Component ──────────────────────────────────────────────────────────

export function AgentPersonaEditor({ agentName, projectPath, onClose }: AgentPersonaEditorProps): React.JSX.Element {
  const [agentMdPath, setAgentMdPath] = useState<string | null>(null)
  const [wakeupPath, setWakeupPath] = useState<string | null>(null)
  const [wakeupContent, setWakeupContent] = useState<string>('')
  const [watchDir, setWatchDir] = useState<string | null>(null)
  const [agentContent, setAgentContent] = useState<string>('')
  const [context, setContext] = useState<EditorContext | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [previewMode, setPreviewMode] = useState<'preview' | 'edit'>('preview')
  const [previewScale, setPreviewScale] = useState(100)
  const cssScale = Math.round(previewScale * 0.7)

  // Tabbed preview state
  const [activeTab, setActiveTab] = useState<PreviewTab>('profile')
  const [claudeMdContent, setClaudeMdContent] = useState<string>('')
  const [claudeMdPath, setClaudeMdPath] = useState<string>('')
  const [claudeMdGenerated, setClaudeMdGenerated] = useState<string>('')
  const [wsClaudeMdContent, setWsClaudeMdContent] = useState<string>('')
  const wsClaudeMdPath = `${projectPath}/CLAUDE.md`
  const [regenerating, setRegenerating] = useState(false)

  // Resolve the user's default AI agent command
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const presets = usePresetsStore((s) => s.presets)
  const agentCommand = useMemo(() => {
    const preset = presets.find((p) => p.id === defaultAgent) || presets.find((p) => p.enabled)
    if (!preset) return null
    return parseCommand(preset.command)
  }, [defaultAgent, presets])

  // Initialize: fetch agent context + CLAUDE.md preview
  useEffect(() => {
    const init = async () => {
      try {
        const ctx = await invoke<EditorContext>('k2so_agents_get_editor_context', {
          projectPath,
          agentName,
        })
        setContext(ctx)
        setAgentContent(ctx.agentMd)
        setAgentMdPath(ctx.agentMdPath)
        setWatchDir(ctx.agentDir)

        // Load WAKEUP.md (created lazily by the backend on first app
        // launch after this feature shipped — may still be missing for
        // agent-template type agents, which don't use wake-up).
        const wkPath = `${ctx.agentDir}/WAKEUP.md`
        setWakeupPath(wkPath)
        try {
          const wk = await invoke<{ content: string }>('fs_read_file', { path: wkPath })
          setWakeupContent(wk.content)
        } catch {
          setWakeupContent('')
        }

        // Fetch agent context preview (SKILL.md body + co-written
        // CLAUDE.md harness fallback).
        try {
          const preview = await invoke<AgentContextPreview>(
            'k2so_agents_preview_agent_context',
            { projectPath, agentName }
          )
          setClaudeMdGenerated(preview.generated)
          setClaudeMdContent(preview.onDisk ?? preview.generated)
          setClaudeMdPath(preview.contextPath ?? preview.claudeMdPath)
        } catch {
          // Context preview not available — non-fatal
        }

        // Load workspace root CLAUDE.md
        try {
          const result = await invoke<{ content: string }>('fs_read_file', { path: `${projectPath}/CLAUDE.md` })
          setWsClaudeMdContent(result.content)
        } catch {
          setWsClaudeMdContent('')
        }
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err)
        console.error('[agent-editor] Init failed:', msg)
        setError(msg)
      }
    }
    init()
  }, [projectPath, agentName])

  const handleFileChange = useCallback((content: string, path?: string) => {
    // In multi-file mode the watcher tells us which path changed; route
    // to the right state slot directly. Fall back to the active preview
    // tab when path is unknown (single-file mode preserved).
    if (path && wakeupPath && path === wakeupPath) {
      setWakeupContent(content)
      return
    }
    if (path && agentMdPath && path === agentMdPath) {
      setAgentContent(content)
      return
    }
    if (activeTab === 'profile' || activeTab === 'wakeup') {
      // 'wakeup' routes above via path; treat unpathed content as AGENT.md
      setAgentContent(content)
    } else if (activeTab === 'claude-md') {
      setClaudeMdContent(content)
    } else {
      setWsClaudeMdContent(content)
    }
  }, [activeTab, agentMdPath, wakeupPath])

  // Manual refresh: re-read AGENT.md directly
  const handleManualRefresh = useCallback(async () => {
    if (!agentMdPath) return
    try {
      const result = await invoke<{ content: string }>('fs_read_file', { path: agentMdPath })
      handleFileChange(result.content)
    } catch (err) {
      console.error('[agent-editor] Manual refresh failed:', err)
    }
  }, [agentMdPath, handleFileChange])

  // On close: backup AGENT.md, then close
  const handleClose = useCallback(async () => {
    try {
      if (agentMdPath) {
        const result = await invoke<{ content: string }>('fs_read_file', { path: agentMdPath })
        await invoke('k2so_agents_save_agent_md', {
          projectPath,
          agentName,
          content: result.content,
        })
      }
    } catch (err) {
      console.error('[agent-editor] Failed to save on close:', err)
    }
    onClose()
  }, [projectPath, agentName, agentMdPath, onClose])

  // Regenerate agent context (SKILL.md + co-written CLAUDE.md) to defaults
  const handleRegenerate = useCallback(async () => {
    setRegenerating(true)
    try {
      const content = await invoke<string>('k2so_agents_regenerate_agent_context', {
        projectPath,
        agentName,
      })
      setClaudeMdContent(content)
      setClaudeMdGenerated(content)
    } catch (err) {
      console.error('[agent-editor] Regenerate failed:', err)
    } finally {
      setRegenerating(false)
    }
  }, [projectPath, agentName])

  // Save CLAUDE.md edits
  const handleSaveClaudeMd = useCallback(async (content: string) => {
    if (!claudeMdPath) return
    try {
      await invoke('fs_write_file', { path: claudeMdPath, content })
      setClaudeMdContent(content)
    } catch (err) {
      console.error('[agent-editor] CLAUDE.md save failed:', err)
    }
  }, [claudeMdPath])

  // Build the system prompt for the AI assistant
  const agentPrompt = useMemo(() => {
    if (!context) return ''
    const isCustom = context.agentType === 'custom'
    const isK2SO = context.agentType === 'k2so'
    const isCoordMode = context.agentType === 'manager' || context.agentType === 'coordinator' || context.agentType === 'agent-template'
      || context.agentType === 'pod-leader' || context.agentType === 'pod-member'

    const typeLabel = isK2SO ? 'K2SO Agent' : isCustom ? 'Custom Agent' : context.isCoordinator ? 'Workspace Manager' : 'Agent Template'

    const typeGuidance = isK2SO
      ? [
          `This is the K2SO Agent — the top-level planner and orchestrator for this workspace.`,
          ``,
          `IMPORTANT: The default K2SO agent knowledge (CLI tools, workflow docs, work queue structure)`,
          `is auto-injected at launch. Editing those defaults is at the user's own risk.`,
          ``,
          `Your job is to help the user ADD project-specific context, NOT replace the defaults.`,
          `Focus on helping them define:`,
          ``,
          `• Work Sources — Where does new work come from? Examples:`,
          `  - GitHub Issues: \`gh issue list --repo OWNER/REPO --label bug --state open\``,
          `  - Linear: \`linear issue list --team TEAM --status "In Progress"\``,
          `  - Jira: \`jira issue list --project KEY --status "To Do"\``,
          `  - Custom API: \`curl -s https://api.example.com/tasks | jq '.items[]'\``,
          `  - Local directory: check \`/path/to/intake/\` for new .md files`,
          ``,
          `• Project Context — What does this codebase do? What are the key directories?`,
          `  What conventions should the agent follow?`,
          ``,
          `• Integration Commands — CLI tools the agent should use to check for work,`,
          `  report status, or interact with external systems (NO MCP servers — CLI only).`,
          ``,
          `• Constraints — Hours of operation, cost limits, repos that are off-limits,`,
          `  branches that should never be modified directly.`,
          ``,
          `Ask the user: "Where does new work come from for this project?" and help them`,
          `configure the Work Sources section with the right CLI commands.`,
        ].join('\n')
      : isCustom
        ? [
            `This is a Custom Agent — it runs purely from AGENT.md with no K2SO infrastructure injected.`,
            `The body of AGENT.md IS the agent's entire system prompt when it wakes up on the heartbeat.`,
            `Focus on: what software it operates, what it does on each wake, tools/APIs it uses, constraints.`,
          ].join('\n')
        : [
            `This agent runs within K2SO. The following docs are auto-injected (don't duplicate them):`,
            `• K2SO CLI tools reference`,
            `• Workflow docs (lead agent vs sub-agent patterns)`,
            `• Work queue structure (inbox/active/done folders)`,
            `• Other agents list (for delegation awareness)`,
            ``,
            `Focus AGENT.md body on what makes this agent unique beyond the standard K2SO setup.`,
          ].join('\n')

    const projectMdNote = isCoordMode
      ? [
          ``,
          `• **PROJECT.md** — There is a shared project context file at \`.k2so/PROJECT.md\` that gets`,
          `  injected into every agent's context at launch. If the user mentions project-wide info`,
          `  (tech stack, conventions, key directories), suggest putting it in PROJECT.md instead of`,
          `  duplicating it in each agent's file. You can read it: \`cat .k2so/PROJECT.md\``,
        ].join('\n')
      : ''

    return [
      `You're helping the user configure an AI agent. Here's the context:`,
      ``,
      `Agent: "${context.agentName}"`,
      `Role: ${context.role}`,
      `Type: ${typeLabel}`,
      ``,
      `Edit the file AGENT.md in the current directory. This single file defines everything about the agent:`,
      ``,
      `• Frontmatter (between --- delimiters): name, role, type — these are read by the system`,
      `• Body (below frontmatter): all instructions, behavior, personality, tools, integrations`,
      ``,
      `## Four Files You'll Be Editing`,
      ``,
      `There are FOUR files that define this agent's behavior. You have direct read/write access to all of them:`,
      ``,
      `1. **AGENT.md** (in current directory) — the agent's core identity, role, standing orders, and personality.`,
      `   Path: \`${projectPath}/.k2so/agents/${context.agentName}/AGENT.md\``,
      ``,
      `2. **WAKEUP.md** (in current directory) — operational wake-up instructions the heartbeat scheduler reads every time this agent wakes.`,
      `   Path: \`${projectPath}/.k2so/agents/${context.agentName}/WAKEUP.md\``,
      `   NOT the persona. Small, tactical, edited often. Keep it focused on the wake-up procedure (checkin, triage, work through inbox, exit).`,
      `   When the user asks to change "what the agent does on wake," edit WAKEUP.md — not AGENT.md.`,
      ``,
      `3. **Agent CLAUDE.md** — read by the agent during heartbeat/automated launches.`,
      `   Path: \`${projectPath}/.k2so/agents/${context.agentName}/CLAUDE.md\``,
      `   Should contain the agent identity + K2SO CLI tools + work queue info.`,
      ``,
      `4. **Workspace CLAUDE.md** — read by the user's manual Claude sessions launched from the workspace.`,
      `   Path: \`${projectPath}/CLAUDE.md\``,
      `   Should contain project context + CLI tools so manual sessions understand the workspace.`,
      ``,
      `When the user asks to update the agent's behavior, choose the right file(s):`,
      `• "What does the agent do on wake?" → WAKEUP.md`,
      `• "Change the agent's personality/role/tools/orders" → AGENT.md AND the relevant CLAUDE.md file(s)`,
      `Don't just copy-paste between files — tailor each for its audience.`,
      `You can read and write all four files directly using their full paths above.`,
      ``,
      `## Key Sections to Help Configure:`,
      ``,
      `• **Standing Orders** — Persistent directives the agent follows every time it wakes up.`,
      `  Unlike inbox work items (one-off tasks), standing orders are ongoing responsibilities.`,
      `  Help the user define what this agent should ALWAYS check or do on each wake cycle.`,
      `  Examples: "Check CI status", "Review open PRs older than 24h", "Scan for new issues".`,
      projectMdNote,
      ``,
      `Current contents:`,
      context.agentMd,
      ``,
      typeGuidance,
      ``,
      `The user sees a tabbed preview on the right with Profile (AGENT.md), Wake-up (WAKEUP.md),`,
      `Agent CLAUDE.md, and Workspace CLAUDE.md tabs. Before editing, confirm which file they want`,
      `updated and use the path from the list above.`,
    ].join('\n')
  }, [context, projectPath])

  const terminalCommand = agentCommand?.command
  const terminalArgs = useMemo(() => {
    if (!agentCommand || !context) return undefined
    const baseArgs = [...agentCommand.args]
    const isClaude = agentCommand.command === 'claude'
    if (isClaude) {
      return [
        ...baseArgs,
        '--append-system-prompt', agentPrompt,
        `Open and read all four files: AGENT.md (current dir), WAKEUP.md (current dir), CLAUDE.md (current dir), and ${projectPath}/CLAUDE.md (workspace root). This defines the agent "${context.agentName}" (${context.role}). The user sees all four files in the preview tabs. Start by asking what they want this agent to do.`,
      ]
    }
    return baseArgs
  }, [agentCommand, agentPrompt, context])

  // ── Conditional returns (after all hooks) ────────────────────────────

  if (error) {
    return (
      <div className="flex flex-col items-center justify-center h-64 gap-3">
        <p className="text-xs text-red-400">Failed to initialize agent editor: {error}</p>
        <button onClick={onClose} className="text-xs text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] cursor-pointer no-drag">&larr; Back</button>
      </div>
    )
  }

  if (!agentMdPath || !watchDir || !context) {
    return (
      <div className="flex items-center justify-center h-64 text-xs text-[var(--color-text-muted)]">
        Setting up agent editor...
      </div>
    )
  }

  // The AIFileEditor's outer tab strip is hidden (showTabs={false})
  // because the preview panel on the right already shows tabs for
  // Profile / Wake-up / Agent CLAUDE.md / Workspace CLAUDE.md. Two
  // layers of tabs was confusing. We still pass `files` so the
  // watcher tracks every file — otherwise AI edits to WAKEUP.md (or
  // any non-active file) wouldn't reach the preview panel.
  const editorFiles = wakeupPath
    ? [
        { path: agentMdPath, label: 'Persona' },
        { path: wakeupPath, label: 'Wake-up' },
      ]
    : undefined
  return (
    <AIFileEditor
      filePath={agentMdPath}
      files={editorFiles}
      showTabs={false}
      watchDir={watchDir}
      cwd={watchDir}
      command={terminalCommand}
      args={terminalArgs}
      title={`Agent: ${agentName}`}
      instructions={undefined}
      warningText="This agent has full system access when running."
      onFileChange={handleFileChange}
      onClose={handleClose}
      onManualRefresh={handleManualRefresh}
      preview={
        <div className="h-full flex flex-col">
          {/* Tab bar */}
          <div className="flex items-center justify-between px-3 py-1.5 border-b border-[var(--color-border)] flex-shrink-0">
            <div className="flex items-center gap-1">
              {((wakeupPath
                ? ['profile', 'wakeup', 'claude-md', 'workspace-claude-md']
                : ['profile', 'claude-md', 'workspace-claude-md']
              ) as PreviewTab[]).map((tab) => (
                <button
                  key={tab}
                  onClick={() => { setActiveTab(tab); setPreviewMode('preview') }}
                  className={`px-2.5 py-1 text-[10px] font-medium transition-colors no-drag cursor-pointer ${
                    activeTab === tab
                      ? 'text-[var(--color-text-primary)] border-b-2 border-[var(--color-accent)]'
                      : 'text-[var(--color-text-muted)] hover:text-[var(--color-text-secondary)]'
                  }`}
                >
                  {tab === 'profile' ? 'Profile'
                    : tab === 'wakeup' ? 'Wake-up'
                    : tab === 'claude-md' ? 'Agent CLAUDE.md'
                    : 'Workspace CLAUDE.md'}
                </button>
              ))}
            </div>
            <div className="flex items-center gap-2 flex-shrink-0">
              {/* Zoom controls — only in preview mode */}
              {previewMode === 'preview' && (
                <div className="flex items-center gap-0.5">
                  <button
                    onClick={() => setPreviewScale((s) => Math.max(50, s - 10))}
                    className="w-5 h-5 flex items-center justify-center text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
                    title="Zoom out"
                  >
                    −
                  </button>
                  <span className="text-[9px] tabular-nums text-[var(--color-text-muted)] w-7 text-center">{previewScale}%</span>
                  <button
                    onClick={() => setPreviewScale((s) => Math.min(200, s + 10))}
                    className="w-5 h-5 flex items-center justify-center text-[11px] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer"
                    title="Zoom in"
                  >
                    +
                  </button>
                </div>
              )}
              {/* Regenerate button — CLAUDE.md tab only */}
              {activeTab === 'claude-md' && (
                <button
                  onClick={handleRegenerate}
                  disabled={regenerating}
                  className="px-2 py-1 text-[10px] font-medium text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] bg-[var(--color-bg-elevated)] border border-[var(--color-border)] no-drag cursor-pointer disabled:opacity-50"
                  title="Regenerate CLAUDE.md from defaults"
                >
                  {regenerating ? 'Regenerating...' : 'Regenerate'}
                </button>
              )}
              {/* Preview/Edit toggle */}
              <div className="flex gap-0.5">
                {(['preview', 'edit'] as const).map((mode) => (
                  <button
                    key={mode}
                    onClick={() => setPreviewMode(mode)}
                    className={`px-2 py-1 text-[10px] font-medium transition-colors no-drag cursor-pointer ${
                      previewMode === mode
                        ? 'bg-[var(--color-accent)] text-white'
                        : 'bg-[var(--color-bg-elevated)] text-[var(--color-text-muted)] hover:text-[var(--color-text-primary)] border border-[var(--color-border)]'
                    }`}
                  >
                    {mode === 'preview' ? 'Preview' : 'Edit'}
                  </button>
                ))}
              </div>
            </div>
          </div>

          {/* CLAUDE.md info banner */}
          {activeTab === 'claude-md' && (
            <div className="px-3 py-2 bg-yellow-500/5 border-b border-yellow-500/20 flex-shrink-0">
              <p className="text-[10px] text-yellow-500/80 leading-relaxed">
                Used by heartbeat/automated launches. Auto-generated from Profile + CLI tools.
                Edits persist but may be overwritten on Regenerate or mode changes.
              </p>
            </div>
          )}
          {activeTab === 'workspace-claude-md' && (
            <div className="px-3 py-2 bg-blue-500/5 border-b border-blue-500/20 flex-shrink-0">
              <p className="text-[10px] text-blue-400/80 leading-relaxed">
                Used by manual Claude sessions launched from the workspace root.
                Customize this for the user's interactive experience.
              </p>
            </div>
          )}

          {/* Content */}
          {previewMode === 'preview' ? (
            <div className="flex-1 overflow-auto p-4">
              <div className="markdown-content" style={{ fontSize: `${cssScale}%` }}>
                <ReactMarkdown remarkPlugins={[remarkGfm]}>
                  {activeTab === 'profile'
                    ? (stripFrontmatter(agentContent) || '*No content yet*')
                    : activeTab === 'wakeup'
                      ? (wakeupContent || '*No WAKEUP.md yet — will be created from the template on first heartbeat.*')
                      : activeTab === 'claude-md'
                        ? (claudeMdContent || '*CLAUDE.md not yet generated. Click Regenerate to create it.*')
                        : (wsClaudeMdContent || '*No workspace CLAUDE.md yet.*')
                  }
                </ReactMarkdown>
              </div>
            </div>
          ) : (
            <div className="flex-1 overflow-hidden">
              <CodeEditor
                code={
                  activeTab === 'profile' ? agentContent
                    : activeTab === 'wakeup' ? wakeupContent
                    : activeTab === 'claude-md' ? claudeMdContent
                    : wsClaudeMdContent
                }
                filePath={
                  activeTab === 'profile' ? (agentMdPath ?? '')
                    : activeTab === 'wakeup' ? (wakeupPath ?? '')
                    : activeTab === 'claude-md' ? claudeMdPath
                    : wsClaudeMdPath
                }
                onSave={async (content) => {
                  if (activeTab === 'profile') {
                    try { await invoke('fs_write_file', { path: agentMdPath, content }) } catch (err) { console.error('[agent-editor] Save failed:', err) }
                  } else if (activeTab === 'wakeup' && wakeupPath) {
                    try { await invoke('fs_write_file', { path: wakeupPath, content }) } catch (err) { console.error('[agent-editor] Wake-up save failed:', err) }
                  } else if (activeTab === 'claude-md') {
                    await handleSaveClaudeMd(content)
                  } else {
                    try { await invoke('fs_write_file', { path: wsClaudeMdPath, content }) } catch (err) { console.error('[agent-editor] Workspace CLAUDE.md save failed:', err) }
                  }
                }}
                onChange={(content) => {
                  if (activeTab === 'profile') {
                    setAgentContent(content)
                  } else if (activeTab === 'wakeup') {
                    setWakeupContent(content)
                  } else if (activeTab === 'claude-md') {
                    setClaudeMdContent(content)
                  } else {
                    setWsClaudeMdContent(content)
                  }
                }}
              />
            </div>
          )}
        </div>
      }
    />
  )
}
