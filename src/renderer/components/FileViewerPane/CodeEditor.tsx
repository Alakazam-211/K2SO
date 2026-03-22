import { useEffect, useRef, useCallback } from 'react'
import { EditorView, keymap, lineNumbers, highlightActiveLine, highlightActiveLineGutter, drawSelection, rectangularSelection } from '@codemirror/view'
import { EditorState, type Extension } from '@codemirror/state'
import { defaultKeymap, history, historyKeymap, indentWithTab } from '@codemirror/commands'
import { oneDarkHighlightStyle } from '@codemirror/theme-one-dark'
import { syntaxHighlighting, defaultHighlightStyle, bracketMatching, indentOnInput } from '@codemirror/language'
import { searchKeymap, highlightSelectionMatches } from '@codemirror/search'

// Language imports
import { javascript } from '@codemirror/lang-javascript'
import { python } from '@codemirror/lang-python'
import { rust } from '@codemirror/lang-rust'
import { css } from '@codemirror/lang-css'
import { html } from '@codemirror/lang-html'
import { json } from '@codemirror/lang-json'
import { markdown } from '@codemirror/lang-markdown'
import { cpp } from '@codemirror/lang-cpp'
import { java } from '@codemirror/lang-java'
import { go } from '@codemirror/lang-go'
import { sql } from '@codemirror/lang-sql'
import { xml } from '@codemirror/lang-xml'
import { yaml } from '@codemirror/lang-yaml'
import { php } from '@codemirror/lang-php'

// ── Language detection ──────────────────────────────────────────────

type LanguageFn = () => Extension

const EXT_LANG_MAP: Record<string, LanguageFn> = {
  // JavaScript / TypeScript
  js: () => javascript(), mjs: () => javascript(), cjs: () => javascript(),
  jsx: () => javascript({ jsx: true }),
  ts: () => javascript({ typescript: true }), mts: () => javascript({ typescript: true }), cts: () => javascript({ typescript: true }),
  tsx: () => javascript({ jsx: true, typescript: true }),
  // Web
  html: () => html(), htm: () => html(), svelte: () => html(), vue: () => html(), astro: () => html(),
  css: () => css(), scss: () => css(), less: () => css(),
  // Data
  json: () => json(), jsonc: () => json(), json5: () => json(),
  yaml: () => yaml(), yml: () => yaml(), toml: () => yaml(),
  xml: () => xml(),
  // Systems
  rs: () => rust(),
  go: () => go(),
  c: () => cpp(), cpp: () => cpp(), h: () => cpp(), hpp: () => cpp(), cc: () => cpp(),
  // Scripting
  py: () => python(),
  rb: () => python(), // Closest available
  lua: () => python(),
  sh: () => python(), bash: () => python(), zsh: () => python(),
  // JVM / .NET
  java: () => java(), kt: () => java(), scala: () => java(), groovy: () => java(),
  cs: () => java(),
  // Docs
  md: () => markdown(), mdx: () => markdown(),
  // SQL
  sql: () => sql(),
  // PHP
  php: () => php(),
}

const FILENAME_LANG_MAP: Record<string, LanguageFn> = {
  'Dockerfile': () => python(),
  'Makefile': () => python(),
  'Gemfile': () => python(),
  'Rakefile': () => python(),
  '.gitignore': () => python(),
  '.env': () => python(),
}

function getLanguageExtension(filePath: string): Extension | null {
  const name = filePath.split('/').pop() || ''

  // Check full filename first
  if (FILENAME_LANG_MAP[name]) return FILENAME_LANG_MAP[name]()

  // Check extension
  const parts = name.split('.')
  if (parts.length > 1) {
    const ext = parts.pop()!.toLowerCase()
    if (EXT_LANG_MAP[ext]) return EXT_LANG_MAP[ext]()
  }

  return null
}

// ── K2SO dark theme (matches app aesthetic) ─────────────────────────

const k2soTheme = EditorView.theme({
  '&': {
    backgroundColor: '#0a0a0a',
    color: '#e4e4e7',
    fontSize: '12px',
    fontFamily: '"MesloLGM Nerd Font", "Menlo", "Monaco", "Courier New", monospace',
    height: '100%',
  },
  '.cm-content': {
    caretColor: '#3b82f6',
    padding: '8px 0',
  },
  '.cm-cursor, .cm-dropCursor': {
    borderLeftColor: '#3b82f6',
    borderLeftWidth: '2px',
  },
  '.cm-selectionBackground, ::selection': {
    backgroundColor: '#3b82f633 !important',
  },
  '.cm-activeLine': {
    backgroundColor: '#ffffff08',
  },
  '.cm-activeLineGutter': {
    backgroundColor: '#ffffff08',
  },
  '.cm-gutters': {
    backgroundColor: '#0a0a0a',
    color: '#555',
    border: 'none',
    borderRight: '1px solid #1a1a1a',
  },
  '.cm-lineNumbers .cm-gutterElement': {
    padding: '0 8px 0 16px',
    minWidth: '3em',
    fontSize: '11px',
  },
  '.cm-scroller': {
    overflow: 'auto',
    lineHeight: '1.6',
  },
  '.cm-matchingBracket': {
    backgroundColor: '#3b82f633',
    outline: '1px solid #3b82f666',
  },
  '.cm-searchMatch': {
    backgroundColor: '#b5890066',
  },
  '.cm-searchMatch.cm-searchMatch-selected': {
    backgroundColor: '#b58900aa',
  },
  '.cm-selectionMatch': {
    backgroundColor: '#3b82f622',
  },
  '.cm-foldPlaceholder': {
    backgroundColor: '#1a1a1a',
    border: '1px solid #2a2a2a',
    color: '#888',
  },
  '&.cm-focused': {
    outline: 'none',
  },
}, { dark: true })

// ── Component ────────────────────────────────────────────────────────

interface CodeEditorProps {
  code: string
  filePath: string
  onSave: (content: string) => void
  onChange: (content: string) => void
  readOnly?: boolean
}

export function CodeEditor({ code, filePath, onSave, onChange, readOnly = false }: CodeEditorProps): React.JSX.Element {
  const containerRef = useRef<HTMLDivElement>(null)
  const viewRef = useRef<EditorView | null>(null)
  const onSaveRef = useRef(onSave)
  const onChangeRef = useRef(onChange)
  const codeRef = useRef(code)

  // Keep refs current
  onSaveRef.current = onSave
  onChangeRef.current = onChange
  codeRef.current = code

  // Create editor on mount
  useEffect(() => {
    if (!containerRef.current) return

    const langExt = getLanguageExtension(filePath)

    const saveKeymap = keymap.of([{
      key: 'Mod-s',
      run: (view) => {
        onSaveRef.current(view.state.doc.toString())
        return true
      },
    }])

    const updateListener = EditorView.updateListener.of((update) => {
      if (update.docChanged) {
        onChangeRef.current(update.state.doc.toString())
      }
    })

    const extensions: Extension[] = [
      // Core editing
      history(),
      drawSelection(),
      rectangularSelection(),
      indentOnInput(),
      bracketMatching(),
      highlightActiveLine(),
      highlightActiveLineGutter(),
      highlightSelectionMatches(),
      // Line numbers
      lineNumbers(),
      // Keymaps
      saveKeymap,
      keymap.of([
        ...defaultKeymap,
        ...historyKeymap,
        ...searchKeymap,
        indentWithTab,
      ]),
      // Syntax highlighting (oneDark colors, our custom chrome)
      syntaxHighlighting(oneDarkHighlightStyle),
      syntaxHighlighting(defaultHighlightStyle, { fallback: true }),
      // Theme (our dark chrome only — no oneDark background)
      k2soTheme,
      // Change tracking
      updateListener,
    ]

    if (langExt) extensions.push(langExt)
    if (readOnly) extensions.push(EditorState.readOnly.of(true))

    const state = EditorState.create({
      doc: code,
      extensions,
    })

    const view = new EditorView({
      state,
      parent: containerRef.current,
    })

    viewRef.current = view

    return () => {
      view.destroy()
      viewRef.current = null
    }
  }, [filePath, readOnly]) // Recreate when file or readOnly changes

  // Update content when external changes arrive (file polling)
  useEffect(() => {
    const view = viewRef.current
    if (!view) return

    const currentContent = view.state.doc.toString()
    if (code !== currentContent) {
      // Replace entire document content
      view.dispatch({
        changes: {
          from: 0,
          to: view.state.doc.length,
          insert: code,
        },
      })
    }
  }, [code])

  return (
    <div
      ref={containerRef}
      className="h-full w-full overflow-hidden"
    />
  )
}
