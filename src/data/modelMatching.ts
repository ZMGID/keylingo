import modelDatabase from './modelDatabase.json'
import type { ModelInfo, ModelProvider } from '../api/tauri'

type DbEntry = {
  displayName: string
  contextWindow: number
  maxOutput: number
  temperature?: number
  capabilities: {
    vision?: boolean
    functionCalling?: boolean
    reasoning?: boolean
    streaming?: boolean
    webSearch?: boolean
    imageGeneration?: boolean
    embedding?: boolean
  }
  dimensions?: number
  multilingual?: boolean
  reasoningEfforts?: string[]
  pricing?: {
    input: number
    output: number
    cachedInput?: number
  }
}

const raw = modelDatabase as Record<string, unknown>
// eslint-disable-next-line @typescript-eslint/no-unused-vars
const { _meta: _, ...dbEntries } = raw
const db = dbEntries as unknown as Record<string, DbEntry>
const dbKeys = Object.keys(db)

// 版本分隔符归一化：数据库键用点号（`claude-sonnet-4.6`、`gpt-5.5`），但很多 provider
// 返回的模型 ID 用连字符（`claude-sonnet-4-6`、`claude-opus-4-8`）。若不归一化，
// `claude-sonnet-4-6` 精确匹配不到 `claude-sonnet-4.6`，会前缀退化到旧的 `claude-sonnet-4`
// → 显示成 "Claude Sonnet 4"（错）。统一把 `.` 视作 `-` 后再比对。
const normalizeSep = (value: string): string => value.replace(/\./g, '-')

// 版本延续判定：DB key 以数字结尾，且候选里紧跟其后的分段是「1~2 位纯数字」（`gpt-5` ← `gpt-5.6-luna`，
// 归一化为 `gpt-5` ← `gpt-5-6-luna`），说明候选是更细的次级版本号（5 → 5.6），不能退化到基础版本，
// 否则会把 `gpt-5.6-luna` 显示成 "GPT-5"。日期/快照后缀（`claude-opus-4-8-20260101`、`gpt-4-0125-…`，
// 数字段 ≥3 位）不算版本延续，仍走正常前缀匹配。
const isVersionContinuation = (keyNorm: string, candidate: string, endIdx: number): boolean => {
  if (!/[0-9]/.test(keyNorm[keyNorm.length - 1])) return false
  if (candidate[endIdx] !== '-') return false
  const nextSegment = candidate.slice(endIdx + 1).split('-')[0]
  return /^[0-9]{1,2}$/.test(nextSegment)
}
// 归一化键 → 原始键（首个占位；当前库无仅靠 `.`/`-` 区分的重复键）。
const normalizedExact = new Map<string, string>()
const normalizedEntries = dbKeys.map((orig) => {
  const norm = normalizeSep(orig)
  if (!normalizedExact.has(norm)) normalizedExact.set(norm, orig)
  return { orig, norm }
})


/**
 * 从内置数据库匹配模型信息
 *
 * 匹配优先级：
 * 1. 精确匹配
 * 2. 前缀匹配（最长）
 * 3. 包含匹配（最长）
 *
 * 自动处理 OpenRouter 风格 "provider/model" → 去掉前缀再匹配
 */
export function matchModel(modelName: string): ModelInfo | null {
  if (!modelName.trim()) return null

  const exact = matchModelExact(modelName)
  if (exact) return exact

  // 去掉 provider/ 前缀（如 "openai/gpt-4o" → "gpt-4o"）
  const name = modelName.toLowerCase().trim()
  const stripped = name.includes('/') ? name.split('/').pop()! : name
  const normName = normalizeSep(name)
  const normStripped = normalizeSep(stripped)

  // 归一化候选，供前缀 / 包含匹配复用。
  const normCandidates = normName === normStripped ? [normStripped] : [normName, normStripped]

  // 2. 前缀匹配（归一化后最长 key 优先）
  const prefixMatches = normalizedEntries
    .filter((entry) =>
      normCandidates.some(
        (candidate) =>
          candidate.startsWith(entry.norm) &&
          entry.norm.length < candidate.length &&
          !isVersionContinuation(entry.norm, candidate, entry.norm.length),
      ),
    )
    .sort((a, b) => b.norm.length - a.norm.length)
  if (prefixMatches.length > 0) {
    return toModelInfo(db[prefixMatches[0].orig])
  }

  // 3. 包含匹配（归一化后最长 key 优先）
  const containsMatches = normalizedEntries
    .filter((entry) =>
      normCandidates.some((candidate) => {
        if (entry.norm === candidate) return false
        const idx = candidate.indexOf(entry.norm)
        if (idx < 0) return false
        return !isVersionContinuation(entry.norm, candidate, idx + entry.norm.length)
      }),
    )
    .sort((a, b) => b.norm.length - a.norm.length)
  if (containsMatches.length > 0) {
    return toModelInfo(db[containsMatches[0].orig])
  }

  const imageGenerationFallback = matchKnownImageGenerationModel(stripped)
  if (imageGenerationFallback) {
    return imageGenerationFallback
  }

  return null
}

/**
 * 仅按完整模型 ID 匹配，不做前缀或包含推断。
 *
 * 自定义 CLI 供应商常使用带品牌词的私有别名；对这类 ID 做模糊匹配会把别名误判成
 * 公共模型并写入错误能力。允许 provider/model 前缀和点/横线版本分隔符归一化。
 */
export function matchModelExact(modelName: string): ModelInfo | null {
  if (!modelName.trim()) return null
  const name = modelName.toLowerCase().trim()
  const stripped = name.includes('/') ? name.split('/').pop()! : name
  if (db[name]) return toModelInfo(db[name])
  if (db[stripped]) return toModelInfo(db[stripped])

  const exactNorm = normalizedExact.get(normalizeSep(name))
    ?? normalizedExact.get(normalizeSep(stripped))
  return exactNorm ? toModelInfo(db[exactNorm]) : null
}

/**
 * 合并模型信息：数据库默认值 + 用户覆盖
 * 用户覆盖的字段优先，未覆盖的字段用数据库默认
 */
export function providerModelDatabaseId(model: string, provider?: Pick<ModelProvider, 'baseUrl' | 'request'>): string {
  let kimi = provider?.request?.oauth?.provider === 'kimi'
  try { const url = new URL(provider?.baseUrl ?? ''); kimi ||= url.hostname === 'api.kimi.com' && /^\/coding(?:\/|$)/.test(url.pathname) } catch { /* Not a Kimi endpoint. */ }
  const id = model.trim().toLowerCase()
  return kimi && ['k3', 'k3-256k', 'kimi-for-coding', 'kimi-for-coding-highspeed'].includes(id) ? 'kimi-code/' + id : model
}

export function resolveModelInfo(
  modelName: string,
  overrides?: Record<string, ModelInfo>,
  provider?: Pick<ModelProvider, 'baseUrl' | 'request'>,
): ModelInfo {
  const defaults = matchModel(providerModelDatabaseId(modelName, provider))
  const override = overrides?.[modelName]

  if (!defaults && !override) return {}
  if (!defaults) return override!
  if (!override) return defaults

  return {
    displayName: override.displayName ?? defaults.displayName,
    contextWindow: override.contextWindow ?? defaults.contextWindow,
    maxOutput: override.maxOutput ?? defaults.maxOutput,
    temperature: override.omitTemperature
      ? undefined
      : (override.temperature ?? defaults.temperature),
    omitTemperature: override.omitTemperature,
    capabilities: {
      vision: override.capabilities?.vision ?? defaults.capabilities?.vision,
      functionCalling: override.capabilities?.functionCalling ?? defaults.capabilities?.functionCalling,
      reasoning: override.capabilities?.reasoning ?? defaults.capabilities?.reasoning,
      streaming: override.capabilities?.streaming ?? defaults.capabilities?.streaming,
      webSearch: override.capabilities?.webSearch ?? defaults.capabilities?.webSearch,
      imageGeneration: override.capabilities?.imageGeneration ?? defaults.capabilities?.imageGeneration,
      embedding: override.capabilities?.embedding ?? defaults.capabilities?.embedding,
    },
    dimensions: override.dimensions ?? defaults.dimensions,
    multilingual: override.multilingual ?? defaults.multilingual,
    pricing: {
      input: override.pricing?.input ?? defaults.pricing?.input,
      output: override.pricing?.output ?? defaults.pricing?.output,
      cachedInput: override.pricing?.cachedInput ?? defaults.pricing?.cachedInput,
    },
    reasoningEfforts: override.reasoningEfforts ?? defaults.reasoningEfforts,
    // extraBody 只来自用户 override（模型库无此字段）。
    extraBody: override.extraBody ?? defaults.extraBody,
  }
}

function toModelInfo(entry: DbEntry): ModelInfo {
  return {
    displayName: entry.displayName,
    contextWindow: entry.contextWindow,
    maxOutput: entry.maxOutput,
    temperature: entry.temperature,
    capabilities: {
      vision: entry.capabilities.vision ?? false,
      functionCalling: entry.capabilities.functionCalling ?? false,
      reasoning: entry.capabilities.reasoning ?? false,
      streaming: entry.capabilities.streaming ?? false,
      webSearch: entry.capabilities.webSearch ?? false,
      imageGeneration: entry.capabilities.imageGeneration ?? false,
      embedding: entry.capabilities.embedding ?? false,
    },
    dimensions: entry.dimensions,
    multilingual: entry.multilingual,
    reasoningEfforts: entry.reasoningEfforts,
    pricing: {
      input: entry.pricing?.input,
      output: entry.pricing?.output,
      cachedInput: entry.pricing?.cachedInput,
    },
  }
}

function matchKnownImageGenerationModel(modelName: string): ModelInfo | null {
  if (/^gpt-image-[a-z0-9][a-z0-9.-]*$/.test(modelName)) {
    return knownImageGenerationModelInfo(modelName, true)
  }
  if (/^dall-e-[a-z0-9][a-z0-9.-]*$/.test(modelName)) {
    return knownImageGenerationModelInfo(modelName, modelName !== 'dall-e-3')
  }
  // 库未收录的新变体：Mixer 生图槽只认 imageGeneration=true，不能让它们掉出列表。
  if (
    /^grok-imagine-image(?:-|$)/.test(modelName)
    || /^nano-banana(?:-|$)/.test(modelName)
    || /^qwen-image(?:-|$)/.test(modelName)
  ) {
    return knownImageGenerationModelInfo(modelName, true)
  }
  return null
}

function knownImageGenerationModelInfo(modelName: string, vision: boolean): ModelInfo {
  return {
    displayName: titleCaseModelName(modelName),
    contextWindow: 0,
    maxOutput: 0,
    capabilities: {
      vision,
      functionCalling: false,
      reasoning: false,
      streaming: false,
      webSearch: false,
      imageGeneration: true,
    },
    pricing: {
      input: 0,
      output: 0,
    },
  }
}

function titleCaseModelName(modelName: string): string {
  return modelName
    .split('-')
    .map((part) => {
      if (part === 'gpt') return 'GPT'
      if (part === 'dall') return 'DALL'
      if (part === 'e') return 'E'
      return part.length === 0 ? part : part[0].toUpperCase() + part.slice(1)
    })
    .join(' ')
}
