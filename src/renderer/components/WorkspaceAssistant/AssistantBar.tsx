import { useEffect, useRef, useState, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { useAssistantStore } from '../../stores/assistant'
import { useSettingsStore } from '../../stores/settings'
import { useTabsStore } from '../../stores/tabs'
import { useProjectsStore } from '../../stores/projects'

interface ToolCall {
  tool: string
  args: Record<string, unknown>
}

interface ChatResponse {
  raw: string
  parsed: { toolCalls?: ToolCall[]; tool_calls?: ToolCall[]; message?: string }
}

// ── Tool Registry: the complete, finite set of valid tools ──────────

const VALID_TOOLS = new Set([
  'open_terminal',     // Open a terminal in a new tab
  'open_document',     // Open a file as a tab in the active pane
  'add_to_pane',       // Add a terminal or document tab to the current pane
  'arrange_layout',    // Split workspace into multiple panes
  'split_pane',        // Split the active pane vertically (up to 3)
  'resume_chat',       // Resume a past AI conversation
  'switch_workspace',  // Switch to a named workspace
])

interface ChildDescriptor {
  type: 'document' | 'terminal'
  path?: string
  command?: string
  cwd?: string
  items?: ItemDescriptor[]
}

interface ItemDescriptor {
  type: 'document' | 'terminal'
  path?: string
  command?: string
}

/** Validate and sanitize ALL tool calls before execution.
 *  Single source of truth — if a tool isn't handled here, it doesn't execute.
 */
function validateAndSanitize(toolCalls: ToolCall[], cwd: string): ToolCall[] {
  const sanitized: ToolCall[] = []
  let hasArrangeLayout = false

  for (const call of toolCalls) {
    // REJECT unknown tools — only our registered tools pass
    if (!VALID_TOOLS.has(call.tool)) continue

    const args = { ...call.args }

    // ALWAYS strip cwd — we enforce workspace path
    delete args.cwd

    switch (call.tool) {
      case 'open_terminal': {
        if (hasArrangeLayout) continue // Skip if arrange_layout already handles it
        sanitized.push({ tool: call.tool, args })
        break
      }

      case 'open_document': {
        if (!args.path || typeof args.path !== 'string') continue
        sanitized.push({ tool: call.tool, args: { path: args.path } })
        break
      }

      case 'add_to_pane': {
        const type = args.type as string
        if (type !== 'terminal' && type !== 'document') continue
        if (type === 'document' && (!args.path || typeof args.path !== 'string')) continue
        sanitized.push({ tool: call.tool, args: { type, path: args.path, command: args.command } })
        break
      }

      case 'arrange_layout': {
        hasArrangeLayout = true
        // Validate direction
        if (args.direction !== 'horizontal' && args.direction !== 'vertical') {
          args.direction = 'horizontal'
        }
        // Sanitize children — inject cwd, validate types
        if (Array.isArray(args.children)) {
          args.children = (args.children as ChildDescriptor[])
            .filter(child => child.type === 'document' || child.type === 'terminal')
            .map(child => {
              const sanitizedChild: ChildDescriptor = {
                type: child.type,
                cwd: child.type === 'terminal' ? cwd : undefined,
                command: child.type === 'terminal' ? child.command : undefined,
                path: child.type === 'document' ? child.path : undefined,
              }
              // Sanitize items array (multiple tabs per pane)
              if (Array.isArray(child.items)) {
                sanitizedChild.items = (child.items as ItemDescriptor[])
                  .filter(item => item.type === 'document' || item.type === 'terminal')
                  .map(item => ({
                    type: item.type,
                    command: item.type === 'terminal' ? item.command : undefined,
                    path: item.type === 'document' ? item.path : undefined,
                  }))
              }
              return sanitizedChild
            })
        }
        sanitized.push({ tool: call.tool, args })
        break
      }

      case 'resume_chat': {
        if (!args.sessionId || typeof args.sessionId !== 'string') continue
        const provider = (args.provider as string) ?? 'claude'
        sanitized.push({ tool: call.tool, args: { provider, sessionId: args.sessionId } })
        break
      }

      case 'split_pane': {
        sanitized.push({ tool: call.tool, args: {} })
        break
      }

      case 'switch_workspace': {
        if (!args.name || typeof args.name !== 'string') continue
        sanitized.push({ tool: call.tool, args: { name: args.name } })
        break
      }
    }
  }

  return sanitized
}

/** Execute validated tool calls on the tabs store */
function executeToolCalls(toolCalls: ToolCall[]): string {
  const tabsStore = useTabsStore.getState()
  const projectsStore = useProjectsStore.getState()

  const activeProject = projectsStore.projects.find(p => p.id === projectsStore.activeProjectId)
  const cwd = activeProject?.path ?? '~'

  const validCalls = validateAndSanitize(toolCalls, cwd)
  if (validCalls.length === 0) return 'No valid commands found'

  const results: string[] = []

  for (const call of validCalls) {
    try {
      switch (call.tool) {
        case 'arrange_layout': {
          invoke('workspace_arrange', { layout: call.args }).catch((err) => {
            console.error('[AssistantBar] workspace_arrange failed:', err)
          })
          const children = (call.args.children as ChildDescriptor[]) ?? []
          const desc = children.map(c =>
            c.type === 'terminal' ? (c.command ?? 'terminal') : (c.path ?? 'file')
          ).join(' | ')
          results.push(desc)
          break
        }

        case 'open_document': {
          const activeTab = tabsStore.getActiveTab()
          if (activeTab) {
            tabsStore.openFileInPane(activeTab.id, call.args.path as string)
          } else {
            tabsStore.addTab(cwd)
            const tab = tabsStore.tabs[tabsStore.tabs.length - 1]
            if (tab) tabsStore.openFileInPane(tab.id, call.args.path as string)
          }
          results.push((call.args.path as string).split('/').pop() ?? 'file')
          break
        }

        case 'open_terminal': {
          const command = call.args.command as string | undefined
          tabsStore.addTab(cwd, { title: command ?? 'Terminal', command })
          results.push(command ?? 'terminal')
          break
        }

        case 'add_to_pane': {
          const activeTab = tabsStore.getActiveTab()
          if (activeTab) {
            const paneGroupId = tabsStore.getActivePaneGroupId(activeTab.id)
            if (paneGroupId) {
              const itemType = call.args.type as string
              if (itemType === 'terminal') {
                const id = crypto.randomUUID()
                tabsStore.addItemToPaneGroup(activeTab.id, paneGroupId, {
                  id,
                  type: 'terminal',
                  data: { terminalId: id, cwd, command: call.args.command as string | undefined }
                })
                results.push(call.args.command as string ?? 'terminal')
              } else {
                const id = crypto.randomUUID()
                tabsStore.addItemToPaneGroup(activeTab.id, paneGroupId, {
                  id,
                  type: 'file-viewer',
                  data: { filePath: call.args.path as string }
                })
                results.push((call.args.path as string).split('/').pop() ?? 'file')
              }
            }
          }
          break
        }

        case 'resume_chat': {
          const sessionId = call.args.sessionId as string
          const provider = call.args.provider as string
          if (provider === 'claude') {
            tabsStore.addTab(cwd, {
              title: `Claude (resumed)`,
              command: 'claude',
              args: ['--resume', sessionId]
            })
            results.push('Resumed Claude chat')
          }
          break
        }

        case 'split_pane': {
          if (tabsStore.splitCount < 3) {
            tabsStore.splitTerminalArea(cwd)
            results.push('Split into columns')
          } else {
            results.push('Max 3 columns')
          }
          break
        }

        case 'switch_workspace': {
          results.push(`Switch: ${call.args.name}`)
          break
        }
      }
    } catch {
      results.push(`Failed: ${call.tool}`)
    }
  }

  return results.join(', ')
}

export default function AssistantBar(): React.JSX.Element | null {
  const isOpen = useAssistantStore((s) => s.isOpen)
  const isLoading = useAssistantStore((s) => s.isLoading)
  const isDownloading = useAssistantStore((s) => s.isDownloading)
  const downloadProgress = useAssistantStore((s) => s.downloadProgress)
  const modelLoaded = useAssistantStore((s) => s.modelLoaded)
  const lastResult = useAssistantStore((s) => s.lastResult)
  const close = useAssistantStore((s) => s.close)
  const setLoading = useAssistantStore((s) => s.setLoading)
  const setLastResult = useAssistantStore((s) => s.setLastResult)

  const openSettings = useSettingsStore((s) => s.openSettings)

  const [message, setMessage] = useState('')
  const inputRef = useRef<HTMLInputElement>(null)
  const resultTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  // Focus input when bar opens; reset state
  useEffect(() => {
    if (isOpen) {
      setMessage('')
      setLastResult(null)
      requestAnimationFrame(() => {
        inputRef.current?.focus()
      })
    }
    return () => {
      if (resultTimerRef.current) {
        clearTimeout(resultTimerRef.current)
        resultTimerRef.current = null
      }
    }
  }, [isOpen, setLastResult])

  // Auto-clear result after 2 seconds
  useEffect(() => {
    if (lastResult) {
      resultTimerRef.current = setTimeout(() => {
        setLastResult(null)
        close()
      }, 2000)
      return () => {
        if (resultTimerRef.current) {
          clearTimeout(resultTimerRef.current)
          resultTimerRef.current = null
        }
      }
    }
  }, [lastResult, setLastResult, close])

  const handleSubmit = useCallback(async () => {
    const trimmed = message.trim()
    if (!trimmed || isLoading) return

    setLoading(true)
    setMessage('')
    // Don't close — show "working..." indicator

    try {
      const response = await invoke<ChatResponse>('assistant_chat', { message: trimmed })
      console.log('[AssistantBar] Response:', JSON.stringify(response, null, 2))

      // Execute tool calls if present (handle both camelCase and snake_case)
      const toolCalls = response.parsed?.toolCalls ?? response.parsed?.tool_calls
      console.log('[AssistantBar] Parsed:', response.parsed, 'Tool calls:', toolCalls)

      if (toolCalls && toolCalls.length > 0) {
        const summary = executeToolCalls(toolCalls)
        setLastResult(summary)
      } else if (response.parsed?.message) {
        setLastResult(response.parsed.message)
      } else {
        // Fallback: show raw response
        setLastResult(response.raw || 'Done')
      }
    } catch (err) {
      const errorMessage = err instanceof Error ? err.message : String(err)
      setLastResult(`Error: ${errorMessage}`)
    } finally {
      setLoading(false)
    }
  }, [message, isLoading, setLoading, setLastResult])

  const handleKeyDown = useCallback(
    (e: React.KeyboardEvent) => {
      if (e.key === 'Escape') {
        e.preventDefault()
        e.stopPropagation()
        close()
        return
      }
      if (e.key === 'Enter' && !e.shiftKey) {
        e.preventDefault()
        handleSubmit()
      }
    },
    [close, handleSubmit]
  )

  if (!isOpen) return null

  // Downloading state: show progress bar
  if (isDownloading) {
    return (
      <div
        className="fixed bottom-0 left-0 right-0 z-[900] flex items-center"
        style={{
          height: '48px',
          background: '#111111',
          borderTop: '1px solid var(--color-border)',
          fontFamily: 'var(--font-mono, "MesloLGM Nerd Font", monospace)',
          animation: 'assistantSlideUp 150ms ease-out'
        }}
      >
        <div className="flex items-center gap-3 px-4 w-full">
          <span className="text-[11px] text-[var(--color-text-muted)] flex-shrink-0 tracking-wide">
            K2SO
          </span>
          <div className="flex-1 h-[4px] bg-[#1a1a1a] relative overflow-hidden">
            <div
              className="absolute inset-y-0 left-0 bg-[var(--color-accent)] transition-[width] duration-300"
              style={{ width: `${downloadProgress}%` }}
            />
          </div>
          <span className="text-[11px] text-[var(--color-text-muted)] flex-shrink-0 tabular-nums">
            {Math.round(downloadProgress)}%
          </span>
        </div>
      </div>
    )
  }

  // Model not loaded: show configuration message
  if (!modelLoaded) {
    return (
      <div
        className="fixed bottom-0 left-0 right-0 z-[900] flex items-center"
        style={{
          height: '48px',
          background: '#111111',
          borderTop: '1px solid var(--color-border)',
          fontFamily: 'var(--font-mono, "MesloLGM Nerd Font", monospace)',
          animation: 'assistantSlideUp 150ms ease-out'
        }}
        onKeyDown={(e) => {
          if (e.key === 'Escape') {
            e.preventDefault()
            close()
          }
        }}
        tabIndex={-1}
        ref={(el) => el?.focus()}
      >
        <div className="flex items-center gap-3 px-4 w-full">
          <span className="text-[11px] text-[var(--color-text-muted)] flex-shrink-0 tracking-wide">
            K2SO
          </span>
          <span className="text-[11px] text-[var(--color-text-muted)] flex-1">
            AI model not configured
          </span>
          <button
            className="text-[11px] text-[var(--color-accent)] hover:underline bg-transparent border-none cursor-pointer outline-none"
            style={{ fontFamily: 'inherit' }}
            onClick={() => {
              close()
              openSettings('ai-assistant')
            }}
          >
            Settings
          </button>
          <kbd
            className="text-[10px] px-1.5 py-0.5 border border-[var(--color-border)] text-[var(--color-text-muted)] ml-1"
            style={{ background: '#1a1a1a' }}
          >
            ESC
          </kbd>
        </div>
      </div>
    )
  }

  // Result display
  if (lastResult) {
    return (
      <div
        className="fixed bottom-0 left-0 right-0 z-[900] flex items-center"
        style={{
          height: '48px',
          background: '#111111',
          borderTop: '1px solid var(--color-border)',
          fontFamily: 'var(--font-mono, "MesloLGM Nerd Font", monospace)',
          animation: 'assistantSlideUp 150ms ease-out'
        }}
      >
        <div className="flex items-center gap-3 px-4 w-full">
          <span className="text-[11px] text-[var(--color-text-muted)] flex-shrink-0 tracking-wide">
            K2SO
          </span>
          <span className="text-[11px] text-[var(--color-text-primary)] flex-1 truncate">
            {lastResult}
          </span>
        </div>
      </div>
    )
  }

  // Main input state
  return (
    <div
      className="fixed bottom-0 left-0 right-0 z-[900] flex items-center"
      style={{
        height: '48px',
        background: '#111111',
        borderTop: '1px solid var(--color-border)',
        fontFamily: 'var(--font-mono, "MesloLGM Nerd Font", monospace)',
        animation: 'assistantSlideUp 150ms ease-out'
      }}
    >
      <div className="flex items-center gap-3 px-4 w-full">
        <span className="text-[11px] text-[var(--color-text-muted)] flex-shrink-0 tracking-wide">
          K2SO
        </span>

        {isLoading ? (
          <span className="text-[11px] text-[var(--color-text-muted)] flex-1 assistant-pulse">
            thinking...
          </span>
        ) : (
          <input
            ref={inputRef}
            type="text"
            value={message}
            onChange={(e) => setMessage(e.target.value)}
            onKeyDown={handleKeyDown}
            placeholder="Describe your workspace setup..."
            spellCheck={false}
            autoComplete="off"
            className="flex-1 bg-transparent text-[12px] text-[var(--color-text-primary)] placeholder-[var(--color-text-muted)] outline-none border-none"
            style={{
              fontFamily: 'inherit',
              background: '#1a1a1a',
              padding: '6px 10px',
              height: '30px'
            }}
          />
        )}

        {!isLoading && (
          <>
            <button
              onClick={handleSubmit}
              disabled={!message.trim()}
              className="text-[11px] px-2 py-1 border border-[var(--color-border)] bg-transparent cursor-pointer outline-none transition-colors"
              style={{
                color: message.trim()
                  ? 'var(--color-text-primary)'
                  : 'var(--color-text-muted)',
                fontFamily: 'inherit',
                opacity: message.trim() ? 1 : 0.5
              }}
            >
              Run
            </button>
            <kbd
              className="text-[10px] px-1.5 py-0.5 border border-[var(--color-border)] text-[var(--color-text-muted)]"
              style={{ background: '#1a1a1a' }}
            >
              ESC
            </kbd>
          </>
        )}
      </div>

      <style>{`
        @keyframes assistantSlideUp {
          from {
            transform: translateY(100%);
            opacity: 0;
          }
          to {
            transform: translateY(0);
            opacity: 1;
          }
        }
        .assistant-pulse {
          animation: assistantPulse 1.5s ease-in-out infinite;
        }
        @keyframes assistantPulse {
          0%, 100% { opacity: 1; }
          50% { opacity: 0.4; }
        }
      `}</style>
    </div>
  )
}
