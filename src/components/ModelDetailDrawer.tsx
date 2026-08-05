import { useState, useEffect, useCallback } from 'react'
import { ArrowLeft, RotateCcw } from 'lucide-react'
import type { ModelInfo } from '../api/tauri'
import { resolveModelInfo, matchModel } from '../data/modelMatching'
import { Toggle, Input } from '../settings/components'
import { Button, IconButton } from './Button'

type Lang = 'zh' | 'en'

/** 与 Rust `REASONING_EFFORT_LEVELS` 一致，顺序即展示顺序。 */
const EFFORT_LEVELS = ['low', 'medium', 'high', 'xhigh', 'max'] as const

type ModelDetailDrawerProps = {
  modelName: string
  overrides?: Record<string, ModelInfo>
  lang: Lang
  onClose: () => void
  onSave: (modelName: string, info: ModelInfo) => void
  onReset: (modelName: string) => void
}

function deepEqual(a: unknown, b: unknown): boolean {
  return JSON.stringify(a) === JSON.stringify(b)
}

export function ModelDetailDrawer({
  modelName,
  overrides,
  lang,
  onClose,
  onSave,
  onReset,
}: ModelDetailDrawerProps) {
  const resolved = resolveModelInfo(modelName, overrides)
  const dbDefaults = matchModel(modelName)
  const hasOverride = !!overrides?.[modelName]

  const [form, setForm] = useState<ModelInfo>(resolved)
  const [temperatureInput, setTemperatureInput] = useState(
    resolved.temperature?.toString() ?? '',
  )
  const [extraBodyInput, setExtraBodyInput] = useState(
    resolved.extraBody ? JSON.stringify(resolved.extraBody, null, 2) : '',
  )

  useEffect(() => {
    const next = resolveModelInfo(modelName, overrides)
    setForm(next)
    setTemperatureInput(next.temperature?.toString() ?? '')
    setExtraBodyInput(next.extraBody ? JSON.stringify(next.extraBody, null, 2) : '')
  }, [modelName, overrides])

  const updateField = useCallback(<K extends keyof ModelInfo>(key: K, value: ModelInfo[K]) => {
    setForm(prev => ({ ...prev, [key]: value }))
  }, [])

  const updateCapability = useCallback((key: keyof NonNullable<ModelInfo['capabilities']>, value: boolean) => {
    setForm(prev => ({
      ...prev,
      capabilities: { ...prev.capabilities, [key]: value },
    }))
  }, [])

  const updatePricing = useCallback((key: keyof NonNullable<ModelInfo['pricing']>, value: string) => {
    const num = value === '' ? undefined : Number(value)
    setForm(prev => ({
      ...prev,
      pricing: { ...prev.pricing, [key]: num },
    }))
  }, [])

  // 思考等级白名单：undefined=跟随模型库/家族兜底；[]=该模型不发送等级字段；否则只这几档。
  const toggleEffort = useCallback((level: string) => {
    setForm(prev => {
      const current = prev.reasoningEfforts
      if (!current) return { ...prev, reasoningEfforts: [level] }
      const next = current.includes(level)
        ? current.filter(item => item !== level)
        : EFFORT_LEVELS.filter(item => item === level || current.includes(item))
      return { ...prev, reasoningEfforts: [...next] }
    })
  }, [])

  const updateTemperature = useCallback((value: string) => {
    setTemperatureInput(value)
    const trimmed = value.trim()
    if (!trimmed) {
      setForm(prev => ({
        ...prev,
        temperature: undefined,
        omitTemperature: true,
      }))
      return
    }
    const temperature = Number(trimmed)
    if (!Number.isFinite(temperature)) return
    setForm(prev => ({
      ...prev,
      temperature,
      omitTemperature: undefined,
    }))
  }, [])

  const parsedTemperature = temperatureInput.trim() === ''
    ? undefined
    : Number(temperatureInput)
  const temperatureInvalid = parsedTemperature !== undefined && (
    !Number.isFinite(parsedTemperature) || parsedTemperature < 0 || parsedTemperature > 2
  )

  // extraBody：留空=无覆盖；否则必须是合法 JSON 对象。非对象/解析失败视为无效并阻止保存。
  const updateExtraBody = useCallback((value: string) => {
    setExtraBodyInput(value)
    const trimmed = value.trim()
    if (!trimmed) {
      setForm(prev => ({ ...prev, extraBody: undefined }))
      return
    }
    try {
      const parsed = JSON.parse(trimmed)
      if (parsed && typeof parsed === 'object' && !Array.isArray(parsed)) {
        setForm(prev => ({ ...prev, extraBody: parsed }))
      }
    } catch {
      // 无效 JSON：保留输入文本，由 extraBodyInvalid 拦截保存，不改 form。
    }
  }, [])

  const extraBodyInvalid = (() => {
    const trimmed = extraBodyInput.trim()
    if (!trimmed) return false
    try {
      const parsed = JSON.parse(trimmed)
      return !(parsed && typeof parsed === 'object' && !Array.isArray(parsed))
    } catch {
      return true
    }
  })()

  const handleSave = useCallback(() => {
    if (temperatureInvalid || extraBodyInvalid) return
    onSave(modelName, form)
  }, [modelName, form, onSave, temperatureInvalid, extraBodyInvalid])

  const handleReset = useCallback(() => {
    onReset(modelName)
    if (dbDefaults) {
      setForm(dbDefaults)
      setTemperatureInput(dbDefaults.temperature?.toString() ?? '')
      setExtraBodyInput(dbDefaults.extraBody ? JSON.stringify(dbDefaults.extraBody, null, 2) : '')
    }
  }, [modelName, onReset, dbDefaults])

  const isDirty = !deepEqual(form, resolved)

  const t = {
    title: lang === 'zh' ? '模型详情' : 'Model Details',
    back: lang === 'zh' ? '返回' : 'Back',
    displayName: lang === 'zh' ? '显示名称' : 'Display Name',
    contextWindow: lang === 'zh' ? '上下文长度' : 'Context Window',
    maxOutput: lang === 'zh' ? '最大输出' : 'Max Output',
    temperature: lang === 'zh' ? 'Temperature（温度）' : 'Temperature',
    temperatureHint: lang === 'zh' ? '留空则请求不发送 temperature。' : 'Leave blank to omit temperature from requests.',
    temperatureInvalid: lang === 'zh' ? '请输入 0 到 2 之间的数值。' : 'Enter a value between 0 and 2.',
    capabilities: lang === 'zh' ? '功能' : 'Capabilities',
    vision: lang === 'zh' ? '图像输入' : 'Image Input',
    functionCalling: lang === 'zh' ? '工具调用' : 'Tool Calling',
    reasoning: lang === 'zh' ? '推理模式' : 'Reasoning',
    streaming: lang === 'zh' ? '流式输出' : 'Streaming',
    webSearch: lang === 'zh' ? '网络搜索' : 'Web Search',
    imageGeneration: lang === 'zh' ? '生图' : 'Image Generation',
    efforts: lang === 'zh' ? '思考等级' : 'Reasoning Effort',
    effortsFollow: lang === 'zh' ? '跟随模型库' : 'Follow database',
    effortsFollowHint: lang === 'zh'
      ? '跟随模型库；模型库也没列时按家族兜底（Anthropic 全档，其余 low/medium/high）。'
      : 'Follows the model database; falls back per family (Anthropic all levels, others low/medium/high).',
    effortsEmptyHint: lang === 'zh'
      ? '全部取消 = 该模型没有思考等级旋钮，请求不发送等级字段，工具栏也不显示选择器。'
      : 'None selected = no effort knob: requests omit the level and the picker is hidden.',
    effortsHint: lang === 'zh'
      ? '只有勾选的等级可选、可下发。发错等级会被上游直接 400，不做静默降级。'
      : 'Only checked levels are selectable and sent. A wrong level 400s upstream; no silent downgrade.',
    pricing: lang === 'zh' ? '定价 (per 1M tokens, USD)' : 'Pricing (per 1M tokens, USD)',
    input: lang === 'zh' ? '输入' : 'Input',
    output: lang === 'zh' ? '输出' : 'Output',
    cachedInput: lang === 'zh' ? '缓存输入' : 'Cached Input',
    extraBody: lang === 'zh' ? '额外请求体 (JSON)' : 'Extra Request Body (JSON)',
    extraBodyHint: lang === 'zh'
      ? '原样合并进请求体根部。用于端点私有参数，如 NVIDIA NIM / vLLM 的 chat_template_kwargs。留空则不发送。'
      : 'Merged into the request body root. For endpoint-specific params like NVIDIA NIM / vLLM chat_template_kwargs. Leave blank to omit.',
    extraBodyInvalid: lang === 'zh' ? '请输入合法的 JSON 对象。' : 'Enter a valid JSON object.',
    save: lang === 'zh' ? '保存' : 'Save',
    reset: lang === 'zh' ? '重置为默认值' : 'Reset to Defaults',
    noDatabase: lang === 'zh' ? '未在数据库中找到此模型，可手动填写参数。' : 'Model not found in database. You can fill in parameters manually.',
  }

  return (
    <div
      className="kv-modal-backdrop"
      data-tauri-drag-region="false"
      onMouseDown={(e) => { if (e.target === e.currentTarget) onClose() }}
    >
      <div className="kv-drawer" data-tauri-drag-region="false" onMouseDown={(e) => e.stopPropagation()}>
        <div className="kv-drawer-header">
          <IconButton
            size="xs"
            onClick={onClose}
            data-tauri-drag-region="false"
            label={t.back}
          >
            <ArrowLeft size={14} />
          </IconButton>
          {/* 顶栏只放短标题；长模型 id 在下方，避免与聊天设置页 Windows 三键重叠。 */}
          <span className="kv-drawer-title">{t.title}</span>
        </div>

        <div className="kv-drawer-body custom-scrollbar">
          <div className="kv-drawer-model-id" title={modelName}>{modelName}</div>

          {!dbDefaults && (
            <p className="kv-drawer-hint">{t.noDatabase}</p>
          )}

          <div className="kv-drawer-section">
            <label className="kv-drawer-label">{t.displayName}</label>
            <Input
              value={form.displayName || ''}
              onChange={(v) => updateField('displayName', v || undefined)}
              placeholder={modelName}
              mono
            />
          </div>

          <div className="kv-drawer-row">
            <div className="kv-drawer-section flex-1">
              <label className="kv-drawer-label">{t.contextWindow}</label>
              <Input
                type="number"
                value={form.contextWindow?.toString() || ''}
                onChange={(v) => updateField('contextWindow', v ? Number(v) : undefined)}
                placeholder="-"
              />
            </div>
            <div className="kv-drawer-section flex-1">
              <label className="kv-drawer-label">{t.maxOutput}</label>
              <Input
                type="number"
                value={form.maxOutput?.toString() || ''}
                onChange={(v) => updateField('maxOutput', v ? Number(v) : undefined)}
                placeholder="-"
              />
            </div>
          </div>

          <div className="kv-drawer-section">
            <label className="kv-drawer-label">{t.temperature}</label>
            <Input
              type="number"
              value={temperatureInput}
              onChange={updateTemperature}
              placeholder="-"
              min={0}
              max={2}
              step={0.1}
              aria-invalid={temperatureInvalid}
            />
            <p className={`mt-1 text-[11px] ${temperatureInvalid ? 'text-red-500' : 'text-neutral-400 dark:text-neutral-500'}`}>
              {temperatureInvalid ? t.temperatureInvalid : t.temperatureHint}
            </p>
          </div>

          <div className="kv-drawer-section">
            <label className="kv-drawer-label">{t.capabilities}</label>
            <div className="kv-drawer-toggles">
              <CapabilityToggle label={t.vision} checked={form.capabilities?.vision ?? false} onChange={(v) => updateCapability('vision', v)} />
              <CapabilityToggle label={t.functionCalling} checked={form.capabilities?.functionCalling ?? false} onChange={(v) => updateCapability('functionCalling', v)} />
              <CapabilityToggle label={t.reasoning} checked={form.capabilities?.reasoning ?? false} onChange={(v) => updateCapability('reasoning', v)} />
              <CapabilityToggle label={t.streaming} checked={form.capabilities?.streaming ?? false} onChange={(v) => updateCapability('streaming', v)} />
              <CapabilityToggle label={t.webSearch} checked={form.capabilities?.webSearch ?? false} onChange={(v) => updateCapability('webSearch', v)} />
              <CapabilityToggle label={t.imageGeneration} checked={form.capabilities?.imageGeneration ?? false} onChange={(v) => updateCapability('imageGeneration', v)} />
            </div>
          </div>

          <div className="kv-drawer-section">
            <label className="kv-drawer-label">{t.efforts}</label>
            <div className="kv-drawer-toggles">
              <div className="kv-drawer-toggle-row">
                <span className="kv-drawer-toggle-label">{t.effortsFollow}</span>
                <Toggle
                  checked={!form.reasoningEfforts}
                  onChange={(v) =>
                    updateField(
                      'reasoningEfforts',
                      // 关掉「跟随」时从当前生效档起步，而不是空/全档，免得一关就改了行为。
                      v ? undefined : (dbDefaults?.reasoningEfforts ?? ['low', 'medium', 'high']),
                    )
                  }
                />
              </div>
              {form.reasoningEfforts && (
                <div className="kv-drawer-toggle-row">
                  <div className="kv-seg">
                    {EFFORT_LEVELS.map((level) => (
                      <button
                        key={level}
                        type="button"
                        className={`shrink-0 ${form.reasoningEfforts?.includes(level) ? 'active' : ''}`}
                        onClick={() => toggleEffort(level)}
                        data-tauri-drag-region="false"
                      >
                        {level}
                      </button>
                    ))}
                  </div>
                </div>
              )}
            </div>
            <p className="text-[11px] text-neutral-400 dark:text-neutral-500">
              {!form.reasoningEfforts
                ? t.effortsFollowHint
                : form.reasoningEfforts.length === 0
                  ? t.effortsEmptyHint
                  : t.effortsHint}
            </p>
          </div>

          <div className="kv-drawer-section">
            <label className="kv-drawer-label">{t.pricing}</label>
            <div className="kv-drawer-row">
              <div className="kv-drawer-section flex-1">
                <label className="kv-drawer-sublabel">{t.input}</label>
                <Input
                  type="number"
                  value={form.pricing?.input?.toString() || ''}
                  onChange={(v) => updatePricing('input', v)}
                  placeholder="0.00"
                />
              </div>
              <div className="kv-drawer-section flex-1">
                <label className="kv-drawer-sublabel">{t.output}</label>
                <Input
                  type="number"
                  value={form.pricing?.output?.toString() || ''}
                  onChange={(v) => updatePricing('output', v)}
                  placeholder="0.00"
                />
              </div>
              <div className="kv-drawer-section flex-1">
                <label className="kv-drawer-sublabel">{t.cachedInput}</label>
                <Input
                  type="number"
                  value={form.pricing?.cachedInput?.toString() || ''}
                  onChange={(v) => updatePricing('cachedInput', v)}
                  placeholder="-"
                />
              </div>
            </div>
          </div>

          <div className="kv-drawer-section">
            <label className="kv-drawer-label">{t.extraBody}</label>
            <textarea
              className="kv-input font-mono text-[12px] min-h-[88px] resize-y"
              value={extraBodyInput}
              onChange={(e) => updateExtraBody(e.target.value)}
              placeholder={'{\n  "chat_template_kwargs": { "thinking": true }\n}'}
              spellCheck={false}
              data-tauri-drag-region="false"
              aria-invalid={extraBodyInvalid}
            />
            <p className={`mt-1 text-[11px] ${extraBodyInvalid ? 'text-red-500' : 'text-neutral-400 dark:text-neutral-500'}`}>
              {extraBodyInvalid ? t.extraBodyInvalid : t.extraBodyHint}
            </p>
          </div>
        </div>

        <div className="kv-drawer-footer">
          {hasOverride && (
            <Button
              variant="ghost"
              onClick={handleReset}
              data-tauri-drag-region="false"
            >
              <RotateCcw size={12} />
              {t.reset}
            </Button>
          )}
          <div className="flex-1" />
          <Button
            variant="primary"
            onClick={handleSave}
            disabled={!isDirty || temperatureInvalid || extraBodyInvalid}
            data-tauri-drag-region="false"
          >
            {t.save}
          </Button>
        </div>
      </div>
    </div>
  )
}

function CapabilityToggle({ label, checked, onChange }: {
  label: string
  checked: boolean
  onChange: (v: boolean) => void
}) {
  return (
    <div className="kv-drawer-toggle-row">
      <span className="kv-drawer-toggle-label">{label}</span>
      <Toggle checked={checked} onChange={onChange} />
    </div>
  )
}
