import { useState } from 'react'
import { Check, Copy, Gauge, GitBranch, NotebookPen, Pencil, RotateCcw, Trash2 } from 'lucide-react'
import { IconButton } from '../components/Button'
import { copyToClipboard } from '../utils/clipboard'
import { estimateTokens } from '../utils/tokens'
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
  onEdit?: () => void
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
  if (output != null && input != null) {
    return { total: input + output, label: `↑${input} ↓${output}` }
  }
  if (total != null) return { total, label: `${total} tokens` }
  if (output != null) return { total: output, label: `↓${output}` }
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
  onEdit,
  onRegenerate,
  onFork,
  onDelete,
  onSaveToNote,
}: AssistantMessageMetaProps) {
  const [copied, setCopied] = useState(false)
  const [saved, setSaved] = useState(false)
  // 优先显示 provider 报告的真实用量；provider 不报时回落到 chars 估算（带 ~ 前缀）。
  const realUsage = realUsageTokens(usage)
  const tokenLabel = realUsage
    ? realUsage.label
    : `~${estimateTokens(`${content}${reasoning ? `\n${reasoning}` : ''}`)} tokens`
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
    <div className="mt-2.5 flex flex-wrap items-center gap-x-3 gap-y-1 text-xs text-neutral-400 dark:text-neutral-500">
      <span className="shrink-0">{formatAssistantMessageTime(timestamp)}</span>
      {runEntryLabel && <span className="shrink-0">{runEntryLabel}</span>}
      {streamOutcomeLabel && <span className="shrink-0">{streamOutcomeLabel}</span>}

      <div className="flex items-center gap-0.5">
        <IconButton
          size="sm"
          onClick={() => void handleCopy()}
          label={copied ? '已复制' : '复制'}
        >
          {copied ? <Check size={14} strokeWidth={2} className="chat-motion-pop" /> : <Copy size={14} strokeWidth={2} />}
        </IconButton>
        <IconButton
          size="sm"
          onClick={() => void handleSaveToNote()}
          disabled={!onSaveToNote}
          label={saved ? '已存为笔记' : '存为笔记'}
        >
          {saved ? <Check size={14} strokeWidth={2} className="chat-motion-pop" /> : <NotebookPen size={14} strokeWidth={2} />}
        </IconButton>
        <IconButton
          size="sm"
          onClick={onEdit}
          disabled={!onEdit}
          label="编辑"
        >
          <Pencil size={14} strokeWidth={2} />
        </IconButton>
        <IconButton
          size="sm"
          onClick={onRegenerate}
          disabled={!onRegenerate}
          label="重新生成"
        >
          <RotateCcw size={14} strokeWidth={2} />
        </IconButton>
        <IconButton
          size="sm"
          onClick={onFork}
          disabled={!onFork}
          label="建分支"
          title="从这里建分支（复制到新对话）"
        >
          <GitBranch size={14} strokeWidth={2} />
        </IconButton>
        <IconButton
          size="sm"
          onClick={onDelete}
          disabled={!onDelete}
          label="删除"
        >
          <Trash2 size={14} strokeWidth={2} />
        </IconButton>
      </div>

      {speed != null && (
        <span className="inline-flex items-center gap-1">
          <Gauge size={13} strokeWidth={2} />
          <span>{speed} tokens/sec</span>
        </span>
      )}

      <span className="text-neutral-400 dark:text-neutral-500">{tokenLabel}</span>
    </div>
  )
}
