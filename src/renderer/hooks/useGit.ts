import { useState, useEffect, useRef, useCallback } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { GIT_POLL_INTERVAL } from '@shared/constants'

// ── Types ────────────────────────────────────────────────────────────────────

interface GitInfo {
  isRepo: boolean
  currentBranch: string
  ahead: number
  behind: number
  changedFiles: number
  untrackedFiles: number
}

interface ChangedFile {
  path: string
  status: 'modified' | 'added' | 'deleted' | 'untracked'
}

interface UseGitInfoResult {
  data: GitInfo | null
  loading: boolean
  error: string | null
  refetch: () => void
}

interface UseGitChangesResult {
  data: ChangedFile[]
  loading: boolean
  error: string | null
  refetch: () => void
}

// ── useGitInfo ───────────────────────────────────────────────────────────────

export function useGitInfo(projectPath?: string): UseGitInfoResult {
  const [data, setData] = useState<GitInfo | null>(null)
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null)

  const fetch = useCallback(async () => {
    if (!projectPath) {
      setData(null)
      return
    }

    try {
      setLoading((prev) => (prev ? prev : true))
      const result = await invoke<GitInfo>('git_info', { path: projectPath })
      setData(result)
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to fetch git info')
    } finally {
      setLoading(false)
    }
  }, [projectPath])

  useEffect(() => {
    fetch()

    // Auto-refresh every 5 seconds
    intervalRef.current = setInterval(fetch, GIT_POLL_INTERVAL)

    return () => {
      if (intervalRef.current) {
        clearInterval(intervalRef.current)
      }
    }
  }, [fetch])

  return { data, loading, error, refetch: fetch }
}

// ── useGitChanges ────────────────────────────────────────────────────────────

export function useGitChanges(projectPath?: string): UseGitChangesResult {
  const [data, setData] = useState<ChangedFile[]>([])
  const [loading, setLoading] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const intervalRef = useRef<ReturnType<typeof setInterval> | null>(null)

  const fetch = useCallback(async () => {
    if (!projectPath) {
      setData([])
      return
    }

    try {
      setLoading((prev) => (prev ? prev : true))
      const result = await invoke<ChangedFile[]>('git_changes', { path: projectPath })
      setData(result)
      setError(null)
    } catch (e) {
      setError(e instanceof Error ? e.message : 'Failed to fetch git changes')
    } finally {
      setLoading(false)
    }
  }, [projectPath])

  useEffect(() => {
    fetch()

    intervalRef.current = setInterval(fetch, GIT_POLL_INTERVAL)

    return () => {
      if (intervalRef.current) {
        clearInterval(intervalRef.current)
      }
    }
  }, [fetch])

  return { data, loading, error, refetch: fetch }
}
