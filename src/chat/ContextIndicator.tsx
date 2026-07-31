import { Archive, RefreshCw } from 'lucide-react'
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import {
  buildContextBarSlices,
  CONTEXT_AUTO_COMPRESS_PERCENT,
  CONTEXT_CRITICAL_PERCENT,
  CONTEXT_FREE_SEGMENT_ID,
  CONTEXT_WARNING_PERCENT,
  fullnessLabel,
  segmentTokens,
} from './contextPanel'
import { i18n, type I18n, type Lang } from '../settings/i18n'
import { formatTokens } from '../utils/tokens'
import type { ConversationContextState } from './types'

const PANEL_WIDTH = 240
const PANEL_GAP = 8
const VIEW_MARGIN = 8
// 弹层尽量贴底栏，限制高度，少盖住上方对话消息。
const PANEL_MAX_H = 220

interface ContextIndicatorProps {
  contextState?: ConversationContextState | null
  messageCount?: number
  loading?: boolean
  compressing?: boolean
  error?: string
  usesExternalRuntime?: boolean
  onRefresh?: () => void
  onCompress?: () => void
  placement?: 'up' | 'down'
  lang?: Lang
}

function valueFrom<T>(snake: T | undefined, camel: T | undefined, fallback: T): T {
  return snake ?? camel ?? fallback
}

function statusColor(status: string, ratio: number | null): string {
  if (status === 'stale') return '#A15C2F'
  if (status === 'compressed') return '#3E8B60'
  if (status === 'critical' || (ratio ?? 0) >= CONTEXT_CRITICAL_PERCENT / 100) return '#C24135'
  if (status === 'warning' || (ratio ?? 0) >= CONTEXT_WARNING_PERCENT / 100) return '#B7791F'
  return '#3E8B60'
}

function formatTokenTotal(tokens: number, exact = false, approximatePrefix = '~'): string {
  const formatted = formatTokens(tokens).replace('k', 'K')
  return exact ? formatted : `${approximatePrefix}${formatted}`
}

function windowLabel(contextWindowTokens: number | null): string {
  if (!contextWindowTokens) return '—'
  return formatTokens(contextWindowTokens).replace('k', 'K')
}

function messageCountLabel(messageCount: number, compressedMessageCount: number, t: I18n): string {
  if (compressedMessageCount > 0) {
    return t.contextMessagesCompressed
      .replace('{count}', String(messageCount))
      .replace('{compressed}', String(compressedMessageCount))
  }
  return t.contextMessages.replace('{count}', String(messageCount))
}

function freeSliceClassName(isDark: boolean): string {
  return isDark
    ? 'bg-neutral-700'
    : 'bg-neutral-200'
}

export function ContextIndicator({
  contextState,
  messageCount = 0,
  loading = false,
  compressing = false,
  error = '',
  usesExternalRuntime = false,
  onRefresh,
  onCompress,
  placement: _placement = 'down',
  lang = 'zh',
}: ContextIndicatorProps) {
  void _placement
  const t = i18n[lang]
  const approximatePrefix = '~'
  const [open, setOpen] = useState(false)
  // 用 bottom/right 锚定，避免量高不准时整块飘到对话消息中间。
  const [pos, setPos] = useState<{ bottom: number; right: number; maxH: number; width: number } | null>(null)
  const triggerRef = useRef<HTMLDivElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)

  const estimatedInputTokens = valueFrom(
    contextState?.estimated_input_tokens,
    contextState?.estimatedInputTokens,
    0,
  )
  const contextWindowTokens = valueFrom(
    contextState?.context_window_tokens,
    contextState?.contextWindowTokens,
    null,
  )
  const usageRatio = valueFrom(contextState?.usage_ratio, contextState?.usageRatio, null)
  const status = contextState?.status ?? 'unknown'
  const contextSource = valueFrom(contextState?.context_source, contextState?.contextSource, null)
  const tokenCountSource = valueFrom(
    contextState?.token_count_source,
    contextState?.tokenCountSource,
    null,
  )
  const isExternalContext =
    usesExternalRuntime || contextSource === 'external_cli'
  const isCliReported = tokenCountSource === 'cli_reported'
  // 内置路径把 provider 实报 usage 作锚点时的口径（对齐 CLI 的 cli_reported）：显示精确值、不带 `~`。
  const isProviderReported = tokenCountSource === 'provider_reported'
  const isReportedExact = isCliReported || isProviderReported
  const compressedMessageCount = valueFrom(
    contextState?.compressed_message_count,
    contextState?.compressedMessageCount,
    0,
  )
  const compressionCount = valueFrom(
    contextState?.compression_count,
    contextState?.compressionCount,
    0,
  )
  const color = statusColor(status, usageRatio)
  const rawSegments = useMemo(
    () => (contextState?.segments ?? []).filter((segment) => segmentTokens(segment) > 0),
    [contextState?.segments],
  )
  const barSlices = useMemo(
    () => buildContextBarSlices(rawSegments, estimatedInputTokens, contextWindowTokens, t),
    [contextWindowTokens, estimatedInputTokens, rawSegments, t],
  )
  const legendSlices = useMemo(
    () =>
      barSlices
        .filter((slice) => slice.id !== CONTEXT_FREE_SEGMENT_ID)
        // 对话消息固定排在图例首位
        .sort((a, b) => Number(b.id === 'conversation') - Number(a.id === 'conversation')),
    [barSlices],
  )
  const fullness = fullnessLabel(usageRatio, isExternalContext, contextWindowTokens, t)
  const usedLabel = formatTokenTotal(estimatedInputTokens, isReportedExact, approximatePrefix)
  const windowPart = windowLabel(contextWindowTokens)
  // 有比例 → "42% · ~1.2K / 128K"；有 token 无比例 → "~0 / —"；全空 → "—"
  const displayMetric = usageRatio != null
    ? `${Math.round(Math.max(0, Math.min(1, usageRatio)) * 100)}% · ${usedLabel} / ${windowPart}`
    : (estimatedInputTokens > 0 || contextWindowTokens)
      ? `${usedLabel} / ${windowPart}`
      : '—'
  const sourceLabel = isExternalContext
    ? (isCliReported ? t.contextSourceCliReported : t.contextSourceCliEstimated)
    : (isProviderReported ? t.contextSourceProviderReported : t.contextSourceKivio)
  const ringDegrees = usageRatio == null ? 0 : Math.max(0, Math.min(1, usageRatio)) * 360
  const canCompress = Boolean(onCompress) && !compressing && !loading && messageCount > 2
  const compressLabel = isExternalContext
    ? (compressing ? t.contextCliCompacting : t.contextCliCompact)
    : (compressing ? t.contextCompressing : t.contextCompress)
  const autoHint = isExternalContext
    ? null
    : t.contextPanelAutoCompress.replace('{auto}', String(CONTEXT_AUTO_COMPRESS_PERCENT))
  // 只在真正压过时露出次数；自动压缩阈值放压缩按钮 title，不占正文。
  const compressMeta = compressionCount > 0
    ? t.contextCompressionCount.replace('{count}', String(compressionCount))
    : null

  // 锚定在圆环旁：优先左侧 + 底边与按钮对齐（贴底栏，少盖对话消息）；
  // 左侧不够再翻到按钮正上方。用 bottom/right，不依赖量高。
  const place = useCallback(() => {
    const trigger = triggerRef.current
    if (!trigger) return
    const rect = trigger.getBoundingClientRect()
    const width = Math.min(PANEL_WIDTH, window.innerWidth - VIEW_MARGIN * 2)
    const spaceLeft = rect.left - VIEW_MARGIN
    const spaceAbove = rect.top - VIEW_MARGIN - PANEL_GAP
    const openLeft = spaceLeft >= width + PANEL_GAP

    if (openLeft) {
      // 底边对齐圆环底 → 弹层坐在底栏高度带，只向上长一截
      const bottom = Math.max(VIEW_MARGIN, window.innerHeight - rect.bottom)
      const right = window.innerWidth - rect.left + PANEL_GAP
      const maxH = Math.max(96, Math.min(PANEL_MAX_H, window.innerHeight - bottom - VIEW_MARGIN))
      setPos({ bottom, right, maxH, width })
      return
    }

    // 回退：贴在圆环正上方，右缘对齐
    const bottom = window.innerHeight - rect.top + PANEL_GAP
    const right = Math.max(VIEW_MARGIN, window.innerWidth - rect.right)
    const maxH = Math.max(96, Math.min(PANEL_MAX_H, spaceAbove))
    setPos({ bottom, right, maxH, width })
  }, [])

  useLayoutEffect(() => {
    if (!open) {
      setPos(null)
      return
    }
    place()
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    return () => {
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place, true)
    }
  }, [open, place, legendSlices.length, displayMetric, compressMeta, error])

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      const target = e.target as Node
      if (triggerRef.current?.contains(target) || popoverRef.current?.contains(target)) return
      setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [open])

  const panel = open
    ? createPortal(
        <div
          ref={popoverRef}
          className="chat-motion-popover fixed z-[200] flex flex-col overflow-hidden kv-menu p-2"
          style={{
            bottom: pos?.bottom ?? 0,
            right: pos?.right ?? 0,
            width: pos?.width ?? PANEL_WIDTH,
            maxHeight: pos?.maxH,
            visibility: pos ? 'visible' : 'hidden',
            ['--chat-popover-origin' as string]: 'bottom right',
          }}
          data-tauri-drag-region="false"
        >
          <div className="mb-1.5 flex items-center gap-1">
            <div className="min-w-0 flex-1">
              <div className="truncate text-[12px] font-semibold text-neutral-800 dark:text-neutral-100">
                {t.contextPanelTitle}
              </div>
              <div className="mt-0.5 truncate text-[11px] tabular-nums leading-none text-neutral-500 dark:text-neutral-400">
                {displayMetric}
              </div>
            </div>
            <button
              type="button"
              className="grid size-7 shrink-0 place-items-center rounded-md text-neutral-500 transition-colors hover:bg-neutral-100 hover:text-neutral-800 disabled:opacity-40 dark:text-neutral-400 dark:hover:bg-neutral-800 dark:hover:text-neutral-100"
              aria-label={t.contextRefreshAria}
              title={t.contextRefresh}
              onClick={onRefresh}
              disabled={loading}
            >
              <RefreshCw size={13} strokeWidth={1.9} className={loading ? 'animate-spin' : ''} />
            </button>
            <button
              type="button"
              className="inline-flex h-7 shrink-0 items-center gap-1 rounded-md px-1.5 text-[11px] font-semibold text-neutral-700 transition-colors hover:bg-neutral-100 disabled:cursor-not-allowed disabled:opacity-40 dark:text-neutral-200 dark:hover:bg-neutral-800"
              aria-label={t.contextCompressAria}
              title={autoHint ? `${compressLabel} · ${autoHint}` : compressLabel}
              onClick={onCompress}
              disabled={!canCompress}
            >
              <Archive size={13} strokeWidth={1.9} />
              <span>{compressLabel}</span>
            </button>
          </div>

          <div className="relative mb-1">
            <div className="flex h-1.5 overflow-hidden rounded-full bg-neutral-100 dark:bg-neutral-800">
              {barSlices.length === 0 ? (
                <div className="h-full w-full bg-neutral-200/80 dark:bg-neutral-700" />
              ) : (
                barSlices.map((slice) => (
                  <div
                    key={slice.id}
                    className={`h-full min-w-[1px] ${slice.id === CONTEXT_FREE_SEGMENT_ID ? freeSliceClassName(document.documentElement.classList.contains('dark')) : ''}`}
                    style={{
                      width: `${slice.widthPercent}%`,
                      backgroundColor: slice.id === CONTEXT_FREE_SEGMENT_ID ? undefined : slice.color,
                    }}
                    title={`${slice.label} · ${formatTokenTotal(slice.tokens, isCliReported, approximatePrefix)}`}
                  />
                ))
              )}
            </div>
          </div>

          {legendSlices.length > 0 && (
            <div className="chat-popover-scroll min-h-0 max-h-32 space-y-0 overflow-y-auto">
              {legendSlices.map((slice) => (
                <div
                  key={`row-${slice.id}`}
                  className="flex items-center gap-1.5 py-[2px] text-[11px] leading-none"
                >
                  <span
                    className="size-1.5 shrink-0 rounded-full"
                    style={{ backgroundColor: slice.color }}
                  />
                  <span className="min-w-0 flex-1 truncate text-neutral-600 dark:text-neutral-300">
                    {slice.label}
                  </span>
                  <span className="shrink-0 tabular-nums text-neutral-400 dark:text-neutral-500">
                    {formatTokenTotal(slice.tokens, isCliReported, approximatePrefix)}
                  </span>
                </div>
              ))}
            </div>
          )}

          {compressMeta && (
            <div className="mt-1 truncate text-[10px] text-neutral-400 dark:text-neutral-500">
              {compressMeta}
            </div>
          )}

          {error && (
            <p className="mt-1 text-[10px] text-[#C24135] dark:text-[#F08A80]">
              {error}
            </p>
          )}
        </div>,
        document.body,
      )
    : null

  return (
    <div className="relative" ref={triggerRef} data-tauri-drag-region="false">
      <button
        type="button"
        className="grid size-7 shrink-0 place-items-center rounded-full text-neutral-600 transition-colors hover:bg-neutral-100 active:scale-[0.97] dark:text-neutral-300 dark:hover:bg-neutral-800"
        aria-label={t.contextTriggerAria}
        title={loading
          ? t.contextTriggerLoading
          : [
            displayMetric !== '—' ? displayMetric : fullness,
            sourceLabel,
            messageCount > 0 ? messageCountLabel(messageCount, compressedMessageCount, t) : '',
          ].filter(Boolean).join(' · ')}
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <span
          className="grid size-5 place-items-center rounded-full"
          style={{
            background: `conic-gradient(${color} ${ringDegrees}deg, rgba(120,120,120,.22) 0deg)`,
          }}
        >
          <span className="size-3 rounded-full bg-white dark:bg-[#212121]" />
        </span>
      </button>
      {panel}
    </div>
  )
}
