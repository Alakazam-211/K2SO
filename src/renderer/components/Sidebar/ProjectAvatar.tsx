import { useState, useEffect } from 'react'
import { trpc } from '@/lib/trpc'

// Cache icon results across component instances
const iconCache = new Map<string, { found: boolean; dataUrl: string | null }>()

interface ProjectAvatarProps {
  projectPath: string
  projectName: string
  projectColor: string
  projectId?: string
  iconUrl?: string | null
  size?: number
  showActivity?: boolean
}

export default function ProjectAvatar({
  projectPath,
  projectName,
  projectColor,
  projectId,
  iconUrl: iconUrlProp,
  size = 20,
  showActivity = false
}: ProjectAvatarProps): React.JSX.Element {
  const [iconUrl, setIconUrl] = useState<string | null>(iconUrlProp ?? null)
  const [loaded, setLoaded] = useState(!!iconUrlProp)
  const [activeTerminals, setActiveTerminals] = useState(0)

  // Sync prop changes
  useEffect(() => {
    if (iconUrlProp) {
      setIconUrl(iconUrlProp)
      setLoaded(true)
    }
  }, [iconUrlProp])

  useEffect(() => {
    // If iconUrl was provided via prop, skip the query
    if (iconUrlProp) return

    // Check cache first
    const cached = iconCache.get(projectPath)
    if (cached) {
      if (cached.found && cached.dataUrl) {
        setIconUrl(cached.dataUrl)
      }
      setLoaded(true)
      return
    }

    let cancelled = false
    trpc.projects.getIcon
      .query({ path: projectPath, projectId })
      .then((result) => {
        iconCache.set(projectPath, result)
        if (!cancelled && result.found && result.dataUrl) {
          setIconUrl(result.dataUrl)
        }
        if (!cancelled) setLoaded(true)
      })
      .catch(() => {
        iconCache.set(projectPath, { found: false, dataUrl: null })
        if (!cancelled) setLoaded(true)
      })

    return () => {
      cancelled = true
    }
  }, [projectPath, iconUrlProp, projectId])

  // Poll for active terminal count
  useEffect(() => {
    if (!showActivity) return

    let cancelled = false

    const poll = (): void => {
      trpc.terminal.activeCountForPath
        .query({ path: projectPath })
        .then((count) => {
          if (!cancelled) setActiveTerminals(count)
        })
        .catch(() => {
          // ignore
        })
    }

    poll()
    const interval = setInterval(poll, 3000)

    return () => {
      cancelled = true
      clearInterval(interval)
    }
  }, [projectPath, showActivity])

  const firstLetter = projectName.charAt(0).toUpperCase()
  const hasActivity = showActivity && activeTerminals > 0

  const indicator = hasActivity ? (
    <span
      className="terminal-active-indicator"
      style={{
        position: 'absolute',
        bottom: -1,
        right: -1,
        width: 8,
        height: 8,
        backgroundColor: '#22c55e',
        border: '1.5px solid var(--color-bg-surface)',
        display: 'block',
        zIndex: 1
      }}
    />
  ) : null

  if (iconUrl) {
    return (
      <span className="relative flex-shrink-0" style={{ width: size, height: size }}>
        <img
          src={iconUrl}
          alt={projectName}
          className="object-contain"
          style={{
            width: size - 4,
            height: size - 4,
            border: `2px solid ${projectColor}`,
            imageRendering: 'auto',
            display: 'block',
            marginTop: 0
          }}
        />
        {indicator}
      </span>
    )
  }

  return (
    <span className="relative flex-shrink-0" style={{ width: size, height: size }}>
      <span
        className="flex items-center justify-center"
        style={{
          width: size,
          height: size,
          backgroundColor: loaded ? projectColor : 'var(--color-bg-elevated)',
          color: '#ffffff',
          fontSize: size * 0.5,
          fontWeight: 700,
          lineHeight: 1,
          fontFamily: 'inherit',
          display: 'flex'
        }}
      >
        {firstLetter}
      </span>
      {indicator}
    </span>
  )
}
