/**
 * Terminal link detection — finds URLs and file paths in terminal line text.
 * Results are cached by line text to avoid re-running regex on unchanged lines.
 */

// ── Types ────────────────────────────────────────────────────────────

export interface DetectedLink {
  start: number    // char index (inclusive)
  end: number      // char index (exclusive)
  type: 'url' | 'file'
  target: string   // raw matched text
  filePath?: string  // resolved absolute path (file type only)
  line?: number
  col?: number
}

// ── Regex patterns ───────────────────────────────────────────────────

// URLs: multiple schemes followed by non-whitespace, non-bracket chars
const URL_RE = /(?:https?|ftp|ssh|file|gemini|gopher|ipfs|ipns):\/\/[^\s<>'")\]{}]+|mailto:[^\s<>'")\]{}]+/g

// File paths: require at least one `/` and a file extension
// Matches: src/foo.tsx, ./foo/bar.ts, /absolute/path.rs, ../relative.js
// Also matches git diff prefixes: a/src/foo.ts, b/src/foo.ts
// Optional :line or :line:col suffix
const FILE_PATH_RE = /(?:[ab]\/|\.{0,2}\/)?(?:[\w@._-]+\/)+[\w@._-]+\.\w+(?::\d+(?::\d+)?)?/g

// Bracket pairs for balanced cleanup
const BRACKET_PAIRS: Record<string, string> = { ')': '(', ']': '[', '}': '{', '>': '<' }

// ── LRU Cache ────────────────────────────────────────────────────────

const MAX_CACHE_SIZE = 500
const cache = new Map<string, DetectedLink[]>()

function cacheGet(key: string): DetectedLink[] | undefined {
  const val = cache.get(key)
  if (val !== undefined) {
    cache.delete(key)
    cache.set(key, val)
  }
  return val
}

function cacheSet(key: string, value: DetectedLink[]): void {
  if (cache.size >= MAX_CACHE_SIZE) {
    const firstKey = cache.keys().next().value
    if (firstKey !== undefined) cache.delete(firstKey)
  }
  cache.set(key, value)
}

// ── Detection helpers ────────────────────────────────────────────────

function cleanTrailingPunct(url: string): string {
  // Iteratively strip trailing punctuation/brackets that are unbalanced
  let cleaned = url
  let changed = true
  while (changed) {
    changed = false
    const last = cleaned[cleaned.length - 1]
    // Strip trailing punctuation
    if (last && '.,;:!?\'"'.includes(last)) {
      cleaned = cleaned.slice(0, -1)
      changed = true
      continue
    }
    // Strip unbalanced closing brackets
    const opener = last ? BRACKET_PAIRS[last] : undefined
    if (opener) {
      const opens = (cleaned.match(new RegExp('\\' + opener, 'g')) || []).length
      const closes = (cleaned.match(new RegExp('\\' + last, 'g')) || []).length
      if (closes > opens) {
        cleaned = cleaned.slice(0, -1)
        changed = true
      }
    }
  }
  return cleaned
}

function parseFileSuffix(target: string): { path: string; line?: number; col?: number } {
  const colonMatch = target.match(/:(\d+)(?::(\d+))?$/)
  if (colonMatch) {
    const path = target.slice(0, target.indexOf(':' + colonMatch[1]))
    return {
      path,
      line: parseInt(colonMatch[1], 10),
      col: colonMatch[2] ? parseInt(colonMatch[2], 10) : undefined,
    }
  }
  return { path: target }
}

function stripGitDiffPrefix(path: string): string {
  // Strip a/ or b/ prefixes from git diff output
  if (path.startsWith('a/') || path.startsWith('b/')) {
    return path.slice(2)
  }
  return path
}

// ── Main detection ───────────────────────────────────────────────────

export function detectLinks(text: string, cwd: string): DetectedLink[] {
  if (!text || text.trim().length === 0) return []

  const cacheKey = `${cwd}\0${text}`
  const cached = cacheGet(cacheKey)
  if (cached) return cached

  const links: DetectedLink[] = []
  const chars = [...text]

  // Build mapping: string index → char index (for multi-byte support)
  const strToChar = new Map<number, number>()
  let strIdx = 0
  for (let ci = 0; ci < chars.length; ci++) {
    strToChar.set(strIdx, ci)
    strIdx += chars[ci].length
  }
  strToChar.set(strIdx, chars.length)

  function strPosToCharIdx(pos: number): number {
    const exact = strToChar.get(pos)
    if (exact !== undefined) return exact
    let best = 0
    for (const [sp, ci] of strToChar) {
      if (sp <= pos) best = ci
      else break
    }
    return best
  }

  // Detect URLs
  URL_RE.lastIndex = 0
  let match: RegExpExecArray | null
  while ((match = URL_RE.exec(text)) !== null) {
    const raw = match[0]
    const cleaned = cleanTrailingPunct(raw)
    if (cleaned.length < 10) continue

    const start = strPosToCharIdx(match.index)
    const end = start + [...cleaned].length

    links.push({
      start,
      end,
      type: 'url',
      target: cleaned,
    })
  }

  // Detect file paths
  FILE_PATH_RE.lastIndex = 0
  while ((match = FILE_PATH_RE.exec(text)) !== null) {
    const raw = match[0]
    const start = strPosToCharIdx(match.index)
    const end = start + [...raw].length

    // Skip if this range overlaps with an already-detected URL
    const overlaps = links.some((l) => start < l.end && end > l.start)
    if (overlaps) continue

    const { path, line, col } = parseFileSuffix(raw)

    // Strip git diff prefixes and resolve relative paths
    const stripped = stripGitDiffPrefix(path)
    const absolutePath = stripped.startsWith('/') ? stripped : `${cwd}/${stripped}`

    links.push({
      start,
      end,
      type: 'file',
      target: raw,
      filePath: absolutePath,
      line,
      col,
    })
  }

  links.sort((a, b) => a.start - b.start)

  cacheSet(cacheKey, links)
  return links
}
