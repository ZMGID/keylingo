import { AlertTriangle, Ban, Gauge, MessageSquareOff, Scissors } from 'lucide-react'
import type { DegradedAnswer } from './types'

/**
 * 降级兜底卡片。
 *
 * 此前后端把「失败原因 + 已完成工具摘要」拼成一段纯文本塞进 assistant 正文，
 * 结果它长得和模型的真实回答一模一样：能被复制、会进对话历史再发回模型。
 * 现在正文归正文、故障归卡片（数据来自 `ChatMessage.degraded`）。
 */

const KIND_META: Record<
  string,
  { Icon: typeof AlertTriangle; label: string; tone: string }
> = {
  rate_limited: { Icon: Gauge, label: '限流 / 配额', tone: 'amber' },
  context_overflow: { Icon: Scissors, label: '上下文超长', tone: 'amber' },
  moderation: { Icon: Ban, label: '内容审核拦截', tone: 'red' },
  empty_response: { Icon: MessageSquareOff, label: '空响应', tone: 'amber' },
  unknown: { Icon: AlertTriangle, label: '调用失败', tone: 'red' },
}

const TONE_CLASS: Record<string, { border: string; icon: string; label: string }> = {
  amber: {
    border: 'border-amber-200/80 bg-amber-50/60 dark:border-amber-900/50 dark:bg-amber-950/25',
    icon: 'text-amber-600 dark:text-amber-400',
    label: 'text-amber-800 dark:text-amber-300',
  },
  red: {
    border: 'border-red-200/80 bg-red-50/60 dark:border-red-900/50 dark:bg-red-950/25',
    icon: 'text-red-600 dark:text-red-400',
    label: 'text-red-800 dark:text-red-300',
  },
}

export function DegradedAnswerCard({ degraded }: { degraded: DegradedAnswer }) {
  const meta = KIND_META[degraded.kind] ?? KIND_META.unknown
  const tone = TONE_CLASS[meta.tone] ?? TONE_CLASS.red
  const { Icon } = meta
  const summaries = degraded.toolSummaries ?? []

  return (
    <div
      className={`my-2 rounded-xl border px-3.5 py-3 ${tone.border}`}
      role="status"
      data-degraded-kind={degraded.kind}
    >
      <div className="flex items-start gap-2.5">
        <Icon size={15} strokeWidth={2} className={`mt-0.5 shrink-0 ${tone.icon}`} />
        <div className="min-w-0 flex-1">
          <div className={`text-[13px] font-medium ${tone.label}`}>{meta.label}</div>
          <p className="mt-1 text-[12.5px] leading-relaxed text-neutral-600 dark:text-neutral-400">
            {degraded.reason}
          </p>

          {summaries.length > 0 && (
            <div className="mt-2.5 border-t border-black/[0.06] pt-2 dark:border-white/[0.08]">
              <div className="text-[11.5px] text-neutral-500 dark:text-neutral-500">
                本轮已完成 {summaries.length} 个工具调用，结果见上方卡片
              </div>
              <ul className="mt-1.5 space-y-1">
                {summaries.map((s, i) => (
                  <li
                    key={`${s.name}-${i}`}
                    className="flex min-w-0 gap-1.5 text-[12px] text-neutral-600 dark:text-neutral-400"
                  >
                    <span className="shrink-0 font-mono text-[11.5px] text-neutral-500 dark:text-neutral-500">
                      {s.name}
                    </span>
                    <span className="min-w-0 flex-1 truncate" title={s.preview}>
                      {s.preview}
                    </span>
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>
      </div>
    </div>
  )
}
