import { useMemo, useState, type ComponentType, type CSSProperties } from 'react'
import { createPortal } from 'react-dom'
import {
  Eye,
  EyeOff,
  Plus,
  RefreshCw,
  RotateCcw,
  SlidersHorizontal,
  Trash2,
  X,
} from 'lucide-react'
import ChatGLM from '@lobehub/icons/es/ChatGLM/components/Color'
import Moonshot from '@lobehub/icons/es/Moonshot/components/Mono'
import DeepSeek from '@lobehub/icons/es/DeepSeek/components/Color'
import Minimax from '@lobehub/icons/es/Minimax/components/Color'
import Bailian from '@lobehub/icons/es/Bailian/components/Color'
import OpenRouter from '@lobehub/icons/es/OpenRouter/components/Mono'
import OpenCode from '@lobehub/icons/es/OpenCode/components/Mono'
import LongCat from '@lobehub/icons/es/LongCat/components/Color'
import XiaomiMiMo from '@lobehub/icons/es/XiaomiMiMo/components/Mono'
import OpenAI from '@lobehub/icons/es/OpenAI/components/Mono'
import { Input, Label, Select, SuggestInput, TextArea, Toggle } from './components'
import { Button, IconButton } from '../components/Button'
import type { SelectOption } from './utils'
import { chatApi } from '../chat/api'
import { i18n, type Lang } from './i18n'
import type { ExternalCliProvider } from '../api/tauri'
import {
  applyClaudePreset,
  CLAUDE_BASE_URL,
  CLAUDE_CUSTOM_PRESET,
  CLAUDE_RELAY_PRESETS,
  CLAUDE_TIER_KEYS,
  CUSTOM_PROXY_PRESET_ID,
  detectClaudePresetId,
  envPairsToRecord,
  readClaudeApiKey,
  writeClaudeApiKey,
  type ClaudePresetBrand,
  type ClaudePresetId,
  type EnvPair,
} from './cliClaudePresets'
import {
  applyCodexPreset,
  CODEX_CUSTOM_PRESET_ID,
  CODEX_PRESET_BUTTONS,
  detectCodexPresetId,
  extractCodexBaseUrl,
  extractCodexModel,
  extractOpenAiApiKey,
  initialCodexTomlAuth,
  setCodexStructuredFields,
  validateCodexConfigToml,
  type CodexPresetBrand,
  type CodexPresetId,
} from './cliCodexPresets'
import {
  buildNativeCliProvider,
  emptyNativeModel,
  normalizeNativeModels,
  PI_API_OPTIONS,
  piThinkingOptionsForModel,
  readNativeCliProvider,
  recommendedPiThinkingLevel,
  resolvePiModelMetadata,
  type NativeCliAgentId,
  type PiThinkingLevel,
} from './cliNativeProviderConfigs'

function isPositiveInteger(value: string): boolean {
  return /^\d+$/.test(value) && Number.isSafeInteger(Number(value)) && Number(value) > 0
}

function formatTokenLimit(value: number): string {
  if (value >= 1_000_000) return `${Number((value / 1_000_000).toFixed(2))}M`
  if (value >= 1_000) return `${Number((value / 1_000).toFixed(1))}K`
  return String(value)
}

type Glyph = ComponentType<{ size?: number; style?: CSSProperties }>
const G = (icon: unknown) => icon as Glyph

type PresetBrand = ClaudePresetBrand | CodexPresetBrand

const BRAND_ICON: Record<PresetBrand, Glyph | null> = {
  claude: null,
  zhipu: G(ChatGLM),
  kimi: G(Moonshot),
  deepseek: G(DeepSeek),
  minimax: G(Minimax),
  xiaomi: G(XiaomiMiMo),
  bailian: G(Bailian),
  longcat: G(LongCat),
  opencode: G(OpenCode),
  openrouter: G(OpenRouter),
  atlas: G(OpenAI),
  custom: null,
}

function readEnv(env: EnvPair[], key: string): string {
  return env.find((pair) => pair.key === key)?.value ?? ''
}

/** 写一个键：空值删键而不是留空串——空串在 claude 那边是「显式清空」，语义不同。 */
function writeEnv(env: EnvPair[], key: string, value: string): EnvPair[] {
  const next = env.filter((pair) => pair.key !== key)
  if (value.trim()) next.push({ key, value })
  return next
}

/** 必填标记的字段标题。 */
function FieldLabel({ text, required }: { text: string; required?: boolean }) {
  return (
    <Label>
      <span>
        {text}
        {required && <span className="kv-req"> *</span>}
      </span>
    </Label>
  )
}

/**
 * 模型档位：始终可手输；获取模型后右侧出现下拉箭头，点开用与 Select 同款菜单。
 */
function TierField({
  label,
  value,
  onChange,
  modelOptions,
  placeholder,
}: {
  label: string
  value: string
  onChange: (value: string) => void
  modelOptions: SelectOption[]
  placeholder?: string
}) {
  return (
    <div className="kv-tier-field kv-tier-field--suggest">
      <span>{label}</span>
      <SuggestInput
        value={value}
        onChange={onChange}
        options={modelOptions}
        placeholder={placeholder ?? '—'}
        mono
        className="kv-tier-suggest"
        ariaLabel={label}
      />
    </div>
  )
}

function PresetIcon({ brand }: { brand: PresetBrand }) {
  const Icon = BRAND_ICON[brand]
  if (!Icon) {
    return <SlidersHorizontal size={14} strokeWidth={2.1} />
  }
  return <Icon size={14} />
}

type I18nDict = (typeof i18n)[Lang]

function presetLabel(t: I18nDict, nameKey: string): string {
  const map: Record<string, string> = {
    custom: t.externalAgentsPresetCustom,
    zhipu: t.externalAgentsPresetZhipu,
    kimi: t.externalAgentsPresetKimi,
    kimiCoding: t.externalAgentsPresetKimiCoding,
    deepseek: t.externalAgentsPresetDeepseek,
    minimax: t.externalAgentsPresetMinimax,
    xiaomi: t.externalAgentsPresetXiaomi,
    xiaomiPlan: t.externalAgentsPresetXiaomiPlan,
    bailian: t.externalAgentsPresetBailian,
    bailianCoding: t.externalAgentsPresetBailianCoding,
    longcat: t.externalAgentsPresetLongcat,
    opencodeGo: t.externalAgentsPresetOpencodeGo,
    openrouter: t.externalAgentsPresetOpenrouter,
    atlasCloud: t.externalAgentsPresetAtlasCloud,
  }
  return map[nameKey] ?? nameKey
}

function initialClaudeEnv(initial?: ExternalCliProvider | null): EnvPair[] {
  if (initial?.env?.length) return initial.env
  // 新建默认自定义（官方区块已去掉，不再默认官方直连）
  return applyClaudePreset(CUSTOM_PROXY_PRESET_ID, [])
}

/**
 * 供应商编辑弹窗。**按 CLI 分形态**，因为各 CLI 接第三方的机制根本不同：
 * - claude：预设 + 结构化字段 + JSON / 原始 env
 * - codex：预设 + config.toml / auth.json（物化成私有 CODEX_HOME）
 * - opencode / pi：原生 provider / auth / default model 配置
 * - 其余：手填环境变量
 */
export function CliProviderModal({
  lang,
  agentId,
  agentName,
  initial,
  onSave,
  onClose,
}: {
  lang: Lang
  agentId: string
  agentName: string
  initial?: ExternalCliProvider | null
  onSave: (provider: ExternalCliProvider) => void
  onClose: () => void
}) {
  const t = i18n[lang]
  const isCodex = agentId === 'codex'
  const isClaude = agentId === 'claude'
  const isOpenCode = agentId === 'opencode'
  const isPi = agentId === 'pi'
  const isNative = isOpenCode || isPi
  const nativeAgentId: NativeCliAgentId = isPi ? 'pi' : 'opencode'
  const codexInitial = isCodex ? initialCodexTomlAuth(initial) : null
  const [name, setName] = useState(initial?.name ?? '')
  const [remark, setRemark] = useState(initial?.remark ?? '')
  const [env, setEnv] = useState<EnvPair[]>(() =>
    isClaude ? initialClaudeEnv(initial) : (initial?.env ?? []),
  )
  const [configToml, setConfigToml] = useState(codexInitial?.configToml ?? initial?.configToml ?? '')
  const [authJson, setAuthJson] = useState(codexInitial?.authJson ?? initial?.authJson ?? '')
  const [nativeForm, setNativeForm] = useState(() =>
    readNativeCliProvider(nativeAgentId, isNative ? initial : null),
  )
  const [showRaw, setShowRaw] = useState(!isClaude)
  const [showKey, setShowKey] = useState(false)
  const [error, setError] = useState('')
  const [fetchedModels, setFetchedModels] = useState<string[]>([])
  const [fetching, setFetching] = useState(false)
  const [fetchNote, setFetchNote] = useState('')
  // null = 跟着上面的字段实时生成；非 null = 用户正在直接编辑这段 JSON。
  const [jsonDraft, setJsonDraft] = useState<string | null>(null)
  const [jsonError, setJsonError] = useState('')
  const [showJson, setShowJson] = useState(isClaude)
  const [expandedNativeModels, setExpandedNativeModels] = useState<Set<number>>(() => new Set())
  const [activePreset, setActivePreset] = useState<ClaudePresetId>(() =>
    isClaude
      ? detectClaudePresetId(envPairsToRecord(initialClaudeEnv(initial)))
      : CUSTOM_PROXY_PRESET_ID,
  )
  const [codexPreset, setCodexPreset] = useState<CodexPresetId>(() =>
    isCodex ? detectCodexPresetId(codexInitial?.configToml ?? initial?.configToml ?? '') : CODEX_CUSTOM_PRESET_ID,
  )
  // Codex 高级区：默认收起，日常只改 URL / Key / 模型
  const [showCodexAdvanced, setShowCodexAdvanced] = useState(false)

  const patchEnv = (key: string, value: string) => {
    setEnv((prev) => writeEnv(prev, key, value))
    setJsonDraft(null)
  }
  const baseUrl = readEnv(env, CLAUDE_BASE_URL)
  const modelSelectOptions = useMemo<SelectOption[]>(
    () => fetchedModels.map((model) => ({ value: model, label: model })),
    [fetchedModels],
  )
  const nativeModelOptions = useMemo<SelectOption[]>(
    () => normalizeNativeModels(nativeForm.models).map((model) => ({
      value: model.id,
      label: isPi ? resolvePiModelMetadata(model).displayName : (model.name || model.id),
    })),
    [isPi, nativeForm.models],
  )
  const defaultPiModel = useMemo(
    () => nativeForm.models.find((model) => model.id.trim() === nativeForm.defaultModel.trim()),
    [nativeForm.defaultModel, nativeForm.models],
  )
  const piThinkingOptions = useMemo(
    () => piThinkingOptionsForModel(defaultPiModel),
    [defaultPiModel],
  )

  const updateNativeModelId = (idx: number, id: string) => {
    setNativeForm((prev) => {
      const previousId = prev.models[idx]?.id ?? ''
      const models = prev.models.map((item, i) => i === idx ? { ...item, id } : item)
      if (prev.defaultModel.trim() && prev.defaultModel !== previousId) {
        return { ...prev, models }
      }
      return {
        ...prev,
        models,
        defaultModel: id,
        defaultThinkingLevel: isPi ? recommendedPiThinkingLevel(models[idx]) : prev.defaultThinkingLevel,
      }
    })
  }

  const updatePiModelReasoning = (idx: number, reasoning: boolean | null) => {
    setNativeForm((prev) => {
      const models = prev.models.map((item, i) => i === idx ? { ...item, reasoning } : item)
      const updated = models[idx]
      if (!updated || updated.id.trim() !== prev.defaultModel.trim()) return { ...prev, models }
      const supported = piThinkingOptionsForModel(updated)
      const current = prev.defaultThinkingLevel as PiThinkingLevel
      return {
        ...prev,
        models,
        defaultThinkingLevel: supported.includes(current)
          ? current
          : recommendedPiThinkingLevel(updated),
      }
    })
  }

  const jsonText =
    jsonDraft ??
    JSON.stringify({ env: Object.fromEntries(env.map((pair) => [pair.key, pair.value])) }, null, 2)

  /** JSON → 字段。解析失败只提示，不拦着用户继续敲。 */
  const onJsonChange = (text: string) => {
    setJsonDraft(text)
    try {
      const parsed = JSON.parse(text)
      const nextEnv = parsed?.env
      if (!nextEnv || typeof nextEnv !== 'object' || Array.isArray(nextEnv)) {
        setJsonError(t.externalAgentsProviderJsonShape)
        return
      }
      const pairs = Object.entries(nextEnv).map(([key, value]) => ({
        key,
        value: String(value ?? ''),
      }))
      setEnv(pairs)
      setActivePreset(detectClaudePresetId(envPairsToRecord(pairs)))
      setJsonError('')
    } catch {
      setJsonError(t.externalAgentsProviderJsonInvalid)
    }
  }

  const formatJson = () => {
    try {
      const parsed = JSON.parse(jsonText)
      setJsonDraft(JSON.stringify(parsed, null, 2))
      setJsonError('')
    } catch {
      setJsonError(t.externalAgentsProviderJsonInvalid)
    }
  }

  const applyPreset = (presetId: ClaudePresetId) => {
    setActivePreset(presetId)
    setFetchedModels([])
    setFetchNote('')
    setJsonDraft(null)
    setJsonError('')
    setEnv((prev) => applyClaudePreset(presetId, prev))

    // 新建时若名称还空，用预设名填一格（自定义除外）
    if (!initial && !name.trim() && presetId !== CUSTOM_PROXY_PRESET_ID) {
      const preset = CLAUDE_RELAY_PRESETS.find((p) => p.id === presetId)
      if (preset) setName(presetLabel(t, preset.nameKey))
    }
  }

  const applyCodexPresetClick = (presetId: CodexPresetId) => {
    setCodexPreset(presetId)
    setError('')
    setFetchedModels([])
    setFetchNote('')
    const applied = applyCodexPreset(presetId, authJson)
    setConfigToml(applied.configToml)
    setAuthJson(applied.authJson)
    // 新建：名称空，或仍是某个预设显示名时，跟切换走
    if (!initial && presetId !== CODEX_CUSTOM_PRESET_ID) {
      const nextLabel = presetLabel(
        t,
        CODEX_PRESET_BUTTONS.find((p) => p.id === presetId)?.nameKey ?? '',
      )
      const wasPresetName = CODEX_PRESET_BUTTONS.some(
        (p) =>
          p.id !== CODEX_CUSTOM_PRESET_ID
          && (name === p.name || name === presetLabel(t, p.nameKey)),
      )
      if (!name.trim() || wasPresetName) setName(nextLabel)
    }
  }

  const patchCodexFields = (patch: { baseUrl?: string; model?: string; apiKey?: string }) => {
    const next = setCodexStructuredFields(configToml, authJson, patch)
    setConfigToml(next.configToml)
    setAuthJson(next.authJson)
    if (patch.baseUrl !== undefined) {
      setCodexPreset(detectCodexPresetId(next.configToml))
    }
  }

  const formatAuthJson = () => {
    try {
      const parsed = JSON.parse(authJson)
      setAuthJson(JSON.stringify(parsed, null, 2))
      setError('')
    } catch {
      setError(t.externalAgentsProviderAuthInvalid)
    }
  }

  const codexBaseUrl = isCodex ? extractCodexBaseUrl(configToml) : ''
  const codexModel = isCodex ? (extractCodexModel(configToml) || 'gpt-5.5') : ''
  const codexApiKey = isCodex ? extractOpenAiApiKey(authJson) : ''

  const fetchModels = async () => {
    const url = isCodex ? codexBaseUrl : isNative ? nativeForm.baseUrl : baseUrl
    const key = isCodex ? codexApiKey : isNative ? nativeForm.apiKey : readClaudeApiKey(env)
    setFetching(true)
    setFetchNote('')
    try {
      const models = await chatApi.externalCliFetchRelayModels(url, key)
      setFetchedModels(models)
      if (isNative && models.length > 0 && normalizeNativeModels(nativeForm.models).length === 0) {
        setNativeForm((prev) => ({
          ...prev,
          models: [emptyNativeModel(nativeAgentId, models[0])],
          defaultModel: prev.defaultModel || models[0],
        }))
      }
      setFetchNote(
        models.length === 0
          ? t.externalAgentsProviderModelsEmpty
          : t.externalAgentsProviderModelsFetched.replace('{count}', String(models.length)),
      )
    } catch (err) {
      setFetchedModels([])
      setFetchNote(String(err))
    } finally {
      setFetching(false)
    }
  }

  const handleSave = () => {
    if (!name.trim()) {
      setError(t.externalAgentsProviderNameRequired)
      return
    }
    if (isClaude) {
      if (!baseUrl.trim()) {
        setError(t.externalAgentsProviderUrlRequired)
        return
      }
      if (!readClaudeApiKey(env).trim()) {
        setError(t.externalAgentsProviderKeyRequired)
        return
      }
      // 保存前统一 Key 键名（API_KEY → AUTH_TOKEN）
      const normalizedEnv = writeClaudeApiKey(env, readClaudeApiKey(env))
        .filter((pair) => pair.key.trim())
      if (normalizedEnv.length === 0) {
        setError(t.externalAgentsProviderEnvRequired)
        return
      }
      onSave({
        id: initial?.id || `p-${Date.now().toString(36)}`,
        name: name.trim(),
        remark: remark.trim(),
        env: normalizedEnv,
        configToml: '',
        authJson: '',
      })
      return
    }
    if (isCodex) {
      if (!codexBaseUrl.trim()) {
        setError(t.externalAgentsProviderUrlRequired)
        return
      }
      if (!codexApiKey.trim()) {
        setError(t.externalAgentsProviderKeyRequired)
        return
      }
      if (!configToml.trim()) {
        setError(t.externalAgentsProviderTomlRequired)
        return
      }
      const tomlErr = validateCodexConfigToml(configToml)
      if (tomlErr) {
        setError(t.externalAgentsProviderTomlInvalid)
        return
      }
      if (authJson.trim()) {
        try {
          JSON.parse(authJson)
        } catch {
          setError(t.externalAgentsProviderAuthInvalid)
          return
        }
      }
      onSave({
        id: initial?.id || `p-${Date.now().toString(36)}`,
        name: name.trim(),
        remark: remark.trim(),
        env: [],
        configToml: configToml,
        authJson: authJson.trim(),
      })
      return
    }
    if (isNative) {
      if (!nativeForm.baseUrl.trim()) {
        setError(t.externalAgentsProviderUrlRequired)
        return
      }
      if (!nativeForm.apiKey.trim()) {
        setError(t.externalAgentsProviderKeyRequired)
        return
      }
      const rawModelIds = nativeForm.models.map((model) => model.id.trim()).filter(Boolean)
      const models = normalizeNativeModels(nativeForm.models)
      if (models.length === 0) {
        setError(t.externalAgentsNativeModelsRequired)
        return
      }
      if (new Set(rawModelIds).size !== rawModelIds.length) {
        setError(t.externalAgentsNativeModelsDuplicate)
        return
      }
      if (isPi && models.some((model) => (
        (model.contextWindow !== '' && !isPositiveInteger(model.contextWindow))
        || (model.maxTokens !== '' && !isPositiveInteger(model.maxTokens))
      ))) {
        setError(t.externalAgentsPiTokenLimitsInvalid)
        return
      }
      if (!models.some((model) => model.id === nativeForm.defaultModel.trim())) {
        setError(t.externalAgentsNativeDefaultInvalid)
        return
      }
      const defaultNativeModel = models.find((model) => model.id === nativeForm.defaultModel.trim())
      if (
        isPi
        && defaultNativeModel
        && !piThinkingOptionsForModel(defaultNativeModel).includes(
          nativeForm.defaultThinkingLevel as PiThinkingLevel,
        )
      ) {
        setError(t.externalAgentsPiDefaultReasoningRequired)
        return
      }
      const native = buildNativeCliProvider(nativeAgentId, name.trim(), {
        ...nativeForm,
        models,
      })
      onSave({
        id: initial?.id || `p-${Date.now().toString(36)}`,
        name: name.trim(),
        remark: remark.trim(),
        env: [],
        configToml: '',
        ...native,
      })
      return
    }
    if (env.filter((pair) => pair.key.trim()).length === 0) {
      setError(t.externalAgentsProviderEnvRequired)
      return
    }
    onSave({
      id: initial?.id || `p-${Date.now().toString(36)}`,
      name: name.trim(),
      remark: remark.trim(),
      env: env.filter((pair) => pair.key.trim()),
      configToml: '',
      authJson: '',
    })
  }

  const title = (initial ? t.externalAgentsProviderEditTitle : t.externalAgentsProviderAddTitle).replace(
    '{name}',
    agentName,
  )

  const relayPresets = useMemo(
    () => [CLAUDE_CUSTOM_PRESET, ...CLAUDE_RELAY_PRESETS],
    [],
  )
  const codexPresets = CODEX_PRESET_BUTTONS

  const envEditor = (
    <div className="space-y-2">
      {env.map((pair, idx) => (
        <div key={idx} className="flex items-center gap-2">
          <Input
            value={pair.key}
            onChange={(value) => {
              setEnv((prev) => prev.map((p, i) => (i === idx ? { ...p, key: value } : p)))
              setJsonDraft(null)
            }}
            placeholder={t.externalAgentsEnvKey}
            mono
          />
          <Input
            value={pair.value}
            onChange={(value) => {
              setEnv((prev) => prev.map((p, i) => (i === idx ? { ...p, value } : p)))
              setJsonDraft(null)
            }}
            placeholder={t.externalAgentsEnvValue}
            mono
          />
          <IconButton
            size="sm"
            label={t.externalAgentsRemove}
            onClick={() => {
              setEnv((prev) => prev.filter((_, i) => i !== idx))
              setJsonDraft(null)
            }}
          >
            <Trash2 size={13} />
          </IconButton>
        </div>
      ))}
      <Button
        size="sm"
        onClick={() => {
          setEnv((prev) => [...prev, { key: '', value: '' }])
          setJsonDraft(null)
        }}
      >
        <Plus size={12} />
        {t.externalAgentsEnvAdd}
      </Button>
    </div>
  )

  const claudeBody = (
    <>
      <div className="kv-preset-group">
        <div className="kv-preset-title">{t.externalAgentsPresetRelaySection}</div>
        <div className="kv-preset-buttons">
          {relayPresets.map((preset) => (
            <button
              key={preset.id}
              type="button"
              className={`kv-preset-btn ${activePreset === preset.id ? 'active' : ''}`}
              onClick={() => applyPreset(preset.id)}
              data-tauri-drag-region="false"
            >
              <span className="kv-preset-btn-icon" aria-hidden>
                <PresetIcon brand={preset.brand} />
              </span>
              {presetLabel(t, preset.nameKey)}
            </button>
          ))}
        </div>
        <p className="kv-row-desc">{t.externalAgentsPresetRelayHint}</p>
      </div>

      <div className="kv-form-grid">
        <div className="kv-form-block">
          <FieldLabel text={t.externalAgentsProviderName} required />
          <Input value={name} onChange={setName} placeholder={t.externalAgentsProviderNamePlaceholder} />
        </div>
        <div className="kv-form-block">
          <FieldLabel text={t.externalAgentsProviderRemark} />
          <Input value={remark} onChange={setRemark} placeholder={t.externalAgentsProviderRemarkHint} />
        </div>
      </div>

      <div className="kv-form-grid">
        <div className="kv-form-block">
          <FieldLabel text={t.externalAgentsProviderApiUrl} required />
          <Input
            value={baseUrl}
            onChange={(value) => {
              patchEnv(CLAUDE_BASE_URL, value)
              setActivePreset(detectClaudePresetId({ ANTHROPIC_BASE_URL: value }))
            }}
            mono
            placeholder="https://example.com/anthropic"
          />
          <p className="kv-row-desc">{t.externalAgentsProviderApiUrlHint}</p>
        </div>
        <div className="kv-form-block">
          <FieldLabel text={t.externalAgentsProviderApiKey} required />
          <div className="kv-key-field">
            <Input
              value={readClaudeApiKey(env)}
              onChange={(value) => {
                setEnv((prev) => writeClaudeApiKey(prev, value))
                setJsonDraft(null)
              }}
              type={showKey ? 'text' : 'password'}
              mono
              placeholder="sk-…"
            />
            <IconButton
              size="sm"
              label={showKey ? t.externalAgentsProviderHideKey : t.externalAgentsProviderShowKey}
              onClick={() => setShowKey((prev) => !prev)}
            >
              {showKey ? <EyeOff size={13} /> : <Eye size={13} />}
            </IconButton>
          </div>
          <p className="kv-row-desc">{t.externalAgentsProviderApiKeyHint}</p>
        </div>
      </div>

      <div className="kv-form-block">
        <div className="kv-field-row">
          <FieldLabel text={t.externalAgentsProviderModels} />
          <Button size="sm" onClick={() => void fetchModels()} disabled={fetching || !baseUrl.trim()}>
            <RefreshCw size={12} className={fetching ? 'animate-spin' : ''} />
            {fetching ? t.externalAgentsProviderFetchingModels : t.externalAgentsProviderFetchModels}
          </Button>
        </div>
        <div className="kv-tier-grid">
          {CLAUDE_TIER_KEYS.map((tier) => (
            <TierField
              key={tier.key}
              label={tier.label}
              value={readEnv(env, tier.key)}
              onChange={(value) => patchEnv(tier.key, value)}
              modelOptions={modelSelectOptions}
              placeholder={t.externalAgentsProviderModelPlaceholder}
            />
          ))}
        </div>
        <p className={`kv-row-desc ${fetchNote && fetchedModels.length === 0 && fetchNote !== t.externalAgentsProviderModelsEmpty ? 'kv-form-error-inline' : ''}`}>
          {fetchNote || t.externalAgentsProviderModelsHint}
        </p>
      </div>

      <div className="kv-form-block kv-cli-advanced">
        <button
          type="button"
          className="kv-disclosure"
          onClick={() => setShowJson((prev) => !prev)}
          data-tauri-drag-region="false"
        >
          <span className={`kv-disclosure-caret ${showJson ? 'open' : ''}`} />
          {t.externalAgentsProviderJson}
        </button>
        {showJson && (
          <div className="kv-cli-advanced-body">
            <p className="kv-row-desc">{t.externalAgentsProviderJsonHint}</p>
            <div className="kv-field-row">
              <span />
              <Button size="sm" onClick={formatJson}>
                {t.externalAgentsProviderFormatJson}
              </Button>
            </div>
            <TextArea value={jsonText} onChange={onJsonChange} rows={9} mono />
            {jsonError && <p className="kv-form-error-inline">{jsonError}</p>}
          </div>
        )}

        <button
          type="button"
          className="kv-disclosure mt-2"
          onClick={() => setShowRaw((prev) => !prev)}
          data-tauri-drag-region="false"
        >
          <span className={`kv-disclosure-caret ${showRaw ? 'open' : ''}`} />
          {t.externalAgentsProviderRawEnv}
          <span className="kv-disclosure-count">{env.length}</span>
        </button>
        {showRaw && <div className="kv-cli-advanced-body">{envEditor}</div>}
      </div>
    </>
  )

  const codexBody = (
    <>
      <div className="kv-preset-group">
        <div className="kv-preset-title">{t.externalAgentsPresetRelaySection}</div>
        <div className="kv-preset-buttons">
          {codexPresets.map((preset) => (
            <button
              key={preset.id}
              type="button"
              className={`kv-preset-btn ${codexPreset === preset.id ? 'active' : ''}`}
              onClick={() => applyCodexPresetClick(preset.id)}
              data-tauri-drag-region="false"
            >
              <span className="kv-preset-btn-icon" aria-hidden>
                <PresetIcon brand={preset.brand} />
              </span>
              {presetLabel(t, preset.nameKey)}
            </button>
          ))}
        </div>
        <p className="kv-row-desc">{t.externalAgentsCodexPresetHint}</p>
      </div>

      <div className="kv-form-grid">
        <div className="kv-form-block">
          <FieldLabel text={t.externalAgentsProviderName} required />
          <Input value={name} onChange={setName} placeholder={t.externalAgentsProviderNamePlaceholder} />
        </div>
        <div className="kv-form-block">
          <FieldLabel text={t.externalAgentsProviderRemark} />
          <Input value={remark} onChange={setRemark} placeholder={t.externalAgentsProviderRemarkHint} />
        </div>
      </div>

      <div className="kv-form-grid">
        <div className="kv-form-block">
          <FieldLabel text={t.externalAgentsProviderApiUrl} required />
          <Input
            value={codexBaseUrl}
            onChange={(value) => patchCodexFields({ baseUrl: value })}
            mono
            placeholder="https://api.example.com/v1"
          />
          <p className="kv-row-desc">{t.externalAgentsCodexApiUrlHint}</p>
        </div>
        <div className="kv-form-block">
          <FieldLabel text={t.externalAgentsProviderApiKey} required />
          <div className="kv-key-field">
            <Input
              value={codexApiKey}
              onChange={(value) => patchCodexFields({ apiKey: value })}
              type={showKey ? 'text' : 'password'}
              mono
              placeholder="sk-…"
            />
            <IconButton
              size="sm"
              label={showKey ? t.externalAgentsProviderHideKey : t.externalAgentsProviderShowKey}
              onClick={() => setShowKey((prev) => !prev)}
            >
              {showKey ? <EyeOff size={13} /> : <Eye size={13} />}
            </IconButton>
          </div>
          <p className="kv-row-desc">{t.externalAgentsCodexAuthHint}</p>
        </div>
      </div>

      <div className="kv-form-block">
        <div className="kv-field-row">
          <FieldLabel text={t.externalAgentsCodexDefaultModel} />
          <Button size="sm" onClick={() => void fetchModels()} disabled={fetching || !codexBaseUrl.trim()}>
            <RefreshCw size={12} className={fetching ? 'animate-spin' : ''} />
            {fetching ? t.externalAgentsProviderFetchingModels : t.externalAgentsProviderFetchModels}
          </Button>
        </div>
        <SuggestInput
          value={codexModel}
          onChange={(value) => patchCodexFields({ model: value })}
          options={modelSelectOptions}
          placeholder="gpt-5.5"
          mono
          ariaLabel={t.externalAgentsCodexDefaultModel}
        />
        <p className={`kv-row-desc ${fetchNote && fetchedModels.length === 0 && fetchNote !== t.externalAgentsProviderModelsEmpty ? 'kv-form-error-inline' : ''}`}>
          {fetchNote || t.externalAgentsCodexModelHint}
        </p>
      </div>

      <div className="kv-form-block kv-cli-advanced">
        <button
          type="button"
          className="kv-disclosure"
          onClick={() => setShowCodexAdvanced((prev) => !prev)}
          data-tauri-drag-region="false"
        >
          <span className={`kv-disclosure-caret ${showCodexAdvanced ? 'open' : ''}`} />
          {t.externalAgentsCodexAdvanced}
        </button>
        {showCodexAdvanced && (
          <div className="kv-cli-advanced-body">
            <div className="kv-form-block">
              <FieldLabel text="config.toml" required />
              <TextArea
                value={configToml}
                onChange={(value) => {
                  setConfigToml(value)
                  setCodexPreset(detectCodexPresetId(value))
                }}
                rows={10}
                mono
              />
              <p className="kv-row-desc">{t.externalAgentsProviderTomlHint}</p>
            </div>
            <div className="kv-form-block">
              <div className="kv-field-row">
                <FieldLabel text="auth.json" />
                <Button size="sm" onClick={formatAuthJson}>
                  {t.externalAgentsProviderFormatJson}
                </Button>
              </div>
              <TextArea
                value={authJson}
                onChange={setAuthJson}
                rows={4}
                mono
                placeholder={'{\n  "OPENAI_API_KEY": "sk-…"\n}'}
              />
            </div>
          </div>
        )}
      </div>
    </>
  )

  const nativeBody = (
    <div className="kv-native-provider-form">
      <section className="kv-native-section">
        <div className="kv-native-section-head">
          <h4>{t.externalAgentsNativeIdentitySection}</h4>
        </div>
        <div className="kv-form-grid">
          <div className="kv-form-block">
            <FieldLabel text={t.externalAgentsProviderName} required />
            <Input value={name} onChange={setName} placeholder={t.externalAgentsProviderNamePlaceholder} />
          </div>
          <div className="kv-form-block">
            <FieldLabel text={t.externalAgentsProviderRemark} />
            <Input value={remark} onChange={setRemark} placeholder={t.externalAgentsProviderRemarkHint} />
          </div>
        </div>
      </section>

      <section className="kv-native-section">
        <div className="kv-native-section-head">
          <div>
            <h4>{t.externalAgentsNativeConnectionSection}</h4>
            <p>{t.externalAgentsNativeConnectionHint}</p>
          </div>
        </div>
        <div className={`kv-native-connection-grid ${isPi ? 'kv-native-connection-grid--pi' : ''}`}>
          <div className="kv-form-block">
            <FieldLabel text={t.externalAgentsProviderApiUrl} required />
            <Input
              value={nativeForm.baseUrl}
              onChange={(baseUrl) => setNativeForm((prev) => ({ ...prev, baseUrl }))}
              mono
              placeholder="https://api.example.com/v1"
            />
          </div>
          <div className="kv-form-block">
            <FieldLabel text={t.externalAgentsProviderApiKey} required />
            <div className="kv-key-field">
              <Input
                value={nativeForm.apiKey}
                onChange={(apiKey) => setNativeForm((prev) => ({ ...prev, apiKey }))}
                type={showKey ? 'text' : 'password'}
                mono
                placeholder="sk-…"
              />
              <IconButton
                size="sm"
                label={showKey ? t.externalAgentsProviderHideKey : t.externalAgentsProviderShowKey}
                onClick={() => setShowKey((prev) => !prev)}
              >
                {showKey ? <EyeOff size={13} /> : <Eye size={13} />}
              </IconButton>
            </div>
          </div>
          {isPi && (
            <div className="kv-form-block">
              <FieldLabel text={t.externalAgentsNativeProtocol} required />
              <Select
                value={nativeForm.api}
                onChange={(api) => setNativeForm((prev) => ({ ...prev, api }))}
                options={PI_API_OPTIONS.map((api) => ({ value: api, label: api }))}
              />
            </div>
          )}
        </div>
      </section>

      <section className="kv-native-section">
        <div className="kv-native-section-head kv-native-section-head--actions">
          <div>
            <h4>{t.externalAgentsNativeModels}</h4>
            <p>
              {fetchNote && (fetchedModels.length > 0 || fetchNote === t.externalAgentsProviderModelsEmpty)
                ? fetchNote
                : t.externalAgentsNativeModelsHint}
            </p>
          </div>
          <div className="kv-native-model-actions">
            <span>{t.externalAgentsModelsCount.replace('{count}', String(nativeModelOptions.length))}</span>
            <Button
              size="sm"
              onClick={() => void fetchModels()}
              disabled={fetching || !nativeForm.baseUrl.trim()}
            >
              <RefreshCw size={12} className={fetching ? 'animate-spin' : ''} />
              {fetching ? t.externalAgentsProviderFetchingModels : t.externalAgentsProviderFetchModels}
            </Button>
            <IconButton
              size="sm"
              label={t.externalAgentsNativeModelAdd}
              onClick={() => setNativeForm((prev) => ({
                ...prev,
                models: [...prev.models, emptyNativeModel(nativeAgentId)],
              }))}
            >
              <Plus size={13} />
            </IconButton>
          </div>
        </div>

        <div className="kv-native-model-list">
          <div className="kv-native-model-columns" aria-hidden>
            <span />
            <span>{t.externalAgentsNativeModelId}</span>
            <span>{isPi ? t.externalAgentsPiAutoMetadata : t.externalAgentsNativeModelName}</span>
            <span />
          </div>
          {nativeForm.models.map((model, idx) => {
            const metadata = isPi ? resolvePiModelMetadata(model) : null
            const expanded = expandedNativeModels.has(idx)
            const automaticLabel = metadata?.matched
              ? t.externalAgentsPiMetadataMatched
              : t.externalAgentsPiMetadataDefault
            return (
              <div key={idx} className={`kv-native-model-row ${expanded ? 'is-expanded' : ''}`}>
                <div className="kv-native-model-main">
                  <span className="kv-native-model-index">{String(idx + 1).padStart(2, '0')}</span>
                  <SuggestInput
                    value={model.id}
                    onChange={(id) => updateNativeModelId(idx, id)}
                    options={modelSelectOptions}
                    placeholder={t.externalAgentsProviderModelPlaceholder}
                    mono
                    className="min-w-0"
                    ariaLabel={t.externalAgentsNativeModelId}
                  />
                  {isPi && metadata ? (
                    <div className="kv-native-model-summary">
                      <span className={metadata.matched ? 'matched' : ''}>{automaticLabel}</span>
                      <span>{formatTokenLimit(metadata.contextWindow)} {t.externalAgentsPiContextShort}</span>
                      <span>{formatTokenLimit(metadata.maxTokens)} {t.externalAgentsPiOutputShort}</span>
                      {metadata.reasoning && <span>{t.externalAgentsPiReasoningShort}</span>}
                      {metadata.vision && <span>{t.externalAgentsPiVisionShort}</span>}
                    </div>
                  ) : (
                    <Input
                      value={model.name}
                      onChange={(modelName) => setNativeForm((prev) => ({
                        ...prev,
                        models: prev.models.map((item, i) => i === idx ? { ...item, name: modelName } : item),
                      }))}
                      placeholder={t.externalAgentsNativeModelName}
                      className="min-w-0"
                    />
                  )}
                  <div className="kv-native-model-row-actions">
                    {isPi && (
                      <IconButton
                        size="sm"
                        variant="ghost"
                        className={expanded ? 'active' : ''}
                        label={t.externalAgentsPiModelAdvanced}
                        aria-expanded={expanded}
                        onClick={() => setExpandedNativeModels((previous) => {
                          const next = new Set(previous)
                          if (next.has(idx)) next.delete(idx)
                          else next.add(idx)
                          return next
                        })}
                      >
                        <SlidersHorizontal size={13} />
                      </IconButton>
                    )}
                    <IconButton
                      size="sm"
                      label={t.externalAgentsRemove}
                      onClick={() => setNativeForm((prev) => {
                        const removedDefault = prev.models[idx]?.id.trim() === prev.defaultModel.trim()
                        const models = prev.models.length === 1
                          ? [emptyNativeModel(nativeAgentId)]
                          : prev.models.filter((_, i) => i !== idx)
                        if (!removedDefault) return { ...prev, models }
                        const defaultModel = models[0]?.id ?? ''
                        return {
                          ...prev,
                          models,
                          defaultModel,
                          defaultThinkingLevel: isPi
                            ? recommendedPiThinkingLevel(models[0])
                            : prev.defaultThinkingLevel,
                        }
                      })}
                    >
                      <Trash2 size={13} />
                    </IconButton>
                  </div>
                </div>
                {isPi && metadata && expanded && (
                  <div className="kv-native-model-advanced">
                    <div className="kv-native-model-advanced-head">
                      <span>{t.externalAgentsPiModelOverrides}</span>
                      <span>{t.externalAgentsPiModelOverridesHint}</span>
                    </div>
                    <div className="kv-native-model-advanced-grid">
                      <div className="kv-form-block">
                        <FieldLabel text={t.externalAgentsNativeModelName} />
                        <Input
                          value={model.name}
                          onChange={(modelName) => setNativeForm((prev) => ({
                            ...prev,
                            models: prev.models.map((item, i) => i === idx ? { ...item, name: modelName } : item),
                          }))}
                          placeholder={metadata.displayName}
                        />
                      </div>
                      <div className="kv-form-block">
                        <FieldLabel text={t.externalAgentsPiContextWindow} />
                        <Input
                          value={model.contextWindow}
                          onChange={(contextWindow) => setNativeForm((prev) => ({
                            ...prev,
                            models: prev.models.map((item, i) => i === idx ? { ...item, contextWindow } : item),
                          }))}
                          placeholder={String(metadata.contextWindow)}
                          type="number"
                          min="1"
                          step="1"
                          inputMode="numeric"
                          mono
                          aria-label={t.externalAgentsPiContextWindow}
                        />
                      </div>
                      <div className="kv-form-block">
                        <FieldLabel text={t.externalAgentsPiMaxTokens} />
                        <Input
                          value={model.maxTokens}
                          onChange={(maxTokens) => setNativeForm((prev) => ({
                            ...prev,
                            models: prev.models.map((item, i) => i === idx ? { ...item, maxTokens } : item),
                          }))}
                          placeholder={String(metadata.maxTokens)}
                          type="number"
                          min="1"
                          step="1"
                          inputMode="numeric"
                          mono
                          aria-label={t.externalAgentsPiMaxTokens}
                        />
                      </div>
                    </div>
                    <div className="kv-native-model-switches">
                      <label>
                        <Toggle
                          checked={metadata.reasoning}
                          onChange={(reasoning) => updatePiModelReasoning(idx, reasoning)}
                          ariaLabel={t.externalAgentsPiReasoning}
                        />
                        <span>{t.externalAgentsPiReasoning}</span>
                        {model.reasoning === null ? (
                          <small>{t.externalAgentsPiAutomatic}</small>
                        ) : (
                          <IconButton
                            size="xs"
                            variant="ghost"
                            label={t.externalAgentsPiRestoreAutomatic}
                            onClick={() => updatePiModelReasoning(idx, null)}
                          >
                            <RotateCcw size={11} />
                          </IconButton>
                        )}
                      </label>
                      <label>
                        <Toggle
                          checked={metadata.vision}
                          onChange={(vision) => setNativeForm((prev) => ({
                            ...prev,
                            models: prev.models.map((item, i) => i === idx ? { ...item, vision } : item),
                          }))}
                          ariaLabel={t.externalAgentsPiVision}
                        />
                        <span>{t.externalAgentsPiVision}</span>
                        {model.vision === null ? (
                          <small>{t.externalAgentsPiAutomatic}</small>
                        ) : (
                          <IconButton
                            size="xs"
                            variant="ghost"
                            label={t.externalAgentsPiRestoreAutomatic}
                            onClick={() => setNativeForm((prev) => ({
                              ...prev,
                              models: prev.models.map((item, i) => i === idx ? { ...item, vision: null } : item),
                            }))}
                          >
                            <RotateCcw size={11} />
                          </IconButton>
                        )}
                      </label>
                    </div>
                  </div>
                )}
              </div>
            )
          })}
        </div>
        {fetchNote && fetchedModels.length === 0 && fetchNote !== t.externalAgentsProviderModelsEmpty && (
          <p className="kv-form-error-inline">{fetchNote}</p>
        )}
      </section>

      <section className="kv-native-section">
        <div className="kv-native-section-head">
          <div>
            <h4>{t.externalAgentsNativeStartupSection}</h4>
            <p>{t.externalAgentsNativeDefaultHint}</p>
          </div>
        </div>
        <div className={`kv-form-grid ${isPi ? '' : 'kv-form-grid--single'}`}>
          <div className="kv-form-block">
            <FieldLabel text={t.externalAgentsCodexDefaultModel} required />
            <SuggestInput
              value={nativeForm.defaultModel}
              onChange={(defaultModel) => setNativeForm((prev) => {
                const model = prev.models.find((item) => item.id.trim() === defaultModel.trim())
                return {
                  ...prev,
                  defaultModel,
                  defaultThinkingLevel: isPi
                    ? recommendedPiThinkingLevel(model)
                    : prev.defaultThinkingLevel,
                }
              })}
              options={nativeModelOptions}
              placeholder={t.externalAgentsProviderModelPlaceholder}
              mono
              ariaLabel={t.externalAgentsCodexDefaultModel}
            />
          </div>
          {isPi && (
            <div className="kv-form-block">
              <FieldLabel text={t.externalAgentsPiDefaultThinking} required />
              <Select
                value={nativeForm.defaultThinkingLevel}
                onChange={(defaultThinkingLevel) => setNativeForm((prev) => ({
                  ...prev,
                  defaultThinkingLevel,
                }))}
                options={piThinkingOptions.map((level) => ({
                  value: level,
                  label: level === 'off' ? t.externalAgentsPiThinkingOff : level,
                }))}
              />
            </div>
          )}
        </div>
      </section>

      <p className="kv-native-persistence-note">{t.externalAgentsNativeWriteHint}</p>
    </div>
  )

  return createPortal(
    <div
      className="kv-modal-backdrop kv-modal-backdrop--portal"
      data-tauri-drag-region="false"
    >
      <div
        className={`kv kv-modal kv-provider-modal ${isClaude || isCodex || isNative ? 'kv-provider-modal--wide' : ''}`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        data-tauri-drag-region="false"
      >
        <div className="kv-provider-modal-head">
          <h3>{title}</h3>
          <IconButton size="sm" label={t.cancel} onClick={onClose} data-tauri-drag-region="false">
            <X size={15} />
          </IconButton>
        </div>

        <div className="kv-provider-modal-body custom-scrollbar">
          {isClaude ? (
            claudeBody
          ) : isCodex ? (
            codexBody
          ) : isNative ? (
            nativeBody
          ) : (
            <>
              <div className="kv-form-grid">
                <div className="kv-form-block">
                  <FieldLabel text={t.externalAgentsProviderName} required />
                  <Input value={name} onChange={setName} placeholder={t.externalAgentsProviderNamePlaceholder} />
                </div>
                <div className="kv-form-block">
                  <FieldLabel text={t.externalAgentsProviderRemark} />
                  <Input value={remark} onChange={setRemark} placeholder={t.externalAgentsProviderRemarkHint} />
                </div>
              </div>

              <div className="kv-form-block">
                <FieldLabel text={t.externalAgentsProviderRawEnv} required />
                {envEditor}
              </div>
            </>
          )}
        </div>

        <div className="kv-provider-modal-foot">
          {error && <p className="kv-form-error">{error}</p>}
          <Button onClick={onClose} data-tauri-drag-region="false">
            {t.cancel}
          </Button>
          <Button
            variant="primary"
            onClick={handleSave}
            disabled={!name.trim()}
            data-tauri-drag-region="false"
          >
            {initial ? t.save : t.externalAgentsProviderConfirmAdd}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  )
}
