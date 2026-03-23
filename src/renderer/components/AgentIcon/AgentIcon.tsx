/**
 * Agent preset icons — inline SVGs with brand-appropriate colors.
 */

interface AgentIconProps {
  agent: string
  size?: number
  className?: string
}

export default function AgentIcon({ agent, size = 14, className = '' }: AgentIconProps): React.JSX.Element {
  const normalized = agent.toLowerCase().trim()
  const s = size

  switch (normalized) {
    case 'claude':
      // Anthropic — warm copper starburst
      return (
        <svg width={s} height={s} viewBox="0 0 24 24" fill="none" className={className} style={{ flexShrink: 0 }}>
          <path d="M12 3L14.5 9.5L21 12L14.5 14.5L12 21L9.5 14.5L3 12L9.5 9.5L12 3Z" fill="#D4A574"/>
        </svg>
      )

    case 'codex':
      // OpenAI — green terminal brackets
      return (
        <svg width={s} height={s} viewBox="0 0 24 24" fill="none" className={className} style={{ flexShrink: 0 }}>
          <circle cx="12" cy="12" r="9" stroke="#10A37F" strokeWidth="1.5"/>
          <path d="M9.5 8.5L7 12L9.5 15.5" stroke="#10A37F" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"/>
          <path d="M14.5 8.5L17 12L14.5 15.5" stroke="#10A37F" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"/>
        </svg>
      )

    case 'gemini':
      // Google — blue four-point sparkle
      return (
        <svg width={s} height={s} viewBox="0 0 24 24" fill="none" className={className} style={{ flexShrink: 0 }}>
          <path d="M12 2C12 8.5 5.5 12 2 12C5.5 12 12 15.5 12 22C12 15.5 18.5 12 22 12C18.5 12 12 8.5 12 2Z" fill="#4285F4"/>
        </svg>
      )

    case 'copilot':
      // GitHub — purple visor face
      return (
        <svg width={s} height={s} viewBox="0 0 24 24" fill="none" className={className} style={{ flexShrink: 0 }}>
          <path d="M12 2C8.13 2 5 5.13 5 9V10C3.9 10 3 10.9 3 12V16C3 18.21 4.79 20 7 20H17C19.21 20 21 18.21 21 16V12C21 10.9 20.1 10 19 10V9C19 5.13 15.87 2 12 2Z" stroke="#6E40C9" strokeWidth="1.5" fill="none"/>
          <circle cx="9" cy="13" r="1.3" fill="#6E40C9"/>
          <circle cx="15" cy="13" r="1.3" fill="#6E40C9"/>
          <path d="M7 9V10H17V9" stroke="#6E40C9" strokeWidth="1"/>
        </svg>
      )

    case 'aider':
      // Aider — green monospace A
      return (
        <svg width={s} height={s} viewBox="0 0 24 24" fill="none" className={className} style={{ flexShrink: 0 }}>
          <rect x="3" y="3" width="18" height="18" stroke="#14B014" strokeWidth="1.2" fill="none"/>
          <text x="12" y="17" textAnchor="middle" fontFamily="monospace" fontSize="14" fontWeight="bold" fill="#14B014">A</text>
        </svg>
      )

    case 'cursor agent':
    case 'cursor-agent':
      // Cursor — cyan cursor arrow in box
      return (
        <svg width={s} height={s} viewBox="0 0 24 24" fill="none" className={className} style={{ flexShrink: 0 }}>
          <rect x="3" y="3" width="18" height="18" rx="3" stroke="#22D3EE" strokeWidth="1.5" fill="none"/>
          <path d="M8 6L8 17L11.5 14L14.5 18.5L16 17.5L13 13.5L17 12.5L8 6Z" fill="#22D3EE"/>
        </svg>
      )

    case 'opencode':
      // OpenCode — orange pixel grid
      return (
        <svg width={s} height={s} viewBox="0 0 24 24" fill="none" className={className} style={{ flexShrink: 0 }}>
          <rect x="4" y="4" width="6" height="6" fill="#FB923C"/>
          <rect x="14" y="4" width="6" height="6" fill="#FB923C" opacity="0.7"/>
          <rect x="4" y="14" width="6" height="6" fill="#FB923C" opacity="0.7"/>
          <rect x="14" y="14" width="6" height="6" fill="#FB923C" opacity="0.4"/>
        </svg>
      )

    case 'code puppy':
    case 'codepuppy':
      // Code Puppy — pink dog face
      return (
        <svg width={s} height={s} viewBox="0 0 24 24" fill="none" className={className} style={{ flexShrink: 0 }}>
          <path d="M5 9L3 5" stroke="#F472B6" strokeWidth="1.5" strokeLinecap="round"/>
          <path d="M19 9L21 5" stroke="#F472B6" strokeWidth="1.5" strokeLinecap="round"/>
          <ellipse cx="12" cy="14" rx="7" ry="6" stroke="#F472B6" strokeWidth="1.5"/>
          <circle cx="9.5" cy="12.5" r="1" fill="#F472B6"/>
          <circle cx="14.5" cy="12.5" r="1" fill="#F472B6"/>
          <ellipse cx="12" cy="16" rx="1.5" ry="1" fill="#F472B6"/>
        </svg>
      )

    default:
      // Generic terminal prompt
      return (
        <svg width={s} height={s} viewBox="0 0 24 24" fill="none" className={className} style={{ flexShrink: 0 }}>
          <path d="M7 8L12 12L7 16" stroke="#9CA3AF" strokeWidth="1.5" strokeLinecap="round" strokeLinejoin="round"/>
          <path d="M13 17H18" stroke="#9CA3AF" strokeWidth="1.5" strokeLinecap="round"/>
        </svg>
      )
  }
}
