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
  isPodLeader: boolean
  agentMd: string
  agentMdPath: string
  agentDir: string
}

interface AgentPersonaEditorProps {
  agentName: string
  projectPath: string
  onClose: () => void
}

// ── Component ──────────────────────────────────────────────────────────

export function AgentPersonaEditor({ agentName, projectPath, onClose }: AgentPersonaEditorProps): React.JSX.Element {
  const [agentMdPath, setAgentMdPath] = useState<string | null>(null)
  const [watchDir, setWatchDir] = useState<string | null>(null)
  const [agentContent, setAgentContent] = useState<string>('')
  const [context, setContext] = useState<EditorContext | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [previewMode, setPreviewMode] = useState<'preview' | 'edit'>('preview')
  const [previewScale, setPreviewScale] = useState(100)
  // 100% display maps to 70% CSS font-size as the baseline
  const cssScale = Math.round(previewScale * 0.7)

  // Resolve the user's default AI agent command
  const defaultAgent = useSettingsStore((s) => s.defaultAgent)
  const presets = usePresetsStore((s) => s.presets)
  const agentCommand = useMemo(() => {
    const preset = presets.find((p) => p.id === defaultAgent) || presets.find((p) => p.enabled)
    if (!preset) return null
    return parseCommand(preset.command)
  }, [defaultAgent, presets])

  // Initialize: fetch agent context
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
      } catch (err) {
        const msg = err instanceof Error ? err.message : String(err)
        console.error('[agent-editor] Init failed:', msg)
        setError(msg)
      }
    }
    init()
  }, [projectPath, agentName])

  const handleFileChange = useCallback((content: string) => {
    setAgentContent(content)
  }, [])

  // Manual refresh: re-read agent.md directly
  const handleManualRefresh = useCallback(async () => {
    if (!agentMdPath) return
    try {
      const result = await invoke<{ content: string }>('fs_read_file', { path: agentMdPath })
      handleFileChange(result.content)
    } catch (err) {
      console.error('[agent-editor] Manual refresh failed:', err)
    }
  }, [agentMdPath, handleFileChange])

  // On close: backup agent.md, then close
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

  // Build the system prompt for the AI assistant
  const agentPrompt = useMemo(() => {
    if (!context) return ''
    const isCustom = context.agentType === 'custom'
    return [
      `You're helping the user configure an AI agent. Here's the context:`,
      ``,
      `Agent: "${context.agentName}"`,
      `Role: ${context.role}`,
      `Type: ${isCustom ? 'Custom Agent' : context.isPodLeader ? 'Pod Leader' : 'Pod Member'}`,
      ``,
      `Edit the file agent.md in the current directory. This single file defines everything about the agent:`,
      ``,
      `• Frontmatter (between --- delimiters): name, role, type — these are read by the system`,
      `• Body (below frontmatter): all instructions, behavior, personality, tools, integrations`,
      ``,
      `Current contents:`,
      context.agentMd,
      ``,
      isCustom
        ? [
            `This is a Custom Agent — it runs purely from agent.md with no K2SO infrastructure injected.`,
            `The body of agent.md IS the agent's entire system prompt when it wakes up on the heartbeat.`,
            `Focus on: what software it operates, what it does on each wake, tools/APIs it uses, constraints.`,
          ].join('\n')
        : [
            `This agent runs within K2SO. The following docs are auto-injected (don't duplicate them):`,
            `• K2SO CLI tools reference`,
            `• Workflow docs (lead agent vs sub-agent patterns)`,
            `• Work queue structure (inbox/active/done folders)`,
            `• Other agents list (for delegation awareness)`,
            ``,
            `Focus agent.md body on what makes this agent unique beyond the standard K2SO setup.`,
          ].join('\n'),
      ``,
      `The user sees a live preview on the right. Only edit agent.md.`,
    ].join('\n')
  }, [context])

  const terminalCommand = agentCommand?.command
  const terminalArgs = useMemo(() => {
    if (!agentCommand || !context) return undefined
    const baseArgs = [...agentCommand.args]
    const isClaude = agentCommand.command === 'claude'
    if (isClaude) {
      return [
        ...baseArgs,
        '--append-system-prompt', agentPrompt,
        `Open and read agent.md in the current directory. This defines the agent "${context.agentName}" (${context.role}). The user sees a live preview on the right. Start by asking what they want this agent to do.`,
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

  return (
    <AIFileEditor
      filePath={agentMdPath}
      watchDir={watchDir}
      cwd={watchDir}
      command={terminalCommand}
      args={terminalArgs}
      title={`Agent: ${agentName}`}
      instructions={`Editing agent.md for "${agentName}". This file defines the agent's identity, role, and all instructions.`}
      warningText="This agent has full system access when running."
      onFileChange={handleFileChange}
      onClose={handleClose}
      onManualRefresh={handleManualRefresh}
      preview={
        <div className="h-full flex flex-col">
          {/* Header: agent info + Preview/Edit toggle */}
          <div className="flex items-center justify-between px-4 py-2 border-b border-[var(--color-border)] flex-shrink-0">
            <div className="text-xs text-[var(--color-text-muted)]">
              <span className="font-medium text-[var(--color-text-primary)]">{context.agentName}</span>
              <span className="mx-2">&middot;</span>
              <span>{context.role}</span>
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
          {/* Content */}
          {previewMode === 'preview' ? (
            <div className="flex-1 overflow-auto p-4">
              <div className="markdown-content" style={{ fontSize: `${cssScale}%` }}>
                <ReactMarkdown remarkPlugins={[remarkGfm]}>
                  {stripFrontmatter(agentContent) || '*No content yet*'}
                </ReactMarkdown>
              </div>
            </div>
          ) : (
            <div className="flex-1 overflow-hidden">
              <CodeEditor
                code={agentContent}
                filePath={agentMdPath}
                onSave={async (content) => {
                  try {
                    await invoke('fs_write_file', { path: agentMdPath, content })
                  } catch (err) {
                    console.error('[agent-editor] Save failed:', err)
                  }
                }}
                onChange={(content) => setAgentContent(content)}
              />
            </div>
          )}
        </div>
      }
    />
  )
}
