import type { CSSProperties } from 'react'

const ICON_EXT: Record<string, 'svg'> = {
  claude: 'svg',
  codex: 'svg',
  'cursor-agent': 'svg',
  opencode: 'svg',
  gemini: 'svg',
  kimi: 'svg',
  pi: 'svg',
  hermes: 'svg',
  grok: 'svg',
  dsh: 'svg',
}

const MONO_ICONS = new Set(['cursor-agent', 'opencode', 'hermes', 'grok'])

interface AgentIconProps {
  id: string
  size?: number
  className?: string
}

export function AgentIcon({ id, size = 20, className }: AgentIconProps) {
  const cls = ['agent-icon', className].filter(Boolean).join(' ')
  const ext = ICON_EXT[id]
  if (ext === 'svg' && MONO_ICONS.has(id)) {
    const src = `/agent-icons/${id}.svg`
    const style: CSSProperties = {
      width: size,
      height: size,
      WebkitMaskImage: `url("${src}")`,
      maskImage: `url("${src}")`,
    }
    return (
      <span
        className={`${cls} agent-icon-mono bg-neutral-800 dark:bg-neutral-200`}
        style={style}
        aria-hidden="true"
      />
    )
  }
  if (ext) {
    return (
      <img
        src={`/agent-icons/${id}.${ext}`}
        alt=""
        className={cls}
        width={size}
        height={size}
        draggable={false}
      />
    )
  }
  return (
    <span
      className={`${cls} inline-flex items-center justify-center rounded-md bg-neutral-200 text-[10px] font-semibold uppercase text-neutral-600 dark:bg-neutral-700 dark:text-neutral-300`}
      style={{ width: size, height: size }}
      aria-hidden="true"
    >
      {id.slice(0, 2)}
    </span>
  )
}
