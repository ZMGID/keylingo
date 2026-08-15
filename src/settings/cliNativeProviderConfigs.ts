import type { ExternalCliProvider } from '../api/tauri'
import { matchModelExact } from '../data/modelMatching'

export type NativeCliAgentId = 'opencode' | 'pi' | 'dsh'

export type NativeCliModel = {
  id: string
  name: string
  reasoning: boolean | null
  vision: boolean | null
  toolCall?: boolean | null
  contextWindow: string
  maxTokens: string
  thinkingLevels?: PiThinkingLevel[] | null
}

export type PiThinkingLevel = 'off' | 'minimal' | 'low' | 'medium' | 'high' | 'xhigh' | 'max'

type PiMappedThinkingLevel = Exclude<PiThinkingLevel, 'off'>
type PiThinkingLevelMap = Record<PiMappedThinkingLevel, string | null>

export type ResolvedPiModelMetadata = {
  matched: boolean
  displayName: string
  reasoning: boolean
  vision: boolean
  contextWindow: number
  maxTokens: number
  thinkingLevels: PiThinkingLevel[]
  thinkingLevelMap?: PiThinkingLevelMap
}

export type ResolvedOpenCodeModelMetadata = Omit<ResolvedPiModelMetadata, 'thinkingLevels' | 'thinkingLevelMap'> & {
  toolCall: boolean
}

export type NativeCliProviderForm = {
  nativeProviderId: string
  baseUrl: string
  apiKey: string
  api: string
  models: NativeCliModel[]
  defaultModel: string
  defaultThinkingLevel: string
  sourceConfigJson: string
}

// OpenCode custom providers use AI SDK package names as the wire selector:
// openai-compatible → /v1/chat/completions; openai → /v1/responses.
// See https://opencode.ai/docs/providers/ ("if a model uses /v1/responses, use @ai-sdk/openai").
export const OPENCODE_NPM_OPTIONS = [
  '@ai-sdk/openai-compatible',
  '@ai-sdk/openai',
  '@ai-sdk/anthropic',
  '@ai-sdk/google',
] as const

export const PI_API_OPTIONS = [
  'openai-completions',
  'openai-responses',
  'anthropic-messages',
  'google-generative-ai',
] as const

export const DSH_API_OPTIONS = [
  'openai-completions',
  'openai-responses',
  'anthropic-messages',
] as const

export const PI_THINKING_OPTIONS = [
  'off',
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
] as const

const PI_DEFAULT_REASONING_OPTIONS: PiThinkingLevel[] = ['off', 'minimal', 'low', 'medium', 'high']
const PI_MAPPED_THINKING_OPTIONS: PiMappedThinkingLevel[] = [
  'minimal',
  'low',
  'medium',
  'high',
  'xhigh',
  'max',
]

const PI_DEFAULT_CONTEXT_WINDOW = 128000
const PI_DEFAULT_MAX_TOKENS = 16384
const OPENCODE_DEFAULT_CONTEXT_WINDOW = 128000
const OPENCODE_DEFAULT_MAX_TOKENS = 16384

export function nativeProviderIdFromName(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/^[._-]+|[._-]+$/g, '')
    .replace(/-{2,}/g, '-')
}

export function isValidNativeProviderId(value: string): boolean {
  return /^[a-z0-9](?:[a-z0-9._-]{0,62}[a-z0-9])?$/.test(value)
}

export function dshApiKeyEnv(providerId: string): string {
  const suffix = providerId.trim().toUpperCase().replace(/[^A-Z0-9]+/g, '_') || 'PROVIDER'
  return `KIVIO_DSH_${suffix}_API_KEY`
}

export function emptyNativeModel(agentId: NativeCliAgentId, id = ''): NativeCliModel {
  return {
    id,
    name: '',
    reasoning: null,
    vision: null,
    toolCall: agentId === 'opencode' ? null : undefined,
    contextWindow: '',
    maxTokens: '',
    thinkingLevels: null,
  }
}

export function sanitizeThinkingLevels(levels: readonly string[]): PiThinkingLevel[] {
  return PI_THINKING_OPTIONS.filter((level) => levels.includes(level))
}

export function toggleThinkingLevel(
  current: readonly PiThinkingLevel[],
  level: PiThinkingLevel,
): PiThinkingLevel[] {
  const next = current.includes(level)
    ? current.filter((item) => item !== level)
    : sanitizeThinkingLevels([...current, level])
  return next.length > 0 ? next : [level]
}

function sameThinkingLevels(
  left: readonly PiThinkingLevel[],
  right: readonly PiThinkingLevel[],
): boolean {
  return left.length === right.length && left.every((level, index) => level === right[index])
}

function parseReasoningEffortKeys(value: unknown): PiThinkingLevel[] | null {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return null
  const keys = sanitizeThinkingLevels(Object.keys(value))
  return keys.length > 0 ? keys : null
}

function catalogThinkingLevels(modelId: string, reasoning: boolean): PiThinkingLevel[] {
  if (!reasoning) return ['off']
  const matched = matchModelExact(modelId)
  const catalogLevels = PI_MAPPED_THINKING_OPTIONS.filter((level) =>
    matched?.reasoningEfforts?.includes(level),
  )
  return catalogLevels.length > 0
    ? ['off', ...catalogLevels]
    : PI_DEFAULT_REASONING_OPTIONS
}

function objectValue(text?: string): Record<string, unknown> {
  if (!text?.trim()) return {}
  try {
    const value = JSON.parse(text) as unknown
    return value && typeof value === 'object' && !Array.isArray(value)
      ? value as Record<string, unknown>
      : {}
  } catch {
    return {}
  }
}

function stringValue(value: unknown): string {
  return typeof value === 'string' ? value : ''
}

function positiveIntegerString(value: unknown): string {
  if (typeof value === 'number' && Number.isSafeInteger(value) && value > 0) return String(value)
  if (typeof value === 'string' && /^\d+$/.test(value)) {
    const parsed = Number(value)
    if (Number.isSafeInteger(parsed) && parsed > 0) return value
  }
  return ''
}

function legacyEnv(initial: ExternalCliProvider | null | undefined, suffix: RegExp): string {
  return initial?.env?.find((pair) => suffix.test(pair.key))?.value ?? ''
}

export function normalizeNativeModels(models: NativeCliModel[]): NativeCliModel[] {
  const seen = new Set<string>()
  const normalized: NativeCliModel[] = []
  for (const model of models) {
    const id = model.id.trim()
    if (!id || seen.has(id)) continue
    seen.add(id)
    normalized.push({
      id,
      name: model.name.trim(),
      reasoning: model.reasoning,
      vision: model.vision,
      toolCall: model.toolCall ?? null,
      contextWindow: model.contextWindow.trim(),
      maxTokens: model.maxTokens.trim(),
      thinkingLevels: model.thinkingLevels?.length
        ? sanitizeThinkingLevels(model.thinkingLevels)
        : null,
    })
  }
  return normalized
}

export function resolveOpenCodeModelMetadata(model: NativeCliModel): ResolvedOpenCodeModelMetadata {
  const matched = matchModelExact(model.id)
  const matchedReasoning = matched?.capabilities?.reasoning === true
    || (matched?.reasoningEfforts?.length ?? 0) > 0
  return {
    matched: matched !== null,
    displayName: model.name.trim() || matched?.displayName || model.id.trim(),
    reasoning: model.reasoning ?? matchedReasoning,
    vision: model.vision ?? matched?.capabilities?.vision === true,
    toolCall: model.toolCall ?? matched?.capabilities?.functionCalling !== false,
    contextWindow: Number(model.contextWindow)
      || (matched?.contextWindow && matched.contextWindow > 0
        ? matched.contextWindow
        : OPENCODE_DEFAULT_CONTEXT_WINDOW),
    maxTokens: Number(model.maxTokens)
      || (matched?.maxOutput && matched.maxOutput > 0
        ? matched.maxOutput
        : OPENCODE_DEFAULT_MAX_TOKENS),
  }
}

// Anthropic adaptive-generation Claude 判定：opus ≥4.6 / sonnet ≥4.6 / fable ≥5
// （对齐 pi-cache-optimizer 的 ADAPTIVE_*_PATTERN；容忍 4.6/4-6 两种写法与日期戳、[1M] 等后缀）。
// 这些模型走 adaptive thinking（thinking.type:"adaptive"），pi 的自定义 anthropic-messages
// 渠道必须显式 compat.forceAdaptiveThinking:true，否则 pi 发旧版 budget thinking、
// 严格上游直接拒（pi models.md「Anthropic Messages Compatibility」；pi-cache-optimizer
// 对缺失该 compat 的渠道也会弹告警）。
const PI_ADAPTIVE_CLAUDE_PATTERN =
  /(^|[/\s:_-])(opus-4[.-][6-9]|opus-4-[1-9][0-9]|opus-([5-9]|[1-9][0-9])|sonnet-4[.-][6-9]|sonnet-4-[1-9][0-9]|sonnet-([5-9]|[1-9][0-9])|fable-([5-9]|[1-9][0-9]))($|[-_.:/\s[])/i

export function isPiAdaptiveThinkingModel(modelId: string): boolean {
  return PI_ADAPTIVE_CLAUDE_PATTERN.test(modelId)
}

export function resolvePiModelMetadata(model: NativeCliModel): ResolvedPiModelMetadata {
  const matched = matchModelExact(model.id)
  const matchedReasoning = matched?.capabilities?.reasoning === true
    || (matched?.reasoningEfforts?.length ?? 0) > 0
  const matchedContextWindow = matched?.contextWindow && matched.contextWindow > 0
    ? matched.contextWindow
    : PI_DEFAULT_CONTEXT_WINDOW
  const matchedMaxTokens = matched?.maxOutput && matched.maxOutput > 0
    ? matched.maxOutput
    : PI_DEFAULT_MAX_TOKENS

  const reasoning = model.reasoning ?? matchedReasoning
  const overrideLevels = model.thinkingLevels?.length
    ? sanitizeThinkingLevels(model.thinkingLevels)
    : []
  const thinkingLevels: PiThinkingLevel[] = !reasoning
    ? ['off']
    : overrideLevels.length > 0
      ? overrideLevels
      : catalogThinkingLevels(model.id, true)
  const matchedCatalogLevels = PI_MAPPED_THINKING_OPTIONS.filter((level) =>
    matched?.reasoningEfforts?.includes(level),
  )
  const thinkingLevelMap = reasoning && matchedCatalogLevels.length > 0
    ? Object.fromEntries(PI_MAPPED_THINKING_OPTIONS.map((level) => [
        level,
        matchedCatalogLevels.includes(level) ? level : null,
      ])) as PiThinkingLevelMap
    : undefined

  return {
    matched: matched !== null,
    displayName: model.name.trim() || matched?.displayName || model.id.trim(),
    reasoning,
    vision: model.vision ?? matched?.capabilities?.vision === true,
    contextWindow: Number(model.contextWindow) || matchedContextWindow,
    maxTokens: Number(model.maxTokens) || matchedMaxTokens,
    thinkingLevels,
    thinkingLevelMap,
  }
}

export function piThinkingOptionsForModel(model?: NativeCliModel): PiThinkingLevel[] {
  return model ? resolvePiModelMetadata(model).thinkingLevels : ['off']
}

export function recommendedPiThinkingLevel(model?: NativeCliModel): PiThinkingLevel {
  const levels = piThinkingOptionsForModel(model)
  if (levels.includes('high')) return 'high'
  return levels.at(-1) ?? 'off'
}

function readPersistedThinkingLevels(value: unknown): PiThinkingLevel[] | null {
  const raw = Array.isArray(value)
    ? value.filter((item): item is string => typeof item === 'string')
    : typeof value === 'string'
      ? value.split(',')
      : []
  const levels = sanitizeThinkingLevels(raw)
  return levels.length > 0 ? levels : null
}

function readPersistedPiModel(value: unknown, id: string): NativeCliModel {
  const item = value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {}
  return {
    id,
    name: stringValue(item.name),
    reasoning: typeof item.reasoning === 'boolean' ? item.reasoning : null,
    vision: typeof item.vision === 'boolean' ? item.vision : null,
    toolCall: typeof item.toolCall === 'boolean' ? item.toolCall : null,
    contextWindow: positiveIntegerString(item.contextWindow),
    maxTokens: positiveIntegerString(item.maxTokens),
    thinkingLevels: readPersistedThinkingLevels(item.thinkingLevels),
  }
}

function parseModelMetadata(text?: string): Record<string, unknown> | null {
  if (!text?.trim()) return null
  try {
    const value = JSON.parse(text) as Record<string, unknown>
    if (value?.version !== 1 || !value.models || typeof value.models !== 'object' || Array.isArray(value.models)) {
      return null
    }
    return value.models as Record<string, unknown>
  } catch {
    return null
  }
}

function readLegacyOpenCodeModel(item: Record<string, unknown>, id: string): NativeCliModel {
  const automatic = resolveOpenCodeModelMetadata(emptyNativeModel('opencode', id))
  const limit = item.limit && typeof item.limit === 'object' && !Array.isArray(item.limit)
    ? item.limit as Record<string, unknown>
    : {}
  const modalities = item.modalities && typeof item.modalities === 'object' && !Array.isArray(item.modalities)
    ? item.modalities as Record<string, unknown>
    : {}
  const input = Array.isArray(modalities.input)
    ? modalities.input.filter((value): value is string => typeof value === 'string')
    : null
  const configuredName = stringValue(item.name)
  const configuredContextWindow = positiveIntegerString(limit.context)
  const configuredMaxTokens = positiveIntegerString(limit.output)
  const configuredReasoning = typeof item.reasoning === 'boolean' ? item.reasoning : null
  const configuredVision = typeof item.attachment === 'boolean'
    ? item.attachment
    : input ? input.includes('image') : null
  const configuredToolCall = typeof item.tool_call === 'boolean' ? item.tool_call : null
  return {
    id,
    name: configuredName && configuredName !== id && configuredName !== automatic.displayName
      ? configuredName
      : '',
    reasoning: configuredReasoning !== null && configuredReasoning !== automatic.reasoning
      ? configuredReasoning
      : null,
    vision: configuredVision !== null && configuredVision !== automatic.vision
      ? configuredVision
      : null,
    toolCall: configuredToolCall !== null && configuredToolCall !== automatic.toolCall
      ? configuredToolCall
      : null,
    contextWindow: configuredContextWindow && Number(configuredContextWindow) !== automatic.contextWindow
      ? configuredContextWindow
      : '',
    maxTokens: configuredMaxTokens && Number(configuredMaxTokens) !== automatic.maxTokens
      ? configuredMaxTokens
      : '',
  }
}

function readLegacyPiModel(item: Record<string, unknown>, id: string): NativeCliModel {
  const automatic = resolvePiModelMetadata(emptyNativeModel('pi', id))
  const configuredName = stringValue(item.name)
  const configuredContextWindow = positiveIntegerString(item.contextWindow)
  const configuredMaxTokens = positiveIntegerString(item.maxTokens)
  const configuredReasoning = typeof item.reasoning === 'boolean' ? item.reasoning : null
  const configuredInput = Array.isArray(item.input)
    ? item.input.filter((value): value is string => typeof value === 'string')
    : null
  const configuredVision = configuredInput
    ? configuredInput.includes('image')
    : null

  return {
    id,
    name: configuredName && configuredName !== id && configuredName !== automatic.displayName
      ? configuredName
      : '',
    reasoning: configuredReasoning !== null && configuredReasoning !== automatic.reasoning
      ? configuredReasoning
      : null,
    vision: configuredVision !== null && configuredVision !== automatic.vision
      ? configuredVision
      : null,
    contextWindow: configuredContextWindow
      && Number(configuredContextWindow) !== automatic.contextWindow
      ? configuredContextWindow
      : '',
    maxTokens: configuredMaxTokens && Number(configuredMaxTokens) !== automatic.maxTokens
      ? configuredMaxTokens
      : '',
  }
}

function readLegacyDshModel(item: Record<string, unknown>, id: string): NativeCliModel {
  const automatic = resolvePiModelMetadata(emptyNativeModel('dsh', id))
  const configuredName = stringValue(item.name)
  const configuredContextWindow = positiveIntegerString(item.contextWindow)
  const configuredMaxTokens = positiveIntegerString(item.maxTokens)
  const configuredInput = Array.isArray(item.input)
    ? item.input.filter((value): value is string => typeof value === 'string')
    : null
  const configuredVision = configuredInput ? configuredInput.includes('image') : null
  const efforts = item.reasoningEfforts
  const configuredReasoning = efforts === false
    ? false
    : efforts && typeof efforts === 'object' && !Array.isArray(efforts)
      ? Object.keys(efforts).length > 0
      : null
  const parsedEfforts = parseReasoningEffortKeys(efforts)
  const automaticLevels = catalogThinkingLevels(id, configuredReasoning ?? automatic.reasoning)
  return {
    id,
    name: configuredName && configuredName !== id && configuredName !== automatic.displayName
      ? configuredName
      : '',
    reasoning: configuredReasoning !== null && configuredReasoning !== automatic.reasoning
      ? configuredReasoning
      : null,
    vision: configuredVision !== null && configuredVision !== automatic.vision
      ? configuredVision
      : null,
    contextWindow: configuredContextWindow
      && Number(configuredContextWindow) !== automatic.contextWindow
      ? configuredContextWindow
      : '',
    maxTokens: configuredMaxTokens && Number(configuredMaxTokens) !== automatic.maxTokens
      ? configuredMaxTokens
      : '',
    thinkingLevels: parsedEfforts && !sameThinkingLevels(parsedEfforts, automaticLevels)
      ? parsedEfforts
      : null,
  }
}

export function readNativeCliProvider(
  agentId: NativeCliAgentId,
  initial?: ExternalCliProvider | null,
): NativeCliProviderForm {
  const config = objectValue(initial?.configJson)
  const auth = objectValue(initial?.authJson)
  const nativeProviderId = initial?.nativeProviderId?.trim()
    || nativeProviderIdFromName(initial?.name ?? '')
    || ''
  let baseUrl = ''
  let api = 'openai-completions'
  let models: NativeCliModel[] = []
  const persistedModels = parseModelMetadata(initial?.modelMetadataJson)

  if (agentId === 'opencode') {
    const options = config.options && typeof config.options === 'object' && !Array.isArray(config.options)
      ? config.options as Record<string, unknown>
      : {}
    baseUrl = stringValue(options.baseURL)
    api = stringValue(config.npm) || '@ai-sdk/openai-compatible'
    const rawModels = config.models && typeof config.models === 'object' && !Array.isArray(config.models)
      ? config.models as Record<string, unknown>
      : {}
    models = Object.entries(rawModels).map(([id, value]) => {
      const item = value && typeof value === 'object' && !Array.isArray(value)
        ? value as Record<string, unknown>
        : {}
      return persistedModels !== null
        ? readPersistedPiModel(persistedModels[id], id)
        : readLegacyOpenCodeModel(item, id)
    })
  } else {
    const isDsh = agentId === 'dsh'
    baseUrl = stringValue(isDsh ? config.baseURL : config.baseUrl)
    api = PI_API_OPTIONS.includes(config.api as typeof PI_API_OPTIONS[number])
      ? config.api as string
      : 'openai-completions'
    models = Array.isArray(config.models)
      ? config.models.flatMap((value) => {
          if (!value || typeof value !== 'object' || Array.isArray(value)) return []
          const item = value as Record<string, unknown>
          const id = stringValue(item.id)
          if (!id) return []
          const persisted = persistedModels !== null
            ? readPersistedPiModel(persistedModels[id], id)
            : null
          const fromConfig = isDsh ? readLegacyDshModel(item, id) : readLegacyPiModel(item, id)
          if (!persisted) return [fromConfig]
          return [{
            ...persisted,
            reasoning: persisted.reasoning ?? fromConfig.reasoning,
            vision: persisted.vision ?? fromConfig.vision,
            thinkingLevels: persisted.thinkingLevels ?? fromConfig.thinkingLevels,
          }]
        })
      : []
  }

  baseUrl ||= legacyEnv(initial, /BASE_URL$/i)
  const configuredKeyEnv = stringValue(config.apiKeyEnv)
  const apiKey = agentId === 'dsh'
    ? initial?.env?.find((pair) => pair.key === (configuredKeyEnv || dshApiKeyEnv(nativeProviderId)))?.value ?? ''
    : stringValue(auth.key) || legacyEnv(initial, /(API_KEY|AUTH_TOKEN)$/i)
  const normalized = normalizeNativeModels(models)
  const defaultModel = initial?.defaultModel?.trim() || normalized[0]?.id || ''
  const defaultPiModel = normalized.find((model) => model.id === defaultModel)
  const storedThinkingLevel = PI_THINKING_OPTIONS.includes(
    initial?.defaultReasoning as typeof PI_THINKING_OPTIONS[number],
  )
    ? initial?.defaultReasoning as PiThinkingLevel
    : null
  const supportedThinkingLevels = piThinkingOptionsForModel(defaultPiModel)
  return {
    nativeProviderId,
    baseUrl,
    apiKey,
    api,
    models: normalized.length ? normalized : [emptyNativeModel(agentId)],
    defaultModel,
    defaultThinkingLevel: agentId === 'pi'
      ? storedThinkingLevel && supportedThinkingLevels.includes(storedThinkingLevel)
        ? storedThinkingLevel
        : recommendedPiThinkingLevel(defaultPiModel)
      : 'off',
    sourceConfigJson: initial?.configJson ?? '',
  }
}

function serializeModelMetadata(models: NativeCliModel[]): string {
  const entries = models.flatMap((model) => {
    const metadata: Record<string, string | boolean | string[]> = {}
    if (model.name) metadata.name = model.name
    if (model.reasoning !== null) metadata.reasoning = model.reasoning
    if (model.vision !== null) metadata.vision = model.vision
    if (model.toolCall !== undefined && model.toolCall !== null) metadata.toolCall = model.toolCall
    if (model.contextWindow) metadata.contextWindow = model.contextWindow
    if (model.maxTokens) metadata.maxTokens = model.maxTokens
    if (model.thinkingLevels?.length) metadata.thinkingLevels = model.thinkingLevels
    return Object.keys(metadata).length > 0 ? [[model.id, metadata] as const] : []
  })
  return JSON.stringify({ version: 1, models: Object.fromEntries(entries) }, null, 2)
}

export function buildNativeCliProvider(
  agentId: NativeCliAgentId,
  name: string,
  form: NativeCliProviderForm,
): Pick<ExternalCliProvider, 'configJson' | 'authJson' | 'modelMetadataJson' | 'defaultModel' | 'defaultReasoning'> {
  const models = normalizeNativeModels(form.models)
  const sourceConfig = objectValue(form.sourceConfigJson)
  const config = agentId === 'opencode'
    ? (() => {
        const sourceOptions = sourceConfig.options && typeof sourceConfig.options === 'object'
          && !Array.isArray(sourceConfig.options)
          ? sourceConfig.options as Record<string, unknown>
          : {}
        const sourceModels = sourceConfig.models && typeof sourceConfig.models === 'object'
          && !Array.isArray(sourceConfig.models)
          ? sourceConfig.models as Record<string, unknown>
          : {}
        const baseURL = form.baseUrl.trim()
        const options = { ...sourceOptions }
        if (baseURL) options.baseURL = baseURL
        else delete options.baseURL
        return {
          ...sourceConfig,
          npm: form.api.trim() || '@ai-sdk/openai-compatible',
          name,
          options,
          models: Object.fromEntries(models.map((model) => {
            const resolved = resolveOpenCodeModelMetadata(model)
            const existing = sourceModels[model.id] && typeof sourceModels[model.id] === 'object'
              && !Array.isArray(sourceModels[model.id])
              ? sourceModels[model.id] as Record<string, unknown>
              : {}
            const existingLimit = existing.limit && typeof existing.limit === 'object'
              && !Array.isArray(existing.limit)
              ? existing.limit as Record<string, unknown>
              : {}
            const existingModalities = existing.modalities && typeof existing.modalities === 'object'
              && !Array.isArray(existing.modalities)
              ? existing.modalities as Record<string, unknown>
              : {}
            return [model.id, {
              ...existing,
              name: resolved.displayName,
              reasoning: resolved.reasoning,
              attachment: resolved.vision,
              tool_call: resolved.toolCall,
              limit: {
                ...existingLimit,
                context: resolved.contextWindow,
                output: resolved.maxTokens,
              },
              modalities: {
                ...existingModalities,
                input: resolved.vision ? ['text', 'image'] : ['text'],
                output: Array.isArray(existingModalities.output) ? existingModalities.output : ['text'],
              },
            }]
          })),
        }
      })()
    : agentId === 'dsh'
      ? (() => {
          const resolvedModels = models.map((model) => {
            const resolved = resolvePiModelMetadata(model)
            return {
              id: model.id,
              name: resolved.displayName,
              contextWindow: resolved.contextWindow,
              maxTokens: resolved.maxTokens,
              input: resolved.vision ? ['text', 'image'] : ['text'],
              reasoningEfforts: resolved.reasoning
                ? Object.fromEntries(resolved.thinkingLevels.map((level) => [
                    level,
                    level === 'off' ? null : level,
                  ]))
                : false,
            }
          })
          return {
            ...sourceConfig,
            displayName: name,
            apiKeyEnv: dshApiKeyEnv(form.nativeProviderId),
            api: form.api,
            baseURL: form.baseUrl.trim(),
            defaultInput: resolvedModels.some((model) => model.input.includes('image'))
              ? ['text', 'image']
              : ['text'],
            models: resolvedModels,
          }
        })()
      : {
          name,
          baseUrl: form.baseUrl.trim(),
          api: form.api,
          models: models.map((model) => {
            const resolved = resolvePiModelMetadata(model)
            return {
              id: model.id,
              name: resolved.displayName,
              reasoning: resolved.reasoning,
              input: resolved.vision ? ['text', 'image'] : ['text'],
              contextWindow: resolved.contextWindow,
              maxTokens: resolved.maxTokens,
              ...(resolved.thinkingLevelMap ? { thinkingLevelMap: resolved.thinkingLevelMap } : {}),
              ...(form.api === 'anthropic-messages' && isPiAdaptiveThinkingModel(model.id)
                ? { compat: { forceAdaptiveThinking: true } }
                : {}),
            }
          }),
        }
  const auth = agentId === 'opencode'
    ? form.apiKey.trim() ? { type: 'api', key: form.apiKey.trim() } : null
    : agentId === 'dsh' ? null : { type: 'api_key', key: form.apiKey.trim() }
  return {
    configJson: JSON.stringify(config, null, 2),
    authJson: auth ? JSON.stringify(auth, null, 2) : '',
    modelMetadataJson: serializeModelMetadata(models),
    defaultModel: form.defaultModel.trim(),
    defaultReasoning: agentId === 'pi' ? form.defaultThinkingLevel : '',
  }
}

export function dshNativeDetailToProvider(detail: {
  id: string
  name: string
  baseUrl: string
  api: string
  apiKey: string
  models: Array<{ id: string; name: string }>
  defaultModel?: string
}): ExternalCliProvider {
  const api = DSH_API_OPTIONS.includes(detail.api as typeof DSH_API_OPTIONS[number])
    ? detail.api
    : 'openai-completions'
  const models = detail.models
    .map((model) => ({
      ...emptyNativeModel('dsh', model.id),
      name: model.name,
    }))
    .filter((model) => model.id)
  const form: NativeCliProviderForm = {
    nativeProviderId: detail.id,
    baseUrl: detail.baseUrl,
    apiKey: detail.apiKey,
    api,
    models: models.length ? models : [emptyNativeModel('dsh')],
    defaultModel: detail.defaultModel?.trim() || models[0]?.id || '',
    defaultThinkingLevel: '',
    sourceConfigJson: JSON.stringify({
      displayName: detail.name,
      api,
      baseURL: detail.baseUrl,
      models: detail.models,
    }),
  }
  return {
    id: `p-dsh-${detail.id}`,
    name: detail.name,
    nativeProviderId: detail.id,
    env: [{ key: dshApiKeyEnv(detail.id), value: detail.apiKey }],
    ...buildNativeCliProvider('dsh', detail.name, form),
  }
}
