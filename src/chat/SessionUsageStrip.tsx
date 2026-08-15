import { useMemo } from 'react'
import { formatTokensK } from '../utils/tokens'
import { i18n, type Lang } from '../settings/i18n'
import type { ChatMessage } from './types'

interface SessionUsageStripProps {
  messages: ChatMessage[]
  lang: Lang
  /** provider id → apiFormat；用于判断该 provider 的 input_tokens 是否已含缓存。
   *   Anthropic 的 input_tokens 是纯非缓存部分，OpenAI 系 / Gemini 的 input 含缓存。 */
  apiFormats?: Record<string, string>
  /** 会话主 provider 的 apiFormat（消息没带 provider_id / 表里没有时的兜底）。 */
  defaultApiFormat?: string
  /**
   * 覆盖「input 是否已含缓存」。外部 CLI 的用量形状跟会话挂的 Kivio provider 无关：
   * dsh / claude / pi 已从 input 里扣掉 cache（再减一次会变成 ↑0、缓存 100%）。
   * 不传则仍按 apiFormat 判断（Anthropic 不减，其余减）。
   */
  cacheIncludedInInput?: boolean
}

/** 缓存命中率：命中缓存的 token 占输入总量（新鲜 + 缓存）的比例。 */
function formatCachePercent(cached: number, input: number): string {
  const total = cached + input
  if (total <= 0) return ''
  const pct = (cached / total) * 100
  if (pct >= 10) return `${Math.round(pct)}%`
  if (pct > 0) return `${pct.toFixed(1)}%`
  return '0%'
}

/**
 * 当前对话的 provider 实报用量（输入框功能栏右侧的一行紧凑小字，口径对齐
 * AssistantMessageMeta 的 ↑↓ 写法与 usage.rs 的 apply_cost）：
 * - ↑ = Σ input_tokens（Anthropic 之外的家 input 已含缓存，先减去缓存部分）。
 * - 缓存 = 命中缓存占输入总量的百分比（Σ cached_input_tokens / (新鲜 + 缓存)）。
 * - ↓ = Σ output_tokens。
 * 没有任何一条消息带用量（provider 不报 / 纯空会话）时整条不渲染。
 */
export function SessionUsageStrip({
  messages,
  lang,
  apiFormats = {},
  defaultApiFormat = '',
  cacheIncludedInInput,
}: SessionUsageStripProps) {
  const totals = useMemo(() => {
    let input = 0
    let cached = 0
    let output = 0
    let hasAny = false
    for (const message of messages) {
      const usage = message.usage
      if (!usage) continue
      const providerId = message.provider_id ?? message.providerId
      const apiFormat = (providerId && apiFormats[providerId]) || defaultApiFormat
      const rawInput = usage.input_tokens ?? usage.inputTokens
      const rawOutput = usage.output_tokens ?? usage.outputTokens
      const rawCached = usage.cached_input_tokens ?? usage.cachedInputTokens
      if (rawInput != null) {
        // Anthropic / 外部 CLI（dsh 等）的 input 已排除缓存；OpenAI 系 input 含缓存，减掉后同口径。
        const included = cacheIncludedInInput
          ?? (apiFormat !== 'anthropic_messages')
        input += included ? Math.max(0, rawInput - (rawCached ?? 0)) : rawInput
        hasAny = true
      }
      if (rawOutput != null) {
        output += rawOutput
        hasAny = true
      }
      if (rawCached != null) {
        cached += rawCached
        hasAny = true
      }
    }
    return { input, cached, output, hasAny }
  }, [apiFormats, cacheIncludedInInput, defaultApiFormat, messages])

  if (!totals.hasAny) return null

  const t = i18n[lang]
  const cacheLabel = totals.cached > 0
    ? `${t.chatUsageCacheShort} ${formatCachePercent(totals.cached, totals.input)}`
    : null
  // 悬浮提示给全量描述（箭头太紧凑，命中率的分母语义放这里说明）。
  const tooltip = [
    `${t.chatUsageInput} ${formatTokensK(totals.input)}`,
    ...(cacheLabel ? [`${t.chatUsageCache} ${formatCachePercent(totals.cached, totals.input)}`] : []),
    `${t.chatUsageOutput} ${formatTokensK(totals.output)}`,
  ].join(' · ')

  return (
    <span
      className="chat-session-usage flex shrink-0 items-center gap-1.5 whitespace-nowrap text-[11px] leading-none text-neutral-400 dark:text-neutral-500"
      title={tooltip}
      data-tauri-drag-region="false"
    >
      {cacheLabel && <span>{cacheLabel}</span>}
      <span>↑{formatTokensK(totals.input)}</span>
      <span>↓{formatTokensK(totals.output)}</span>
    </span>
  )
}
