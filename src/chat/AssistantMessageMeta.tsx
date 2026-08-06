import { useState } from 'react'
import { Check, Copy, Gauge, GitBranch, NotebookPen, RotateCcw, Trash2 } from 'lucide-react'
import { IconButton } from '../components/Button'
import { copyToClipboard } from '../utils/clipboard'
import { estimateTokens, formatTokensK } from '../utils/tokens'
import { formatAssistantMessageTime } from './messageFormat'
import { useHoverReveal } from './useHoverReveal'
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
  onDelete?: () => void
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
  onDelete,
  onSaveToNote,
}: AssistantMessageMetaProps) {
  const [copied, setCopied] = useState(false)
  const [saved, setSaved] = useState(false)
  // hover 显隐：指针停在这一条自己的位置上才浮现（事件驱动，原因见 useHoverReveal）。
  // 行平时透明但保留占位，可被指针命中。
  const { ref: rowRef, hovered } = useHoverReveal<HTMLDivElement>()
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
    // 默认隐藏，指针停到这一条的位置才浮现（悬停边界就是本行自身）。`py-1 -my-1`
    // 在不改变视觉布局的前提下扩大命中区，低矮的一行不至于难瞄。
    // focus-within 兜住键盘导航：Tab 到按钮时行必须可见。字号 11px、按钮 xs：
    // 这是元信息，不能跟正文抢对比度。
    <div
      ref={rowRef}
      className={`mt-0.5 flex flex-wrap items-center gap-x-2.5 gap-y-1 py-1 text-[11px] text-neutral-400 transition-opacity duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-out)] focus-within:opacity-100 dark:text-neutral-500 ${hovered ? 'opacity-100' : 'opacity-0'}`}
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
        <IconButton
          size="xs"
          onClick={onDelete}
          disabled={!onDelete}
          label="删除"
        >
          <Trash2 size={13} strokeWidth={2} />
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
