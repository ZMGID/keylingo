import { memo, useEffect, useRef, useState } from 'react'
import { KivioBlob, type BlobMood } from './KivioBlob'
import { getCoarse, getSnapshot } from './streamingStore'
import { resolveBlobMood } from './kivioBlobSim'
import { isAskUserToolName } from './askUserTools'
import { useStatusQuip } from './blobQuips'
import { useConversationTransition } from './conversationTransitionStore'
import type { Lang } from '../settings/i18n'

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

/** 刚收工后小得意的窗口：happy 脸 + 蹦一下 + 「搞定」。 */
export const BLOB_DONE_MS = 2600

function moodFromRun(active: boolean, doneUntil: number, now: number): BlobMood {
  const snapshot = getSnapshot()
  const coarse = getCoarse()
  const running = snapshot.toolCalls.filter((tool) => tool.status === 'running')
  return resolveBlobMood({
    active,
    error: Boolean(coarse.streamError),
    contentLen: snapshot.content.length,
    reasoningStreaming: snapshot.reasoningStreaming,
    runningToolNames: running.map(toolName),
    waiting: running.some((tool) => isAskUserToolName(toolName(tool))),
    done: now < doneUntil,
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
  lang?: Lang
}

interface LiveStats {
  elapsedMs: number
  tokens: number
  running: number
  statusNote: string | null
}

/**
 * 消息流末尾常驻的存在标记（对标 Claude Code 的小星号）：闲置时蓝色墨团眨眼，
 * 生成中按状态切脸（思考 / 检索 / 干活 / 说话 / 等你）+ 「耗时 · N tokens · K running」，
 * 偶尔在末尾自己接一句嘴（blobQuips），收工那两秒得意一下。
 *
 * 实时信息不走 props、不订阅 store（那是每帧一响）：250ms 从 streamingStore 快照
 * 读一次，token 从已流出的正文 + 思维链估算 —— 任何运行时（内置 / 外部 CLI）都有数，
 * 且和 Claude 一样随输出平滑上涨；后端的 live 用量事件太稀（每 planning 轮一次）不用。
 */
export const StreamStatusLine = memo(function StreamStatusLine({ active, lang = 'zh' }: StreamStatusLineProps) {
  const { loading } = useConversationTransition()
  const [stats, setStats] = useState<LiveStats | null>(null)
  const doneUntilRef = useRef(0)
  const prevActiveRef = useRef(active)
  const [mood, setMood] = useState<BlobMood>(() => moodFromRun(active, 0, Date.now()))
  useEffect(() => {
    // 生成 true→false 且没翻车、没被停：进入「搞定」窗口。
    if (prevActiveRef.current && !active) {
      const coarse = getCoarse()
      doneUntilRef.current = coarse.streamError || coarse.cancelling ? 0 : Date.now() + BLOB_DONE_MS
    }
    prevActiveRef.current = active
    const update = () => {
      const now = Date.now()
      setMood(moodFromRun(active, doneUntilRef.current, now))
      if (!active) {
        setStats(null)
        return
      }
      const snapshot = getSnapshot()
      setStats({
        elapsedMs: snapshot.startedAt != null ? now - snapshot.startedAt : 0,
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
    // 「搞定」窗口到点要准时回闲置，不等 800ms 的下一拍。
    const doneLeft = doneUntilRef.current - Date.now()
    const doneTimer = doneLeft > 0 ? window.setTimeout(update, doneLeft + 20) : 0
    return () => {
      clearInterval(id)
      window.clearTimeout(doneTimer)
    }
  }, [active])

  const quip = useStatusQuip(mood, lang)

  const parts: string[] = []
  if (stats) {
    parts.push(formatElapsed(stats.elapsedMs))
    if (stats.tokens > 0) parts.push(`${formatTokens(stats.tokens)} tokens`)
    if (stats.running > 0) parts.push(`${stats.running} running`)
    if (stats.statusNote) parts.push(stats.statusNote)
  }
  // 后端的状态一行字优先，墨团别抢话。
  const showQuip = quip && !stats?.statusNote

  return (
    <div
      className={`flex items-center gap-2.5 py-2${active ? ' chat-motion-fade-up' : ' kv-stream-status-idle'}`}
    >
      <StreamStatusLogo mood={mood} paused={loading} />
      {(parts.length > 0 || showQuip) && (
        <span className="text-xs tabular-nums text-neutral-500 dark:text-neutral-400">
          {parts.join(' · ')}
          {showQuip && (
            <span key={quip} className="kv-blob-quip">
              {parts.length > 0 ? ' · ' : ''}
              {quip}
            </span>
          )}
        </span>
      )}
    </div>
  )
})
