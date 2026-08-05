import type { ExternalCliProvider } from '../api/tauri'
import { matchModelExact } from '../data/modelMatching'

export type NativeCliAgentId = 'opencode' | 'pi'

export type NativeCliModel = {
  id: string
  name: string
  reasoning: boolean | null
  vision: boolean | null
  contextWindow: string
  maxTokens: string
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

export type NativeCliProviderForm = {
  baseUrl: string
  apiKey: string
  api: string
  models: NativeCliModel[]
  defaultModel: string
  defaultThinkingLevel: string
}

export const PI_API_OPTIONS = [
  'openai-completions',
  'openai-responses',
  'anthropic-messages',
  'google-generative-ai',
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

export function emptyNativeModel(agentId: NativeCliAgentId, id = ''): NativeCliModel {
  return {
    id,
    name: '',
    reasoning: agentId === 'pi' ? null : false,
    vision: agentId === 'pi' ? null : false,
    contextWindow: '',
    maxTokens: '',
  }
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
      contextWindow: model.contextWindow.trim(),
      maxTokens: model.maxTokens.trim(),
    })
  }
  return normalized
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
  const catalogThinkingLevels = PI_MAPPED_THINKING_OPTIONS.filter((level) =>
    matched?.reasoningEfforts?.includes(level),
  )
  const thinkingLevels: PiThinkingLevel[] = !reasoning
    ? ['off']
    : catalogThinkingLevels.length > 0
      ? ['off', ...catalogThinkingLevels]
      : PI_DEFAULT_REASONING_OPTIONS
  const thinkingLevelMap = reasoning && catalogThinkingLevels.length > 0
    ? Object.fromEntries(PI_MAPPED_THINKING_OPTIONS.map((level) => [
        level,
        catalogThinkingLevels.includes(level) ? level : null,
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

function readPersistedPiModel(value: unknown, id: string): NativeCliModel {
  const item = value && typeof value === 'object' && !Array.isArray(value)
    ? value as Record<string, unknown>
    : {}
  return {
    id,
    name: stringValue(item.name),
    reasoning: typeof item.reasoning === 'boolean' ? item.reasoning : null,
    vision: typeof item.vision === 'boolean' ? item.vision : null,
    contextWindow: positiveIntegerString(item.contextWindow),
    maxTokens: positiveIntegerString(item.maxTokens),
  }
}

function parsePiModelMetadata(text?: string): Record<string, unknown> | null {
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

export function readNativeCliProvider(
  agentId: NativeCliAgentId,
  initial?: ExternalCliProvider | null,
): NativeCliProviderForm {
  const config = objectValue(initial?.configJson)
  const auth = objectValue(initial?.authJson)
  let baseUrl = ''
  let api = 'openai-completions'
  let models: NativeCliModel[] = []
  const persistedPiModels = agentId === 'pi'
    ? parsePiModelMetadata(initial?.modelMetadataJson)
    : null

  if (agentId === 'opencode') {
    const options = config.options && typeof config.options === 'object' && !Array.isArray(config.options)
      ? config.options as Record<string, unknown>
      : {}
    baseUrl = stringValue(options.baseURL)
    const rawModels = config.models && typeof config.models === 'object' && !Array.isArray(config.models)
      ? config.models as Record<string, unknown>
      : {}
    models = Object.entries(rawModels).map(([id, value]) => {
      const item = value && typeof value === 'object' && !Array.isArray(value)
        ? value as Record<string, unknown>
        : {}
      return { ...emptyNativeModel('opencode', id), name: stringValue(item.name) || id }
    })
  } else {
    baseUrl = stringValue(config.baseUrl)
    api = PI_API_OPTIONS.includes(config.api as typeof PI_API_OPTIONS[number])
      ? config.api as string
      : 'openai-completions'
    models = Array.isArray(config.models)
      ? config.models.flatMap((value) => {
          if (!value || typeof value !== 'object' || Array.isArray(value)) return []
          const item = value as Record<string, unknown>
          const id = stringValue(item.id)
          if (!id) return []
          return [persistedPiModels !== null
            ? readPersistedPiModel(persistedPiModels[id], id)
            : readLegacyPiModel(item, id)]
        })
      : []
  }

  baseUrl ||= legacyEnv(initial, /BASE_URL$/i)
  const apiKey = stringValue(auth.key) || legacyEnv(initial, /(API_KEY|AUTH_TOKEN)$/i)
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
  }
}

function serializePiModelMetadata(models: NativeCliModel[]): string {
  const entries = models.flatMap((model) => {
    const metadata: Record<string, string | boolean> = {}
    if (model.name) metadata.name = model.name
    if (model.reasoning !== null) metadata.reasoning = model.reasoning
    if (model.vision !== null) metadata.vision = model.vision
    if (model.contextWindow) metadata.contextWindow = model.contextWindow
    if (model.maxTokens) metadata.maxTokens = model.maxTokens
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
  const config = agentId === 'opencode'
    ? {
        npm: '@ai-sdk/openai-compatible',
        name,
        options: { baseURL: form.baseUrl.trim() },
        models: Object.fromEntries(models.map((model) => [model.id, { name: model.name || model.id }])),
      }
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
          }
        }),
      }
  const auth = agentId === 'opencode'
    ? { type: 'api', key: form.apiKey.trim() }
    : { type: 'api_key', key: form.apiKey.trim() }
  return {
    configJson: JSON.stringify(config, null, 2),
    authJson: JSON.stringify(auth, null, 2),
    modelMetadataJson: agentId === 'pi' ? serializePiModelMetadata(models) : '',
    defaultModel: form.defaultModel.trim(),
    defaultReasoning: agentId === 'pi' ? form.defaultThinkingLevel : '',
  }
}
