import { useState } from 'react'
import { Check, Copy, Gauge, GitBranch, NotebookPen, RotateCcw } from 'lucide-react'
import { IconButton } from '../components/Button'
import { copyToClipboard } from '../utils/clipboard'
import { estimateTokens, formatTokensK } from '../utils/tokens'
import { formatAssistantMessageTime } from './messageFormat'
import type { MessageUsage } from './types'

interface AssistantMessageMetaProps {
  content: string
  reasoning?: string
  timestamp: number
  tokensPerSec?: number
  runEntry?: string | null
  streamOutcome?: string | null
  usage?: MessageUsage | null
  onRegenerate?: () => void
  onFork?: () => void
  onSaveToNote?: () => Promise<boolean> | boolean
}

/** Provider 报告的真实 token 数（输入+输出聚合的 total，或输出 token）；没有则 null。 */
function realUsageTokens(usage?: MessageUsage | null): { total: number; label: string } | null {
  if (!usage) return null
  const output = usage.output_tokens ?? usage.outputTokens
  const input = usage.input_tokens ?? usage.inputTokens
  const total = usage.total_tokens ?? usage.totalTokens
  // 千位以上收成 K（`↑38897` → `↑38.9K`）：这一行是回答下面的元信息条，精确到个位既没人读、
  // 又比旁边的上下文用量条（一直是 K）长出一截。口径与那条一致，用同一个 formatTokensK。
  if (output != null && input != null) {
    return {
      total: input + output,
      label: `↑${formatTokensK(input)} ↓${formatTokensK(output)}`,
    }
  }
  if (total != null) return { total, label: `${formatTokensK(total)} tokens` }
  if (output != null) return { total: output, label: `↓${formatTokensK(output)}` }
  return null
}

export function AssistantMessageMeta({
  content,
  reasoning,
  timestamp,
  tokensPerSec,
  runEntry,
  streamOutcome,
  usage,
  onRegenerate,
  onFork,
  onSaveToNote,
}: AssistantMessageMetaProps) {
  const [copied, setCopied] = useState(false)
  const [saved, setSaved] = useState(false)
  // 优先显示 provider 报告的真实用量；provider 不报时回落到 chars 估算（带 ~ 前缀）。
  const realUsage = realUsageTokens(usage)
  const tokenLabel = realUsage
    ? realUsage.label
    : `~${formatTokensK(estimateTokens(`${content}${reasoning ? `\n${reasoning}` : ''}`))} tokens`
  const speed =
    tokensPerSec != null && Number.isFinite(tokensPerSec)
      ? Math.max(1, Math.round(tokensPerSec))
      : null

  const handleCopy = async () => {
    const ok = await copyToClipboard(content)
    if (!ok) return
    setCopied(true)
    window.setTimeout(() => setCopied(false), 2000)
  }

  const handleSaveToNote = async () => {
    if (!onSaveToNote) return
    const ok = await Promise.resolve(onSaveToNote())
    if (!ok) return
    setSaved(true)
    window.setTimeout(() => setSaved(false), 2000)
  }

  const runEntryLabel = runEntry === 'regenerate' ? '已重新生成' : null
  const streamOutcomeLabel =
    streamOutcome === 'cancelled'
      ? '已停止后继续'
      : streamOutcome === 'error'
        ? '生成异常结束'
        : streamOutcome === 'interrupted'
          ? '运行中断，未完成'
          : null

  return (
    // 悬停显隐由祖先 MessageBubble 维护的 `data-msg-hovered` DOM 属性 + CSS
    // （index.css 的 `[data-msg-hovered] .msg-hover-reveal`）驱动，不走 React state
    // ——滚动时消息从光标下滑过会连环触发 enter/leave，走 state 会整棵气泡重渲。
    // focus-within 兜住键盘导航（Tab 到按钮时行必须可见）。字号 11px、按钮 xs。
    // [will-change:opacity] 把本行提升为独立合成层：WKWebView 对非合成层的 opacity
    // 变化存在重绘失效（探针实测 computed opacity 已到 0、屏幕上旧画面滞留）；合成层
    // 的 opacity 由合成器每帧应用，不走重绘路径。**别删**——删了显隐在 macOS 上会
    // 间歇性卡在可见。
    <div
      className="msg-hover-reveal mt-1.5 flex flex-wrap items-center gap-x-2.5 gap-y-1 text-[11px] text-neutral-400 opacity-0 transition-opacity duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-out)] [will-change:opacity] focus-within:opacity-100 dark:text-neutral-500"
    >
      <span className="shrink-0">{formatAssistantMessageTime(timestamp)}</span>
      {runEntryLabel && <span className="shrink-0">{runEntryLabel}</span>}
      {streamOutcomeLabel && <span className="shrink-0">{streamOutcomeLabel}</span>}

      <div className="flex items-center gap-0.5">
        <IconButton
          size="xs"
          onClick={() => void handleCopy()}
          label={copied ? '已复制' : '复制'}
        >
          {copied ? <Check size={13} strokeWidth={2} className="chat-motion-pop" /> : <Copy size={13} strokeWidth={2} />}
        </IconButton>
        <IconButton
          size="xs"
          onClick={() => void handleSaveToNote()}
          disabled={!onSaveToNote}
          label={saved ? '已存为笔记' : '存为笔记'}
        >
          {saved ? <Check size={13} strokeWidth={2} className="chat-motion-pop" /> : <NotebookPen size={13} strokeWidth={2} />}
        </IconButton>
        <IconButton
          size="xs"
          onClick={onRegenerate}
          disabled={!onRegenerate}
          label="重新生成"
        >
          <RotateCcw size={13} strokeWidth={2} />
        </IconButton>
        <IconButton
          size="xs"
          onClick={onFork}
          disabled={!onFork}
          label="建分支"
          title="从这里建分支（复制到新对话）"
        >
          <GitBranch size={13} strokeWidth={2} />
        </IconButton>
      </div>

      {speed != null && (
        <span className="inline-flex items-center gap-1">
          <Gauge size={12} strokeWidth={2} />
          <span>{speed} tokens/sec</span>
        </span>
      )}

      <span className="text-neutral-400 dark:text-neutral-500">{tokenLabel}</span>
    </div>
  )
}
