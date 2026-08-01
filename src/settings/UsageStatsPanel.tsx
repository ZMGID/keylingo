import { useCallback, useEffect, useMemo, useState, type MouseEvent as ReactMouseEvent } from 'react'
import { ChevronLeft, ChevronRight, RefreshCw, Trash2 } from 'lucide-react'
import {
  api,
  type UsageGroupStats,
  type UsageRange,
  type UsageRecord,
  type UsageStatsResponse,
  type UsageTrendPoint,
} from '../api/tauri'
import { Button } from '../components/Button'
import { Input, Select, SettingsGroup } from './components'

type UsageView = 'logs' | 'providers' | 'models'

type UsageStatsPanelProps = {
  lang: string
}

const SOURCE_OPTIONS = [
  'all',
  'chat',
  'translator',
  'screenshot_translation',
  'lens',
  'chat_title_summary',
  'chat_compression',
  'chat_aux_vision',
  'chat_image_generation',
  'knowledge_base',
]

const STATUS_OPTIONS = ['all', 'success', 'error', 'cancelled', 'missing_usage']
const LOG_PAGE_SIZE = 15
const SEARCH_DEBOUNCE_MS = 250

function sourceLabel(source: string, lang: string) {
  const zh: Record<string, string> = {
    all: '全部来源',
    chat: 'Chat',
    translator: '输入翻译',
    screenshot_translation: '快速翻译',
    lens: 'Lens',
    chat_title_summary: '标题总结',
    chat_compression: '上下文压缩',
    chat_aux_vision: '辅助视觉',
    chat_image_generation: '图片生成',
    knowledge_base: '知识库',
  }
  const en: Record<string, string> = {
    all: 'All sources',
    chat: 'Chat',
    translator: 'Input translation',
    screenshot_translation: 'Quick translation',
    lens: 'Lens',
    chat_title_summary: 'Title summary',
    chat_compression: 'Context compression',
    chat_aux_vision: 'Aux vision',
    chat_image_generation: 'Image generation',
    knowledge_base: 'Knowledge base',
  }
  return (lang === 'zh' ? zh : en)[source] || source.replace(/_/g, ' ')
}

function statusLabel(status: string, lang: string) {
  const zh: Record<string, string> = {
    all: '全部状态',
    success: '成功',
    error: '失败',
    cancelled: '取消',
    missing_usage: '无 usage',
  }
  const en: Record<string, string> = {
    all: 'All statuses',
    success: 'Success',
    error: 'Error',
    cancelled: 'Cancelled',
    missing_usage: 'No usage',
  }
  return (lang === 'zh' ? zh : en)[status] || status
}

function formatCount(value?: number | null) {
  if (!value || !Number.isFinite(value)) return '0'
  return Math.round(value).toLocaleString()
}

function formatTokens(value?: number | null) {
  const n = Number(value ?? 0)
  if (!Number.isFinite(n) || n <= 0) return '0'
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 1 : 2)}M`
  if (n >= 10_000) return `${(n / 1_000).toFixed(1)}K`
  return Math.round(n).toLocaleString()
}

function formatCost(value?: number | null) {
  const n = Number(value ?? 0)
  if (!Number.isFinite(n) || n <= 0) return '$0.00'
  if (n < 0.01) return `$${n.toFixed(4)}`
  return `$${n.toFixed(2)}`
}

function formatDuration(ms?: number | null) {
  const n = Number(ms ?? 0)
  if (!Number.isFinite(n) || n <= 0) return '--'
  if (n >= 1000) return `${(n / 1000).toFixed(1)}s`
  return `${Math.round(n)}ms`
}

function formatTime(seconds?: number | null, lang = 'zh') {
  if (!seconds) return '--'
  return new Date(seconds * 1000).toLocaleString(lang === 'zh' ? 'zh-CN' : 'en-US', {
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  })
}

function pageRangeLabel(pageIndex: number, pageSize: number, total: number) {
  if (total <= 0) return '0 / 0'
  const start = pageIndex * pageSize + 1
  const end = Math.min(total, start + pageSize - 1)
  return `${start}-${end} / ${total}`
}

function recordTotalTokens(record: UsageRecord) {
  return record.totalTokens ?? ((record.inputTokens ?? 0) + (record.outputTokens ?? 0))
}

function SummaryTile({ label, value, sub }: { label: string; value: string; sub?: string }) {
  return (
    <div className="rounded-md border border-[var(--border)] bg-[var(--bg-input)] px-3 py-2.5">
      <div className="text-[11px] font-medium text-neutral-500 dark:text-neutral-400">{label}</div>
      <div className="mt-1 truncate text-[19px] font-semibold leading-6 text-neutral-950 dark:text-neutral-50">{value}</div>
      {sub && <div className="mt-1 truncate text-[10.5px] text-neutral-500 dark:text-neutral-500">{sub}</div>}
    </div>
  )
}

function formatPercent(value?: number | null) {
  const n = Number(value ?? 0)
  if (!Number.isFinite(n) || n <= 0) return '0%'
  return `${Math.round(n * 100)}%`
}

type TrendSeriesKey = 'inputTokens' | 'outputTokens' | 'cacheCreationInputTokens' | 'cachedInputTokens'

const TREND_SERIES: {
  key: TrendSeriesKey
  labelZh: string
  labelEn: string
  stroke: string
  darkStroke: string
}[] = [
  { key: 'inputTokens', labelZh: '输入', labelEn: 'Input', stroke: '#2a78d6', darkStroke: '#3987e5' },
  { key: 'outputTokens', labelZh: '输出', labelEn: 'Output', stroke: '#1baf7a', darkStroke: '#34d399' },
  { key: 'cacheCreationInputTokens', labelZh: '缓存创建', labelEn: 'Cache creation', stroke: '#eda100', darkStroke: '#fbbf24' },
  { key: 'cachedInputTokens', labelZh: '缓存命中', labelEn: 'Cache read', stroke: '#0891b2', darkStroke: '#22d3ee' },
]

const HIT_RATE_COLOR = { stroke: '#7c3aed', darkStroke: '#a78bfa' }

/// 环形图分类色(dataviz 参考色板,固定顺序不循环;第 7 片起折叠为「其他」灰)。
const PIE_COLORS: { light: string; dark: string }[] = [
  { light: '#2a78d6', dark: '#3987e5' },
  { light: '#1baf7a', dark: '#34d399' },
  { light: '#eda100', dark: '#fbbf24' },
  { light: '#4a3aa7', dark: '#9085e9' },
  { light: '#e34948', dark: '#e66767' },
  { light: '#0891b2', dark: '#22d3ee' },
]
const PIE_OTHER_COLOR = { light: '#a8a29e', dark: '#78716c' }
const PIE_MAX_SLICES = 6

function trendHitRate(point: UsageTrendPoint): number | null {
  if (point.inputTokens <= 0) return null
  return Math.min(1, point.cachedInputTokens / point.inputTokens)
}

/// 单调三次插值(Fritsch-Carlson)平滑折线:曲线严格保持在相邻数据点范围内,
/// 永不过冲——尖峰旁的零值段不会被拉出负凹(Catmull-Rom 会,已踩过)。
function smoothPath(coords: { x: number; y: number }[]): string {
  const n = coords.length
  if (n === 0) return ''
  if (n === 1) return `M ${coords[0].x.toFixed(1)} ${coords[0].y.toFixed(1)}`
  // 相邻段斜率
  const dx: number[] = []
  const slope: number[] = []
  for (let i = 0; i < n - 1; i++) {
    dx.push(coords[i + 1].x - coords[i].x)
    slope.push(dx[i] !== 0 ? (coords[i + 1].y - coords[i].y) / dx[i] : 0)
  }
  // 每个点的切线:相邻段斜率异号或有零 → 0(平台/极值处走平),否则调和平均
  const tangent: number[] = [slope[0]]
  for (let i = 1; i < n - 1; i++) {
    const a = slope[i - 1]
    const b = slope[i]
    tangent.push(a * b <= 0 ? 0 : (2 * a * b) / (a + b))
  }
  tangent.push(slope[n - 2])
  let path = `M ${coords[0].x.toFixed(1)} ${coords[0].y.toFixed(1)}`
  for (let i = 0; i < n - 1; i++) {
    const h = dx[i] / 3
    const c1x = coords[i].x + h
    const c1y = coords[i].y + tangent[i] * h
    const c2x = coords[i + 1].x - h
    const c2y = coords[i + 1].y - tangent[i + 1] * h
    path += ` C ${c1x.toFixed(1)} ${c1y.toFixed(1)}, ${c2x.toFixed(1)} ${c2y.toFixed(1)}, ${coords[i + 1].x.toFixed(1)} ${coords[i + 1].y.toFixed(1)}`
  }
  return path
}

function TrendChart({ points, lang }: { points: UsageTrendPoint[]; lang: string }) {
  const [hidden, setHidden] = useState<Set<string>>(() => new Set())
  const [hoverIndex, setHoverIndex] = useState<number | null>(null)
  const isDark = typeof document !== 'undefined' && document.documentElement.classList.contains('dark')

  const WIDTH = 640
  const HEIGHT = 180
  const PAD_L = 44
  const PAD_R = 40
  const PAD_T = 10
  const PAD_B = 20

  const geom = useMemo(() => {
    const visible = TREND_SERIES.filter(series => !hidden.has(series.key))
    const maxTokens = Math.max(1, ...points.flatMap(point => visible.map(series => point[series.key])))
    const step = points.length > 1 ? (WIDTH - PAD_L - PAD_R) / (points.length - 1) : 0
    const plotH = HEIGHT - PAD_T - PAD_B
    // 单点(如单日区间)居中,否则从左轴按步长铺开
    const singleX = PAD_L + (WIDTH - PAD_L - PAD_R) / 2
    const x = (index: number) => (points.length > 1 ? PAD_L + step * index : singleX)
    const yTokens = (value: number) => PAD_T + plotH - (value / maxTokens) * plotH
    const yRate = (rate: number) => PAD_T + plotH - rate * plotH
    const linePath = (values: (number | null)[]) => {
      const coords = values.flatMap((value, index) =>
        value == null ? [] : [{ x: x(index), y: yTokens(value) }],
      )
      return smoothPath(coords)
    }
    const seriesPaths = visible.map(series => ({
      ...series,
      path: linePath(points.map(point => point[series.key])),
    }))
    const rateCoords = points.flatMap((point, index) => {
      const rate = trendHitRate(point)
      return rate == null ? [] : [{ x: x(index), y: yRate(rate) }]
    })
    return { maxTokens, step, x, yTokens, yRate, seriesPaths, ratePath: smoothPath(rateCoords), plotH }
  }, [hidden, points])

  const toggleSeries = useCallback((key: string) => {
    setHidden(previous => {
      const next = new Set(previous)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }, [])

  const onMove = useCallback(
    (event: ReactMouseEvent<SVGSVGElement>) => {
      if (points.length === 0) return
      const rect = event.currentTarget.getBoundingClientRect()
      const px = ((event.clientX - rect.left) / rect.width) * WIDTH
      const index = geom.step > 0 ? Math.round((px - PAD_L) / geom.step) : 0
      setHoverIndex(Math.max(0, Math.min(points.length - 1, index)))
    },
    [geom.step, points.length],
  )

  if (points.length === 0) {
    return (
      <div className="flex h-36 items-center justify-center rounded-md border border-dashed border-[var(--border)] text-[12px] text-[var(--text-muted)]">
        {lang === 'zh' ? '暂无趋势数据' : 'No trend data'}
      </div>
    )
  }

  const hoverPoint = hoverIndex != null ? points[hoverIndex] : null
  const hoverRate = hoverPoint ? trendHitRate(hoverPoint) : null
  const rateHidden = hidden.has('hitRate')
  const gridYs = [0, 0.5, 1].map(fraction => PAD_T + geom.plotH - fraction * geom.plotH)
  // tooltip 靠左半边时显示在指针右侧，反之左侧，避免出界。
  const tooltipLeftPct = hoverIndex != null ? (geom.x(hoverIndex) / WIDTH) * 100 : 0
  const tooltipFlip = tooltipLeftPct > 55

  return (
    <div>
      <div className="mb-2 flex flex-wrap items-center justify-center gap-2">
        {TREND_SERIES.map(series => (
          <button
            key={series.key}
            type="button"
            onClick={() => toggleSeries(series.key)}
            data-tauri-drag-region="false"
            className={`inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-[11px] transition-opacity ${
              hidden.has(series.key)
                ? 'border-[var(--border)] text-[var(--text-faint)] opacity-55'
                : 'border-[var(--border)] text-[var(--text-muted)]'
            }`}
          >
            <span
              className="h-2 w-2 rounded-full"
              style={{ backgroundColor: isDark ? series.darkStroke : series.stroke }}
            />
            {lang === 'zh' ? series.labelZh : series.labelEn}
          </button>
        ))}
        <button
          type="button"
          onClick={() => toggleSeries('hitRate')}
          data-tauri-drag-region="false"
          className={`inline-flex items-center gap-1.5 rounded-full border px-2 py-0.5 text-[11px] transition-opacity ${
            rateHidden
              ? 'border-[var(--border)] text-[var(--text-faint)] opacity-55'
              : 'border-[var(--border)] text-[var(--text-muted)]'
          }`}
        >
          <span
            className="h-0.5 w-3 rounded-full"
            style={{
              backgroundImage: `repeating-linear-gradient(90deg, ${isDark ? HIT_RATE_COLOR.darkStroke : HIT_RATE_COLOR.stroke} 0 3px, transparent 3px 5px)`,
            }}
          />
          {lang === 'zh' ? '缓存命中率' : 'Cache hit rate'}
        </button>
      </div>
      <div className="relative">
        <svg
          viewBox={`0 0 ${WIDTH} ${HEIGHT}`}
          className="h-48 w-full overflow-visible"
          role="img"
          aria-label="token usage trend"
          onMouseMove={onMove}
          onMouseLeave={() => setHoverIndex(null)}
        >
          {gridYs.map(y => (
            <line
              key={y}
              x1={PAD_L}
              y1={y}
              x2={WIDTH - PAD_R}
              y2={y}
              stroke="currentColor"
              className="text-neutral-200 dark:text-neutral-800"
              strokeWidth="1"
            />
          ))}
          {/* 左轴 token 刻度 */}
          {[0, 0.5, 1].map(fraction => (
            <text
              key={`l-${fraction}`}
              x={PAD_L - 6}
              y={PAD_T + geom.plotH - fraction * geom.plotH + 3.5}
              textAnchor="end"
              className="fill-neutral-500 text-[10px] tabular-nums dark:fill-neutral-500"
            >
              {formatTokens(geom.maxTokens * fraction)}
            </text>
          ))}
          {/* 右轴命中率刻度 */}
          {!rateHidden &&
            [0, 0.5, 1].map(fraction => (
              <text
                key={`r-${fraction}`}
                x={WIDTH - PAD_R + 6}
                y={PAD_T + geom.plotH - fraction * geom.plotH + 3.5}
                textAnchor="start"
                className="text-[10px] tabular-nums"
                style={{ fill: isDark ? HIT_RATE_COLOR.darkStroke : HIT_RATE_COLOR.stroke }}
              >
                {Math.round(fraction * 100)}%
              </text>
            ))}
          {geom.seriesPaths.map(series =>
            series.path ? (
              <path
                key={series.key}
                d={series.path}
                fill="none"
                stroke={isDark ? series.darkStroke : series.stroke}
                strokeWidth="2"
                strokeLinecap="round"
                strokeLinejoin="round"
              />
            ) : null,
          )}
          {/* 单点时线段不可见,补画常驻圆点 */}
          {points.length === 1 &&
            geom.seriesPaths.map(series => (
              <circle
                key={`single-${series.key}`}
                cx={geom.x(0)}
                cy={geom.yTokens(points[0][series.key])}
                r="3.5"
                fill={isDark ? series.darkStroke : series.stroke}
              />
            ))}
          {!rateHidden && geom.ratePath && (
            <path
              d={geom.ratePath}
              fill="none"
              stroke={isDark ? HIT_RATE_COLOR.darkStroke : HIT_RATE_COLOR.stroke}
              strokeWidth="2"
              strokeDasharray="5 4"
              strokeLinecap="round"
            />
          )}
          {hoverIndex != null && (
            <line
              x1={geom.x(hoverIndex)}
              y1={PAD_T}
              x2={geom.x(hoverIndex)}
              y2={PAD_T + geom.plotH}
              stroke="currentColor"
              className="text-neutral-300 dark:text-neutral-700"
              strokeWidth="1"
            />
          )}
          {hoverIndex != null &&
            geom.seriesPaths.map(series => (
              <circle
                key={`dot-${series.key}`}
                cx={geom.x(hoverIndex)}
                cy={geom.yTokens(points[hoverIndex][series.key])}
                r="3"
                fill={isDark ? series.darkStroke : series.stroke}
                stroke={isDark ? '#0a0a0a' : '#ffffff'}
                strokeWidth="1.5"
              />
            ))}
        </svg>
        {hoverPoint && (
          <div
            className="pointer-events-none absolute top-1 z-10 min-w-36 rounded-md border border-[var(--border)] bg-[var(--bg-input)] px-2.5 py-2 text-[11px] shadow-sm"
            style={tooltipFlip ? { right: `${100 - tooltipLeftPct + 2}%` } : { left: `${tooltipLeftPct + 2}%` }}
          >
            <div className="mb-1 font-medium text-neutral-800 dark:text-neutral-100">
              {hoverPoint.label} · {formatCount(hoverPoint.requests)} {lang === 'zh' ? '次' : 'req'}
            </div>
            {TREND_SERIES.map(series => (
              <div key={series.key} className="flex items-center justify-between gap-3 text-neutral-600 dark:text-neutral-300">
                <span className="inline-flex items-center gap-1.5">
                  <span className="h-1.5 w-1.5 rounded-full" style={{ backgroundColor: isDark ? series.darkStroke : series.stroke }} />
                  {lang === 'zh' ? series.labelZh : series.labelEn}
                </span>
                <span className="tabular-nums">{formatTokens(hoverPoint[series.key])}</span>
              </div>
            ))}
            <div className="flex items-center justify-between gap-3 text-neutral-600 dark:text-neutral-300">
              <span>{lang === 'zh' ? '命中率' : 'Hit rate'}</span>
              <span className="tabular-nums">{hoverRate == null ? '--' : formatPercent(hoverRate)}</span>
            </div>
            <div className="mt-0.5 flex items-center justify-between gap-3 border-t border-[var(--divider)] pt-0.5 text-[var(--text-muted)]">
              <span>{lang === 'zh' ? '成本' : 'Cost'}</span>
              <span className="tabular-nums">{formatCost(hoverPoint.costUsd)}</span>
            </div>
          </div>
        )}
      </div>
      <div
        className={`mt-1 flex text-[10.5px] text-neutral-500 dark:text-neutral-500 ${points.length === 1 ? 'justify-center' : 'justify-between'}`}
        style={{ paddingLeft: PAD_L, paddingRight: PAD_R }}
      >
        <span>{points[0]?.label}</span>
        {points.length > 1 && <span>{points[points.length - 1]?.label}</span>}
      </div>
    </div>
  )
}

type PieSlice = {
  label: string
  sub?: string
  value: number
  requests: number
  cost: number
  color: string
}

/// modelStats → 环形图切片:按 token 降序取前 N,余下折叠为「其他」。
function buildPieSlices(rows: UsageGroupStats[], isDark: boolean, lang: string): PieSlice[] {
  const sorted = rows.filter(row => row.totalTokens > 0)
  if (sorted.length === 0) return []
  const head = sorted.slice(0, PIE_MAX_SLICES)
  const rest = sorted.slice(PIE_MAX_SLICES)
  const slices: PieSlice[] = head.map((row, index) => ({
    label: row.label,
    sub: row.providerName ?? undefined,
    value: row.totalTokens,
    requests: row.requestCount,
    cost: row.costUsd,
    color: isDark ? PIE_COLORS[index].dark : PIE_COLORS[index].light,
  }))
  if (rest.length > 0) {
    slices.push({
      label: lang === 'zh' ? `其他 (${rest.length})` : `Other (${rest.length})`,
      value: rest.reduce((sum, row) => sum + row.totalTokens, 0),
      requests: rest.reduce((sum, row) => sum + row.requestCount, 0),
      cost: rest.reduce((sum, row) => sum + row.costUsd, 0),
      color: isDark ? PIE_OTHER_COLOR.dark : PIE_OTHER_COLOR.light,
    })
  }
  return slices
}

function donutArcPath(cx: number, cy: number, rOuter: number, rInner: number, startAngle: number, endAngle: number): string {
  // 单片占比 100% 时画整圆环(arc 命令无法画满 360°)。
  const full = endAngle - startAngle >= Math.PI * 2 - 1e-4
  if (full) {
    return [
      `M ${cx} ${cy - rOuter}`,
      `A ${rOuter} ${rOuter} 0 1 1 ${cx} ${cy + rOuter}`,
      `A ${rOuter} ${rOuter} 0 1 1 ${cx} ${cy - rOuter}`,
      `M ${cx} ${cy - rInner}`,
      `A ${rInner} ${rInner} 0 1 0 ${cx} ${cy + rInner}`,
      `A ${rInner} ${rInner} 0 1 0 ${cx} ${cy - rInner}`,
      'Z',
    ].join(' ')
  }
  const p = (r: number, angle: number) => `${(cx + r * Math.sin(angle)).toFixed(2)} ${(cy - r * Math.cos(angle)).toFixed(2)}`
  const large = endAngle - startAngle > Math.PI ? 1 : 0
  return [
    `M ${p(rOuter, startAngle)}`,
    `A ${rOuter} ${rOuter} 0 ${large} 1 ${p(rOuter, endAngle)}`,
    `L ${p(rInner, endAngle)}`,
    `A ${rInner} ${rInner} 0 ${large} 0 ${p(rInner, startAngle)}`,
    'Z',
  ].join(' ')
}

function ModelDonut({ rows, lang }: { rows: UsageGroupStats[]; lang: string }) {
  const [hover, setHover] = useState<number | null>(null)
  const isDark = typeof document !== 'undefined' && document.documentElement.classList.contains('dark')
  const slices = useMemo(() => buildPieSlices(rows, isDark, lang), [rows, isDark, lang])
  const total = useMemo(() => slices.reduce((sum, slice) => sum + slice.value, 0), [slices])

  if (slices.length === 0) {
    return (
      <div className="flex h-36 items-center justify-center rounded-md border border-dashed border-[var(--border)] text-[12px] text-[var(--text-muted)]">
        {lang === 'zh' ? '暂无模型数据' : 'No model data'}
      </div>
    )
  }

  const CX = 100
  const CY = 100
  const R_OUT = 92
  const R_IN = 56
  let angle = 0
  const arcs = slices.map((slice, index) => {
    const start = angle
    const sweep = (slice.value / total) * Math.PI * 2
    angle += sweep
    return { slice, index, start, end: angle, path: donutArcPath(CX, CY, R_OUT, R_IN, start, angle) }
  })
  const active = hover != null ? slices[hover] : null

  return (
    <div className="@container">
      {/* SettingsGroup 已是卡片外壳,这里不再套边框/背景(避免卡中卡)。
          容器查询:≥28rem 环形图与表格同行(卡不被撑高),更窄才堆叠。 */}
      <div className="flex flex-col items-center gap-4 @md:flex-row @md:items-start">
        <div className="relative shrink-0">
        <svg viewBox="0 0 200 200" className="h-36 w-36" role="img" aria-label="model token distribution">
          {arcs.map(arc => (
            <path
              key={arc.index}
              d={arc.path}
              fill={arc.slice.color}
              opacity={hover == null || hover === arc.index ? 1 : 0.35}
              stroke={isDark ? '#0a0a0a' : '#ffffff'}
              strokeWidth="1.5"
              onMouseEnter={() => setHover(arc.index)}
              onMouseLeave={() => setHover(null)}
              style={{ transition: 'opacity 120ms' }}
            />
          ))}
        </svg>
        <div className="pointer-events-none absolute inset-0 flex flex-col items-center justify-center text-center">
          <div className="max-w-24 truncate text-[11px] text-neutral-500 dark:text-neutral-400">
            {active ? active.label : lang === 'zh' ? '总 Token' : 'Total'}
          </div>
          <div className="text-[16px] font-semibold text-neutral-900 dark:text-neutral-50">
            {formatTokens(active ? active.value : total)}
          </div>
          <div className="text-[10.5px] text-neutral-500 dark:text-neutral-500">
            {active ? formatPercent(active.value / total) : `${formatCount(slices.reduce((sum, slice) => sum + slice.requests, 0))} ${lang === 'zh' ? '次' : 'req'}`}
          </div>
        </div>
      </div>
      <div className="w-full min-w-0 flex-1 @md:w-auto">
        <table className="w-full table-fixed text-left text-[12px]">
          <colgroup>
            <col />
            <col className="w-[58px]" />
            <col className="w-[40px]" />
            <col className="w-[52px]" />
          </colgroup>
          <thead className="text-[10.5px] uppercase tracking-wide text-neutral-500 dark:text-neutral-500">
            <tr>
              <th className="py-1 pr-2 font-semibold">{lang === 'zh' ? '模型' : 'Model'}</th>
              <th className="py-1 pr-2 text-right font-semibold">Token</th>
              <th className="py-1 pr-2 text-right font-semibold">{lang === 'zh' ? '占比' : 'Share'}</th>
              <th className="py-1 text-right font-semibold">{lang === 'zh' ? '成本' : 'Cost'}</th>
            </tr>
          </thead>
          <tbody className="divide-y divide-neutral-100 dark:divide-neutral-800">
            {slices.map((slice, index) => (
              <tr
                key={`${slice.label}-${index}`}
                className={`text-neutral-800 dark:text-neutral-100 ${hover === index ? 'bg-[var(--bg-hover)]' : ''}`}
                onMouseEnter={() => setHover(index)}
                onMouseLeave={() => setHover(null)}
              >
                <td className="py-1.5 pr-2" title={slice.sub ? `${slice.label} · ${slice.sub}` : slice.label}>
                  <div className="flex min-w-0 items-center gap-1.5">
                    <span className="h-2 w-2 shrink-0 rounded-full" style={{ backgroundColor: slice.color }} />
                    <span className="truncate font-medium">{slice.label}</span>
                  </div>
                  {slice.sub && (
                    <div className="truncate pl-3.5 text-[10.5px] text-neutral-500 dark:text-neutral-500">{slice.sub}</div>
                  )}
                </td>
                <td className="py-1.5 pr-2 text-right tabular-nums">{formatTokens(slice.value)}</td>
                <td className="py-1.5 pr-2 text-right tabular-nums">{formatPercent(slice.value / total)}</td>
                <td className="py-1.5 text-right tabular-nums">{formatCost(slice.cost)}</td>
              </tr>
            ))}
          </tbody>
        </table>
        </div>
      </div>
    </div>
  )
}

function GroupTable({ rows, lang, type }: { rows: UsageGroupStats[]; lang: string; type: 'provider' | 'model' }) {
  if (rows.length === 0) {
    return (
      <div className="kv-panel">
        <div className="kv-panel-body">{lang === 'zh' ? '暂无统计数据' : 'No usage data'}</div>
      </div>
    )
  }
  return (
    <div className="custom-scrollbar overflow-x-auto rounded-md border border-[var(--border)] bg-[var(--bg-input)]">
      <table className="min-w-[720px] w-full text-left text-[12px]">
        <thead className="border-b border-[var(--border)] text-[10.5px] uppercase tracking-wide text-[var(--text-muted)]">
          <tr>
            <th className="px-3 py-2 font-semibold">{type === 'provider' ? 'Provider' : 'Model'}</th>
            <th className="px-3 py-2 font-semibold">{lang === 'zh' ? '请求' : 'Req'}</th>
            <th className="px-3 py-2 font-semibold">{lang === 'zh' ? '成功率' : 'Success'}</th>
            <th className="px-3 py-2 font-semibold">Token</th>
            <th className="px-3 py-2 font-semibold">{lang === 'zh' ? '输入/输出' : 'In/Out'}</th>
            <th className="px-3 py-2 font-semibold">{lang === 'zh' ? '成本' : 'Cost'}</th>
            <th className="px-3 py-2 font-semibold">{lang === 'zh' ? '平均耗时' : 'Avg'}</th>
            <th className="px-3 py-2 font-semibold">{lang === 'zh' ? '最近' : 'Last'}</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-neutral-100 dark:divide-neutral-800">
          {rows.map(row => {
            const successRate = row.requestCount > 0 ? row.successCount / row.requestCount : 0
            return (
              <tr key={row.id} className="text-neutral-800 dark:text-neutral-100">
                <td className="max-w-[220px] px-3 py-2">
                  <div className="truncate font-medium">{row.label}</div>
                  {type === 'model' && row.providerName && (
                    <div className="truncate text-[10.5px] text-neutral-500 dark:text-neutral-500">{row.providerName}</div>
                  )}
                </td>
                <td className="px-3 py-2 tabular-nums">{formatCount(row.requestCount)}</td>
                <td className="px-3 py-2 tabular-nums">{Math.round(successRate * 100)}%</td>
                <td className="px-3 py-2 tabular-nums">{formatTokens(row.totalTokens)}</td>
                <td className="px-3 py-2 tabular-nums">{formatTokens(row.inputTokens)} / {formatTokens(row.outputTokens)}</td>
                <td className="px-3 py-2 tabular-nums">{formatCost(row.costUsd)}</td>
                <td className="px-3 py-2 tabular-nums">{formatDuration(row.averageDurationMs)}</td>
                <td className="px-3 py-2 tabular-nums">{formatTime(row.lastUsedAt, lang)}</td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}

function LogsTable({ logs, lang }: { logs: UsageRecord[]; lang: string }) {
  if (logs.length === 0) {
    return (
      <div className="kv-panel">
        <div className="kv-panel-body">{lang === 'zh' ? '暂无请求日志' : 'No request logs'}</div>
      </div>
    )
  }
  return (
    <div className="custom-scrollbar overflow-x-auto rounded-md border border-[var(--border)] bg-[var(--bg-input)]">
      <table className="min-w-[920px] w-full text-left text-[12px]">
        <thead className="border-b border-[var(--border)] text-[10.5px] uppercase tracking-wide text-[var(--text-muted)]">
          <tr>
            <th className="px-3 py-2 font-semibold">{lang === 'zh' ? '时间' : 'Time'}</th>
            <th className="px-3 py-2 font-semibold">{lang === 'zh' ? '来源' : 'Source'}</th>
            <th className="px-3 py-2 font-semibold">Provider</th>
            <th className="px-3 py-2 font-semibold">Model</th>
            <th className="px-3 py-2 font-semibold">{lang === 'zh' ? '输入' : 'Input'}</th>
            <th className="px-3 py-2 font-semibold">{lang === 'zh' ? '输出' : 'Output'}</th>
            <th className="px-3 py-2 font-semibold">Token</th>
            <th className="px-3 py-2 font-semibold">{lang === 'zh' ? '成本' : 'Cost'}</th>
            <th className="px-3 py-2 font-semibold">{lang === 'zh' ? '耗时' : 'Time'}</th>
            <th className="px-3 py-2 font-semibold">{lang === 'zh' ? '状态' : 'Status'}</th>
            <th className="px-3 py-2 font-semibold">Usage</th>
          </tr>
        </thead>
        <tbody className="divide-y divide-neutral-100 dark:divide-neutral-800">
          {logs.map(record => (
            <tr key={record.id} className="text-neutral-800 dark:text-neutral-100">
              <td className="px-3 py-2 tabular-nums">{formatTime(record.createdAt, lang)}</td>
              <td className="px-3 py-2">
                <div className="truncate font-medium">{sourceLabel(record.source, lang)}</div>
                <div className="truncate text-[10.5px] text-neutral-500 dark:text-neutral-500">{record.operation}</div>
              </td>
              <td className="max-w-[140px] px-3 py-2 truncate">{record.providerName || record.providerId}</td>
              <td className="max-w-[180px] px-3 py-2 truncate font-mono text-[11.5px]">{record.model}</td>
              <td className="px-3 py-2 tabular-nums">{formatTokens(record.inputTokens)}</td>
              <td className="px-3 py-2 tabular-nums">
                {record.source === 'knowledge_base' ? '—' : formatTokens(record.outputTokens)}
              </td>
              <td className="px-3 py-2 tabular-nums">{formatTokens(recordTotalTokens(record))}</td>
              <td className="px-3 py-2 tabular-nums">{record.costUsd == null ? '--' : formatCost(record.costUsd)}</td>
              <td className="px-3 py-2 tabular-nums">{formatDuration(record.durationMs)}</td>
              <td className="px-3 py-2">
                <span className={`kv-tag ${record.status === 'success' ? 'ok' : record.status === 'cancelled' ? 'warn' : 'danger'}`}>
                  {statusLabel(record.status, lang)}
                </span>
              </td>
              <td className="px-3 py-2">
                <span className={`kv-tag ${record.usageSource === 'missing' ? 'warn' : 'ok'}`}>
                  {record.usageSource === 'missing' ? (lang === 'zh' ? '缺失' : 'missing') : 'provider'}
                </span>
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </div>
  )
}

export function UsageStatsPanel({ lang }: UsageStatsPanelProps) {
  const [range, setRange] = useState<UsageRange>('7d')
  const [view, setView] = useState<UsageView>('logs')
  const [source, setSource] = useState('all')
  const [status, setStatus] = useState('all')
  const [providerSearch, setProviderSearch] = useState('')
  const [modelSearch, setModelSearch] = useState('')
  const [debouncedProviderSearch, setDebouncedProviderSearch] = useState('')
  const [debouncedModelSearch, setDebouncedModelSearch] = useState('')
  const [logPageIndex, setLogPageIndex] = useState(0)
  const [stats, setStats] = useState<UsageStatsResponse | null>(null)
  const [loading, setLoading] = useState(false)
  const [clearing, setClearing] = useState(false)
  const [error, setError] = useState('')

  const loadStats = useCallback(async () => {
    setLoading(true)
    setError('')
    try {
      const data = await api.usageGetStats({
        range,
        source,
        status,
        providerSearch: debouncedProviderSearch,
        modelSearch: debouncedModelSearch,
        limit: LOG_PAGE_SIZE,
        offset: logPageIndex * LOG_PAGE_SIZE,
      })
      setStats(data)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [debouncedModelSearch, debouncedProviderSearch, logPageIndex, range, source, status])

  useEffect(() => {
    const timer = window.setTimeout(() => {
      setLogPageIndex(0)
      setDebouncedProviderSearch(providerSearch.trim())
      setDebouncedModelSearch(modelSearch.trim())
    }, SEARCH_DEBOUNCE_MS)
    return () => window.clearTimeout(timer)
  }, [modelSearch, providerSearch])

  useEffect(() => {
    void loadStats()
  }, [loadStats])

  const clearStats = useCallback(async () => {
    const ok = window.confirm(lang === 'zh' ? '清空所有本地用量统计？' : 'Clear all local usage statistics?')
    if (!ok) return
    setClearing(true)
    setError('')
    try {
      await api.usageClear()
      await loadStats()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setClearing(false)
    }
  }, [lang, loadStats])

  const summary = stats?.summary
  const totalLogs = stats?.totalLogs ?? 0
  const pageCount = Math.max(1, Math.ceil(totalLogs / LOG_PAGE_SIZE))
  const canGoPrev = logPageIndex > 0 && !loading
  const canGoNext = logPageIndex + 1 < pageCount && !loading

  useEffect(() => {
    if (logPageIndex > 0 && (totalLogs === 0 || logPageIndex >= pageCount)) {
      setLogPageIndex(Math.max(0, pageCount - 1))
    }
  }, [logPageIndex, pageCount, totalLogs])

  const updateRange = useCallback((next: UsageRange) => {
    setLogPageIndex(0)
    setRange(next)
  }, [])

  const updateSource = useCallback((next: string) => {
    setLogPageIndex(0)
    setSource(next)
  }, [])

  const updateStatus = useCallback((next: string) => {
    setLogPageIndex(0)
    setStatus(next)
  }, [])

  return (
    <div className="space-y-3">
      <SettingsGroup title={lang === 'zh' ? '总览' : 'Overview'}>
        <div className="mb-3 flex flex-wrap items-center justify-between gap-2">
          <div className="kv-seg">
            {(['today', '1d', '7d', '30d'] as UsageRange[]).map(option => (
              <button
                key={option}
                type="button"
                className={range === option ? 'active' : ''}
                onClick={() => updateRange(option)}
                data-tauri-drag-region="false"
              >
                {option === 'today' ? (lang === 'zh' ? '当天' : 'Today') : option}
              </button>
            ))}
          </div>
          <div className="flex items-center gap-1.5">
            <Button size="sm" onClick={() => void loadStats()} disabled={loading} data-tauri-drag-region="false">
              <RefreshCw size={11} className={loading ? 'animate-spin' : ''} />
              {lang === 'zh' ? '刷新' : 'Refresh'}
            </Button>
            <Button variant="danger" size="sm" onClick={() => void clearStats()} disabled={clearing || loading} data-tauri-drag-region="false">
              <Trash2 size={11} />
              {lang === 'zh' ? '清空' : 'Clear'}
            </Button>
          </div>
        </div>

        {error && (
          <div className="kv-panel warn mb-3">
            <div className="kv-panel-body">{error}</div>
          </div>
        )}

        <div className="grid grid-cols-2 gap-2 lg:grid-cols-4">
          <SummaryTile label={lang === 'zh' ? '总 Token' : 'Total tokens'} value={formatTokens(summary?.totalTokens)} sub={`${formatCount(summary?.totalRequests)} ${lang === 'zh' ? '次请求' : 'requests'}`} />
          <SummaryTile label={lang === 'zh' ? '估算成本' : 'Estimated cost'} value={formatCost(summary?.totalCostUsd)} sub={lang === 'zh' ? '按本地模型价格估算' : 'From local model pricing'} />
          <SummaryTile label={lang === 'zh' ? '输入 / 输出' : 'Input / Output'} value={`${formatTokens(summary?.inputTokens)} / ${formatTokens(summary?.outputTokens)}`} sub={lang === 'zh' ? 'provider 返回 usage 时统计' : 'Provider usage only'} />
          {/* 命中率提到独立卡，下面「缓存命中」那张就只报 token 数，不再重复同一个百分比。 */}
          <SummaryTile
            label={lang === 'zh' ? '缓存命中率' : 'Cache hit rate'}
            value={
              summary && summary.inputTokens > 0
                ? formatPercent(summary.cachedInputTokens / summary.inputTokens)
                : '0%'
            }
            sub={lang === 'zh' ? '命中缓存的输入 token 占比' : 'Share of input tokens served from cache'}
          />
          <SummaryTile
            label={lang === 'zh' ? '缓存命中' : 'Cached input'}
            value={formatTokens(summary?.cachedInputTokens)}
          />
          <SummaryTile label={lang === 'zh' ? '缓存创建' : 'Cache creation'} value={formatTokens(summary?.cacheCreationInputTokens)} />
          <SummaryTile label={lang === 'zh' ? '推理 Token' : 'Reasoning'} value={formatTokens(summary?.reasoningTokens)} />
          <SummaryTile label={lang === 'zh' ? '平均耗时' : 'Avg duration'} value={formatDuration(summary?.averageDurationMs)} />
        </div>
      </SettingsGroup>

      {/* 容器查询:按内容区实际宽度(非视口)决定并排/堆叠——设置窗口可任意缩放,
          视口断点在这里不可靠。≥64rem(1024px)容器宽才并排,保证每卡内部
          环形图+表格仍能同行。 */}
      <div className="@container">
        <div className="grid grid-cols-1 gap-3 @5xl:grid-cols-2">
          <SettingsGroup title={lang === 'zh' ? '模型分布' : 'Model distribution'}>
            <ModelDonut rows={stats?.modelStats ?? []} lang={lang} />
          </SettingsGroup>

          <SettingsGroup title={lang === 'zh' ? '趋势' : 'Trend'}>
            <TrendChart points={stats?.trend ?? []} lang={lang} />
          </SettingsGroup>
        </div>
      </div>

      <SettingsGroup title={lang === 'zh' ? '明细' : 'Details'}>
        <div className="mb-3 flex flex-col gap-2">
          <div className="flex flex-wrap items-center gap-2">
            <div className="kv-seg">
              {[
                { id: 'logs' as const, label: lang === 'zh' ? '请求日志' : 'Logs' },
                { id: 'providers' as const, label: 'Provider' },
                { id: 'models' as const, label: lang === 'zh' ? '模型' : 'Models' },
              ].map(option => (
                <button
                  key={option.id}
                  type="button"
                  className={view === option.id ? 'active' : ''}
                  onClick={() => setView(option.id)}
                  data-tauri-drag-region="false"
                >
                  {option.label}
                </button>
              ))}
            </div>
            <Select
              className="w-40"
              value={source}
              onChange={updateSource}
              options={SOURCE_OPTIONS.map(value => ({ value, label: sourceLabel(value, lang) }))}
            />
            <Select
              className="w-36"
              value={status}
              onChange={updateStatus}
              options={STATUS_OPTIONS.map(value => ({ value, label: statusLabel(value, lang) }))}
            />
          </div>
          <div className="grid grid-cols-1 gap-2 md:grid-cols-2">
            <Input value={providerSearch} onChange={setProviderSearch} placeholder={lang === 'zh' ? '搜索 Provider' : 'Search provider'} />
            <Input value={modelSearch} onChange={setModelSearch} placeholder={lang === 'zh' ? '搜索模型' : 'Search model'} mono />
          </div>
        </div>

        {view === 'logs' && <LogsTable logs={stats?.logs ?? []} lang={lang} />}
        {view === 'providers' && <GroupTable rows={stats?.providerStats ?? []} lang={lang} type="provider" />}
        {view === 'models' && <GroupTable rows={stats?.modelStats ?? []} lang={lang} type="model" />}

        {stats && view === 'logs' && (
          <div className="mt-2 flex flex-wrap items-center justify-between gap-2 text-[11px] text-neutral-500 dark:text-neutral-500">
            <span>
              {lang === 'zh'
                ? `显示 ${pageRangeLabel(logPageIndex, LOG_PAGE_SIZE, totalLogs)} 条`
                : `Showing ${pageRangeLabel(logPageIndex, LOG_PAGE_SIZE, totalLogs)}`}
            </span>
            <div className="flex items-center gap-1.5">
              <Button
                size="sm"
                onClick={() => setLogPageIndex(page => Math.max(0, page - 1))}
                disabled={!canGoPrev}
                data-tauri-drag-region="false"
                title={lang === 'zh' ? '上一页' : 'Previous page'}
              >
                <ChevronLeft size={11} />
                {lang === 'zh' ? '上一页' : 'Prev'}
              </Button>
              <span className="min-w-12 text-center tabular-nums">
                {logPageIndex + 1} / {pageCount}
              </span>
              <Button
                size="sm"
                onClick={() => setLogPageIndex(page => Math.min(pageCount - 1, page + 1))}
                disabled={!canGoNext}
                data-tauri-drag-region="false"
                title={lang === 'zh' ? '下一页' : 'Next page'}
              >
                {lang === 'zh' ? '下一页' : 'Next'}
                <ChevronRight size={11} />
              </Button>
            </div>
          </div>
        )}
      </SettingsGroup>
    </div>
  )
}
