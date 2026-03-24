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

// URLs: http:// or https:// followed by non-whitespace, non-bracket chars
const URL_RE = /https?:\/\/[^\s<>'")\]]+/g

// File paths: require at least one `/` and a file extension
// Matches: src/foo.tsx, ./foo/bar.ts, /absolute/path.rs, ../relative.js
// Optional :line or :line:col suffix
const FILE_PATH_RE = /(?:\.{0,2}\/)?(?:[\w@._-]+\/)+[\w@._-]+\.\w+(?::\d+(?::\d+)?)?/g

// Trailing punctuation that shouldn't be part of a URL
const TRAILING_PUNCT_RE = /[.,;:!?)]+$/

// ── LRU Cache ────────────────────────────────────────────────────────

const MAX_CACHE_SIZE = 500
const cache = new Map<string, DetectedLink[]>()

function cacheGet(key: string): DetectedLink[] | undefined {
  const val = cache.get(key)
  if (val !== undefined) {
    // Move to end (most recently used)
    cache.delete(key)
    cache.set(key, val)
  }
  return val
}

function cacheSet(key: string, value: DetectedLink[]): void {
  if (cache.size >= MAX_CACHE_SIZE) {
    // Evict oldest entry
    const firstKey = cache.keys().next().value
    if (firstKey !== undefined) cache.delete(firstKey)
  }
  cache.set(key, value)
}

// ── Detection ────────────────────────────────────────────────────────

function cleanTrailingPunct(url: string): string {
  // Remove trailing punctuation that's likely not part of the URL
  // But be careful with balanced parens: https://en.wikipedia.org/wiki/Foo_(bar)
  let cleaned = url
  const match = cleaned.match(TRAILING_PUNCT_RE)
  if (match) {
    const trail = match[0]
    // Keep trailing ) if there's a matching ( in the URL
    if (trail.includes(')') && cleaned.includes('(')) {
      // Only strip trailing chars after the last balanced )
      cleaned = cleaned.replace(/[.,;:!]+$/, '')
    } else {
      cleaned = cleaned.slice(0, -trail.length)
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

export function detectLinks(text: string, cwd: string): DetectedLink[] {
  if (!text || text.trim().length === 0) return []

  const cacheKey = `${cwd}\0${text}`
  const cached = cacheGet(cacheKey)
  if (cached) return cached

  const links: DetectedLink[] = []
  const chars = [...text]

  // We need to map byte/string offsets to char indices for multi-byte support
  // Build a mapping: string index → char index
  const strToChar = new Map<number, number>()
  let strIdx = 0
  for (let ci = 0; ci < chars.length; ci++) {
    strToChar.set(strIdx, ci)
    strIdx += chars[ci].length
  }
  strToChar.set(strIdx, chars.length) // sentinel

  function strPosToCharIdx(pos: number): number {
    // Find the closest char index for a string position
    const exact = strToChar.get(pos)
    if (exact !== undefined) return exact
    // Fallback: find the nearest lower entry
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
    if (cleaned.length < 10) continue // too short to be a real URL

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

    // Resolve relative paths against cwd
    const absolutePath = path.startsWith('/') ? path : `${cwd}/${path}`

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

  // Sort by start position
  links.sort((a, b) => a.start - b.start)

  cacheSet(cacheKey, links)
  return links
}
