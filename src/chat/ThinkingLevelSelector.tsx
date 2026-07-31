import { memo, useEffect, useMemo, useState } from 'react'
import { Brain, Check, ChevronDown } from 'lucide-react'
import { api } from '../api/tauri'
import { getSettingsCached } from '../api/settingsCache'
import { isProviderEnabled } from '../settings/utils'
import { chatTitlebarPillButtonClass } from './platform'
import type { ThinkingLevel } from './types'

interface ThinkingLevelSelectorProps {
  /** 当前等级；null = 未显式设置，按默认档 DEFAULT_LEVEL 处理。 */
  value: ThinkingLevel | null
  currentProviderId: string
  currentModel: string
  onChange: (level: ThinkingLevel) => void
}

// 固定项 + 各等级标签（英文，跨语言更通用）。具体显示哪些等级由后端按模型库决定。
const LABELS: Record<string, string> = {
  off: 'Off',
  low: 'Low',
  medium: 'Medium',
  high: 'High',
  xhigh: 'XHigh',
  max: 'Max',
}
// 未显式选等级时的默认档（与后端 resolve_thinking 保持一致）。
const DEFAULT_LEVEL: ThinkingLevel = 'high'
// 未取到模型能力时的安全兜底（全模型通用子集）。
const FALLBACK_LEVELS = ['low', 'medium', 'high']

function labelFor(value: ThinkingLevel): string {
  return LABELS[value] ?? value
}

function ThinkingLevelSelectorBase({
  value,
  currentProviderId,
  currentModel,
  onChange,
}: ThinkingLevelSelectorProps) {
  const [open, setOpen] = useState(false)
  const [levels, setLevels] = useState<string[]>(FALLBACK_LEVELS)

  // 思考等级清单来自后端模型库（reasoningEfforts），按 (model, apiFormat) 解析。
  useEffect(() => {
    let alive = true
    void (async () => {
      if (!currentModel) {
        if (alive) setLevels(FALLBACK_LEVELS)
        return
      }
      try {
        const settings = await getSettingsCached()
        const apiFormat = (settings.providers || [])
          .filter(isProviderEnabled)
          .find((p) => p.id === currentProviderId)?.apiFormat
        const got = await api.reasoningEffortsForModel(currentModel, apiFormat)
        // 空列表是有意义的答案（该模型没有 effort 旋钮），不能再兜底成 FALLBACK_LEVELS。
        if (alive) setLevels(got)
      } catch {
        if (alive) setLevels(FALLBACK_LEVELS)
      }
    })()
    return () => {
      alive = false
    }
  }, [currentProviderId, currentModel])

  // null（未显式设置）按默认档处理；存的档若不在当前模型的支持列表里（换模型最常见：
  // 在 gpt-5.6 选了 xhigh 再切回 gpt-5）就地收敛，UI 永远高亮一个真实存在的等级。
  const effective = useMemo<ThinkingLevel>(() => {
    const current = value ?? DEFAULT_LEVEL
    if (current === 'off' || levels.length === 0 || levels.includes(current)) return current
    const fixed = levels.includes(DEFAULT_LEVEL) ? DEFAULT_LEVEL : levels[levels.length - 1]
    return fixed as ThinkingLevel
  }, [value, levels])

  // 收敛结果要落盘，否则按钮显示 High、请求却仍按存着的 xhigh 发出去，直接吃 provider 的 400。
  useEffect(() => {
    if (levels.length > 0 && effective !== (value ?? DEFAULT_LEVEL)) onChange(effective)
  }, [effective, value, levels, onChange])

  const options = useMemo<Array<{ value: ThinkingLevel; label: string }>>(
    () => [
      { value: 'off', label: LABELS.off },
      ...levels.map((l) => ({ value: l as ThinkingLevel, label: LABELS[l] ?? l })),
    ],
    [levels],
  )

  // 该模型没有思考等级可调（Claude 4.5 及更早、GLM-4.7、Kimi K2.x…）→ 不显示这个旋钮。
  if (levels.length === 0) return null

  return (
    <div className="relative max-w-full min-w-0" data-tauri-drag-region="false">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className={`${chatTitlebarPillButtonClass} max-w-full min-w-0`}
        title={`思考等级：${labelFor(effective)}`}
        aria-label={`思考等级：${labelFor(effective)}`}
      >
        <Brain size={15} className="shrink-0 text-neutral-500 dark:text-neutral-400" />
        <span className="chat-thinking-level-label max-w-[64px] truncate font-medium text-neutral-800 dark:text-neutral-200">
          {labelFor(effective)}
        </span>
        <ChevronDown
          size={15}
          className={`shrink-0 text-neutral-400 transition-transform ${open ? 'rotate-180' : ''}`}
        />
      </button>

      {open && (
        <>
          <div className="fixed inset-0 z-10" onClick={() => setOpen(false)} aria-hidden />
          <div className="chat-model-selector-menu chat-motion-popover absolute left-0 top-full z-20 mt-2 min-w-[160px] overflow-y-auto kv-menu">
            {options.map((opt) => {
              const active = opt.value === effective
              return (
                <button
                  key={opt.value}
                  type="button"
                  onClick={() => {
                    onChange(opt.value)
                    setOpen(false)
                  }}
                  className={`kv-menu-row justify-between transition-colors ${
                    active
                      ? 'bg-neutral-100 font-medium text-neutral-900 dark:bg-neutral-800 dark:text-neutral-100'
                      : 'text-neutral-700 hover:bg-neutral-50 dark:text-neutral-300 dark:hover:bg-neutral-800/80'
                  }`}
                >
                  <span className="min-w-0 truncate">{opt.label}</span>
                  {active && <Check size={15} className="shrink-0 text-neutral-500" />}
                </button>
              )
            })}
          </div>
        </>
      )}
    </div>
  )
}

// memo：顶栏选择器，仅在 props 变化时重渲。
export const ThinkingLevelSelector = memo(ThinkingLevelSelectorBase)
