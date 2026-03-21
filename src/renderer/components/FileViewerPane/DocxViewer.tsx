import { useState, useEffect } from 'react'
import { invoke } from '@tauri-apps/api/core'
import mammoth from 'mammoth'

interface DocxViewerProps {
  filePath: string
}

export function DocxViewer({ filePath }: DocxViewerProps): React.JSX.Element {
  const [html, setHtml] = useState<string>('')
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false

    async function loadDocx(): Promise<void> {
      setLoading(true)
      setError(null)

      try {
        // Read the file as binary via the existing Tauri command
        const bytes = await invoke<number[]>('fs_read_binary_file', { path: filePath })

        if (cancelled) return

        // Convert the number array to a Uint8Array, then to an ArrayBuffer
        const uint8 = new Uint8Array(bytes)
        const arrayBuffer = uint8.buffer

        // Convert DOCX to HTML using mammoth
        const result = await mammoth.convertToHtml({ arrayBuffer })

        if (cancelled) return

        setHtml(result.value)

        // Log any warnings from mammoth for debugging
        if (result.messages.length > 0) {
          console.warn('[docx-viewer] Mammoth warnings:', result.messages)
        }
      } catch (err) {
        if (cancelled) return
        const message = err instanceof Error ? err.message : String(err)
        setError(message)
      } finally {
        if (!cancelled) {
          setLoading(false)
        }
      }
    }

    loadDocx()

    return () => {
      cancelled = true
    }
  }, [filePath])

  if (loading) {
    return (
      <div className="flex h-full w-full items-center justify-center bg-[#0a0a0a] text-[var(--color-text-muted)] text-sm">
        Converting document...
      </div>
    )
  }

  if (error) {
    return (
      <div className="flex h-full w-full flex-col items-center justify-center gap-3 bg-[#0a0a0a]">
        <span className="text-red-400 text-sm">Failed to load document</span>
        <span className="text-xs text-[var(--color-text-muted)] max-w-md text-center px-4">
          {error}
        </span>
      </div>
    )
  }

  return (
    <div
      className="docx-body p-6 overflow-y-auto h-full bg-[#0a0a0a]"
      dangerouslySetInnerHTML={{ __html: html }}
    />
  )
}
