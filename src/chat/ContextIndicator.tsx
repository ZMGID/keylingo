import { Archive, RefreshCw, X } from 'lucide-react'
import { useEffect, useMemo, useRef, useState, type RefObject } from 'react'
import { createPortal } from 'react-dom'
import { usePopoverMaxHeight } from './usePopoverMaxHeight'
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
  anchorRef?: RefObject<HTMLDivElement | null>
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

function windowLabel(contextWindowTokens: number | null, t: I18n): string {
  if (!contextWindowTokens) return t.contextTokensUnknown
  return `${formatTokens(contextWindowTokens).replace('k', 'K')} ${t.contextTokens}`
}

function messageCountLabel(messageCount: number, compressedMessageCount: number, t: I18n): string {
  if (compressedMessageCount > 0) {
    return t.contextMessagesCompressed
      .replace('{count}', String(messageCount))
      .replace('{compressed}', String(compressedMessageCount))
  }
  return t.contextMessages.replace('{count}', String(messageCount))
}

function panelHeading(t: I18n, isExternalContext: boolean, compressionCount: number): string {
  const countLabel = t.contextCompressionCount.replace('{count}', String(compressionCount))
  if (isExternalContext) {
    return compressionCount > 0
      ? `${t.contextPanelTitle} · ${countLabel}`
      : t.contextPanelTitle
  }
  const auto = t.contextPanelAutoCompress.replace('{auto}', String(CONTEXT_AUTO_COMPRESS_PERCENT))
  return `${t.contextPanelTitle} · ${auto} · ${countLabel}`
}

const THRESHOLD_MARKERS = [
  { percent: CONTEXT_WARNING_PERCENT, color: '#B7791F' },
  { percent: CONTEXT_AUTO_COMPRESS_PERCENT, color: '#C56646' },
  { percent: CONTEXT_CRITICAL_PERCENT, color: '#C24135' },
] as const

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
  placement = 'down',
  anchorRef,
  lang = 'zh',
}: ContextIndicatorProps) {
  const t = i18n[lang]
  const approximatePrefix = '~'
  const [open, setOpen] = useState(false)
  const triggerRef = useRef<HTMLDivElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)
  const maxH = usePopoverMaxHeight(open, popoverRef, placement === 'up' ? 'up' : 'down', 360)

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
  const tokenLine = `${formatTokenTotal(estimatedInputTokens, isReportedExact, approximatePrefix)} / ${windowLabel(contextWindowTokens, t)}`
  const sourceLabel = isExternalContext
    ? (isCliReported ? t.contextSourceCliReported : t.contextSourceCliEstimated)
    : (isProviderReported ? t.contextSourceProviderReported : t.contextSourceKivio)
  const ringDegrees = usageRatio == null ? 0 : Math.max(0, Math.min(1, usageRatio)) * 360
  const canCompress = Boolean(onCompress) && !compressing && !loading && messageCount > 2
  const compressLabel = isExternalContext
    ? (compressing ? t.contextCliCompacting : t.contextCliCompact)
    : (compressing ? t.contextCompressing : t.contextCompress)
  const showThresholdMarkers = (contextWindowTokens ?? 0) > 0

  return (
    <div className="relative" ref={triggerRef} data-tauri-drag-region="false">
      <button
        type="button"
        className="grid size-7 shrink-0 place-items-center rounded-full text-neutral-600 transition-colors hover:bg-neutral-100 active:scale-[0.97] dark:text-neutral-300 dark:hover:bg-neutral-800"
        aria-label={t.contextTriggerAria}
        title={loading
          ? t.contextTriggerLoading
          : [
            fullness,
            tokenLine,
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

      {open && anchorRef?.current && createPortal(
        <div
          ref={popoverRef}
          className={`chat-motion-popover absolute inset-x-0 z-40 flex flex-col overflow-hidden rounded-xl border border-neutral-200/90 bg-white p-3 shadow-[0_12px_32px_-10px_rgba(0,0,0,0.16)] dark:border-neutral-700/90 dark:bg-neutral-900 ${placement === 'up' ? 'bottom-full mb-1.5' : 'top-full mt-1.5'}`}
          style={{ ['--chat-popover-origin' as string]: placement === 'up' ? 'bottom right' : 'top right', maxHeight: maxH }}
          data-tauri-drag-region="false"
        >
          <div className="mb-2 flex items-baseline justify-between gap-2">
            <h3 className="shrink-0 text-[12px] font-medium leading-snug text-neutral-700 dark:text-neutral-300">
              {panelHeading(t, isExternalContext, compressionCount)}
            </h3>
            <div className="flex min-w-0 items-baseline gap-2">
              <span className="min-w-0 truncate text-[11px] leading-none tabular-nums text-neutral-500 dark:text-neutral-400">
                {tokenLine} · {fullness}
              </span>
              <button
                type="button"
                className="-mr-0.5 shrink-0 rounded-md p-1 text-neutral-400 transition-colors hover:bg-neutral-100 hover:text-neutral-600 dark:hover:bg-neutral-800 dark:hover:text-neutral-200"
                aria-label={t.contextCloseAria}
                onClick={() => setOpen(false)}
              >
                <X size={14} strokeWidth={2} />
              </button>
            </div>
          </div>

          <div className="relative mb-2">
            <div className="flex h-2 overflow-hidden rounded-full bg-neutral-100 dark:bg-neutral-800">
              {barSlices.length === 0 ? (
                <div className="h-full w-full bg-neutral-200 dark:bg-neutral-700" />
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
            {showThresholdMarkers && !isExternalContext && (
              <div className="pointer-events-none absolute inset-0">
                {THRESHOLD_MARKERS.map((marker) => (
                  <span
                    key={marker.percent}
                    className="absolute top-0 bottom-0 w-px opacity-50"
                    style={{ left: `${marker.percent}%`, backgroundColor: marker.color }}
                  />
                ))}
              </div>
            )}
          </div>

          {legendSlices.length > 0 && (
            <div className="chat-popover-scroll min-h-0 max-h-36 space-y-0 overflow-y-auto pr-2 [scrollbar-gutter:stable]">
              {legendSlices.map((slice) => (
                <div
                  key={`row-${slice.id}`}
                  className="flex items-center gap-2 py-[3px] pr-0.5 text-[11px] leading-none"
                >
                  <span
                    className="size-[9px] shrink-0 rounded-[2px]"
                    style={{ backgroundColor: slice.color }}
                  />
                  <span className="min-w-0 flex-1 truncate text-neutral-600 dark:text-neutral-300">
                    {slice.label}
                  </span>
                  <span className="shrink-0 tabular-nums text-neutral-500 dark:text-neutral-400">
                    {formatTokenTotal(slice.tokens, isCliReported, approximatePrefix)}
                  </span>
                </div>
              ))}
            </div>
          )}

          {error && (
            <p className="mt-1.5 text-[10px] text-[#C24135] dark:text-[#F08A80]">
              {error}
            </p>
          )}

          <div className="mt-2 flex justify-end gap-1.5">
            <button
              type="button"
              className="inline-flex h-7 items-center gap-1 rounded-md px-2 text-[11px] font-medium text-neutral-600 transition-colors hover:bg-neutral-100 disabled:opacity-50 dark:text-neutral-300 dark:hover:bg-neutral-800"
              aria-label={t.contextRefreshAria}
              onClick={onRefresh}
              disabled={loading}
            >
              <RefreshCw size={12} className={loading ? 'animate-spin' : ''} />
              {t.contextRefresh}
            </button>
            <button
              type="button"
              className="inline-flex h-7 items-center gap-1 rounded-md bg-neutral-900 px-2.5 text-[11px] font-medium text-white transition-colors hover:bg-neutral-700 disabled:cursor-not-allowed disabled:opacity-40 dark:bg-neutral-100 dark:text-neutral-900 dark:hover:bg-neutral-200"
              aria-label={t.contextCompressAria}
              onClick={onCompress}
              disabled={!canCompress}
            >
              <Archive size={12} />
              {compressLabel}
            </button>
          </div>
        </div>,
        anchorRef.current,
      )}
    </div>
  )
}
