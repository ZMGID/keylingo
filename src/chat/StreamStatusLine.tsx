import { memo, useEffect, useState } from 'react'
import { KivioBlob, type BlobMood } from './KivioBlob'
import { getCoarse, getSnapshot } from './streamingStore'
import { resolveBlobMood } from './kivioBlobSim'
import { useConversationTransition } from './conversationTransitionStore'

export const StreamStatusLogo = memo(function StreamStatusLogo({
  size = 28,
  mood = 'idle',
  paused = false,
}: {
  size?: number
  mood?: BlobMood
  paused?: boolean
}) {
  return <KivioBlob size={size} mood={mood} paused={paused} />
})

function toolName(tool: { tool_name?: string; toolName?: string; name?: string }): string {
  return tool.tool_name || tool.toolName || tool.name || ''
}

function moodFromRun(active: boolean): BlobMood {
  const snapshot = getSnapshot()
  const coarse = getCoarse()
  return resolveBlobMood({
    active,
    error: Boolean(coarse.streamError),
    contentLen: snapshot.content.length,
    reasoningStreaming: snapshot.reasoningStreaming,
    runningToolNames: snapshot.toolCalls
      .filter((tool) => tool.status === 'running')
      .map(toolName),
  })
}

function formatElapsed(ms: number): string {
  const total = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(total / 3600)
  const m = Math.floor((total % 3600) / 60)
  const s = total % 60
  if (h > 0) return `${h}h ${m}m ${s}s`
  if (m > 0) return `${m}m ${s}s`
  return `${s}s`
}

function formatTokens(n: number): string {
  if (n >= 10000) return `${Math.round(n / 1000)}k`
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`
  return `${n}`
}

/** CJK 按 1 字 1 token、其余 4 字符 1 token（与 Rust chunking 的口径一致的粗估）。 */
function estimateTokens(text: string): number {
  let cjk = 0
  for (const ch of text) {
    if (ch.codePointAt(0)! > 0x2e7f) cjk++
  }
  return Math.round(cjk + (text.length - cjk) / 4)
}

interface StreamStatusLineProps {
  /** 生成中（按检索/干活/说话/思考切脸 + 实时信息）；false = 闲置或出错脸 */
  active: boolean
}

interface LiveStats {
  elapsedMs: number
  tokens: number
  running: number
  statusNote: string | null
}

/**
 * 消息流末尾常驻的存在标记（对标 Claude Code 的小星号）：闲置时蓝色墨团眨眼，
 * 生成中按状态切脸（思考 / 检索 / 干活 / 说话）+ 「耗时 · N tokens · K running」。
 *
 * 实时信息不走 props、不订阅 store（那是每帧一响）：250ms 从 streamingStore 快照
 * 读一次，token 从已流出的正文 + 思维链估算 —— 任何运行时（内置 / 外部 CLI）都有数，
 * 且和 Claude 一样随输出平滑上涨；后端的 live 用量事件太稀（每 planning 轮一次）不用。
 */
export const StreamStatusLine = memo(function StreamStatusLine({ active }: StreamStatusLineProps) {
  const { loading } = useConversationTransition()
  const [stats, setStats] = useState<LiveStats | null>(null)
  const [mood, setMood] = useState<BlobMood>(() => moodFromRun(active))
  useEffect(() => {
    const update = () => {
      setMood(moodFromRun(active))
      if (!active) {
        setStats(null)
        return
      }
      const snapshot = getSnapshot()
      setStats({
        elapsedMs: snapshot.startedAt != null ? Date.now() - snapshot.startedAt : 0,
        tokens: estimateTokens(snapshot.content) + estimateTokens(snapshot.reasoning),
        running: snapshot.toolCalls.reduce(
          (n, tool) => (tool.status === 'running' ? n + 1 : n),
          0,
        ),
        statusNote: snapshot.statusNote,
      })
    }
    update()
    const id = setInterval(update, active ? 250 : 800)
    return () => clearInterval(id)
  }, [active])

  const parts: string[] = []
  if (stats) {
    parts.push(formatElapsed(stats.elapsedMs))
    if (stats.tokens > 0) parts.push(`${formatTokens(stats.tokens)} tokens`)
    if (stats.running > 0) parts.push(`${stats.running} running`)
    if (stats.statusNote) parts.push(stats.statusNote)
  }

  return (
    <div
      className={`flex items-center gap-2.5 py-2${active ? ' chat-motion-fade-up' : ' kv-stream-status-idle'}`}
    >
      <StreamStatusLogo mood={mood} paused={loading} />
      {active && stats && (
        <span className="text-xs tabular-nums text-neutral-500 dark:text-neutral-400">
          {parts.join(' · ')}
        </span>
      )}
    </div>
  )
})
