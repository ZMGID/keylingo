/**
 * Kimi Code CLI 第三方供应商：落盘到 `~/.kimi-code/config.toml`。
 *
 * 形状对齐官方文档（https://moonshotai.github.io/kimi-code/en/configuration/providers）：
 *   [providers.<id>]  type / base_url / api_key
 *   [models."<id>/<model>"]  provider / model / max_context_size / display_name
 *   default_model = "<id>/<model>"
 *
 * 每条 Kivio 供应商存一份 TOML 片段；后端合并进现有文件，managed:kimi-code 等段不动。
 */

export type KimiApiType =
  | 'openai'
  | 'openai_responses'
  | 'anthropic'
  | 'kimi'
  | 'google-genai'

export type KimiModelFields = {
  /** 线上真实 model id */
  model: string
  displayName: string
  /** max_context_size；空串 = 默认 128000 */
  contextWindow: string
}

export type KimiProviderFields = {
  /** 写入 [providers.<id>] 的 key */
  providerId: string
  type: KimiApiType
  baseUrl: string
  apiKey: string
  models: KimiModelFields[]
  /** 默认模型的 wire model id（不是 alias） */
  defaultModel: string
}

export const KIMI_API_TYPES: KimiApiType[] = [
  'openai',
  'openai_responses',
  'anthropic',
  'kimi',
  'google-genai',
]

// 文案对齐 Kivio 主供应商「接口协议」下拉（ProviderDetail），避免同一概念两种叫法。
export const KIMI_API_TYPE_LABELS: Record<KimiApiType, string> = {
  openai: 'OpenAI Compatible',
  openai_responses: 'OpenAI Responses',
  anthropic: 'Anthropic Messages',
  kimi: 'Kimi',
  'google-genai': 'Google Gemini',
}

export const DEFAULT_KIMI_CONTEXT_WINDOW = '128000'

export const DEFAULT_KIMI_FIELDS: KimiProviderFields = {
  providerId: '',
  type: 'openai',
  baseUrl: 'https://api.example.com/v1',
  apiKey: '',
  models: [{ model: '', displayName: '', contextWindow: DEFAULT_KIMI_CONTEXT_WINDOW }],
  defaultModel: '',
}

const tomlString = (value: string): string => JSON.stringify(value)

function escapeTomlKey(key: string): string {
  if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) return key
  return tomlString(key)
}

/** 官方 id 规则：字母/数字开头，允许 - _ 空格。 */
export function isValidKimiProviderId(value: string): boolean {
  const id = value.trim()
  if (!id || id.length > 64) return false
  return /^[\p{L}\p{N}][\p{L}\p{N}\-_ ]*$/u.test(id)
}

export function kimiProviderIdFromName(name: string): string {
  return name
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9._-]+/g, '-')
    .replace(/^[._-]+|[._-]+$/g, '')
    .replace(/-{2,}/g, '-')
    .slice(0, 64)
}

export function kimiModelAlias(providerId: string, model: string): string {
  return `${providerId.trim()}/${model.trim()}`
}

export function emptyKimiModel(): KimiModelFields {
  return { model: '', displayName: '', contextWindow: DEFAULT_KIMI_CONTEXT_WINDOW }
}

function contextWindowNumber(raw: string): number {
  const digits = raw.trim().replace(/_/g, '')
  if (/^\d+$/.test(digits)) {
    const n = Number(digits)
    if (n >= 1) return n
  }
  return Number(DEFAULT_KIMI_CONTEXT_WINDOW)
}

/** 生成可落盘的 Kimi config.toml 片段。 */
export function buildKimiConfigToml(fields: KimiProviderFields): string {
  const providerId = fields.providerId.trim()
  const models = fields.models
    .map((item) => ({
      model: item.model.trim(),
      displayName: item.displayName.trim(),
      contextWindow: item.contextWindow.trim(),
    }))
    .filter((item) => item.model)

  const defaultWire = fields.defaultModel.trim() || models[0]?.model || ''
  const defaultAlias = defaultWire && providerId
    ? kimiModelAlias(providerId, defaultWire)
    : ''

  const lines: string[] = []
  if (defaultAlias) {
    lines.push(`default_model = ${tomlString(defaultAlias)}`)
    lines.push('')
  }

  if (providerId) {
    lines.push(`[providers.${escapeTomlKey(providerId)}]`)
    lines.push(`type = ${tomlString(fields.type)}`)
    if (fields.baseUrl.trim()) {
      lines.push(`base_url = ${tomlString(fields.baseUrl.trim())}`)
    }
    if (fields.apiKey.trim()) {
      lines.push(`api_key = ${tomlString(fields.apiKey.trim())}`)
    }
    lines.push('')
  }

  for (const item of models) {
    if (!providerId) continue
    const alias = kimiModelAlias(providerId, item.model)
    lines.push(`[models.${escapeTomlKey(alias)}]`)
    lines.push(`provider = ${tomlString(providerId)}`)
    lines.push(`model = ${tomlString(item.model)}`)
    lines.push(`max_context_size = ${contextWindowNumber(item.contextWindow)}`)
    if (item.displayName) {
      lines.push(`display_name = ${tomlString(item.displayName)}`)
    }
    lines.push('')
  }

  return lines.join('\n')
}

function unquoteToml(value: string): string {
  const trimmed = value.trim()
  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"'))
    || (trimmed.startsWith("'") && trimmed.endsWith("'"))
  ) {
    try {
      if (trimmed.startsWith('"')) return JSON.parse(trimmed) as string
      return trimmed.slice(1, -1)
    } catch {
      return trimmed.slice(1, -1)
    }
  }
  return trimmed
}

type TomlSection = { header: string; body: string }

function parseTomlSections(text: string): TomlSection[] {
  const lines = text.split(/\r?\n/)
  const sections: TomlSection[] = []
  let header = ''
  let body: string[] = []
  const flush = () => {
    if (!header && body.every((line) => !line.trim())) {
      body = []
      return
    }
    sections.push({ header, body: body.join('\n') })
    body = []
  }
  for (const line of lines) {
    const trimmed = line.trim()
    if (/^\[.+\]$/.test(trimmed)) {
      flush()
      header = trimmed.slice(1, -1)
      continue
    }
    body.push(line)
  }
  flush()
  return sections
}

function readTomlField(section: string, key: string): string {
  const re = new RegExp(`^\\s*${key}\\s*=\\s*(.+?)\\s*$`, 'im')
  const match = section.match(re)
  if (!match) return ''
  return unquoteToml(match[1] ?? '')
}

function parseSectionKey(header: string): { table: string; key: string } | null {
  // providers.foo / providers."foo-bar" / models."a/b"
  const m = header.match(/^(providers|models)\.(?:"([^"]+)"|'([^']+)'|(.+))$/)
  if (!m) return null
  return {
    table: m[1]!,
    key: m[2] || m[3] || (m[4] ?? '').trim(),
  }
}

/**
 * 从 config.toml 片段抽出结构化字段。
 * 优先读 default_model 指向的 provider；否则取第一个 [providers.*]。
 */
export function parseKimiConfigToml(configToml: string): KimiProviderFields {
  if (!configToml.trim()) {
    return {
      ...DEFAULT_KIMI_FIELDS,
      baseUrl: '',
      models: [emptyKimiModel()],
    }
  }

  const sections = parseTomlSections(configToml)
  const top = sections.find((s) => !s.header)?.body ?? ''
  const defaultAlias = readTomlField(top, 'default_model')
    || readTomlField(configToml, 'default_model')

  const providerSections = sections.flatMap((s) => {
    const parsed = parseSectionKey(s.header)
    if (!parsed || parsed.table !== 'providers') return []
    return [{ id: parsed.key, body: s.body }]
  })

  let providerId = ''
  if (defaultAlias.includes('/')) {
    providerId = defaultAlias.slice(0, defaultAlias.indexOf('/'))
  }
  if (!providerId || !providerSections.some((p) => p.id === providerId)) {
    providerId = providerSections[0]?.id ?? ''
  }

  const providerBody = providerSections.find((p) => p.id === providerId)?.body ?? ''
  const typeRaw = readTomlField(providerBody, 'type')
  const type: KimiApiType = KIMI_API_TYPES.includes(typeRaw as KimiApiType)
    ? (typeRaw as KimiApiType)
    : 'openai'

  const models: KimiModelFields[] = sections.flatMap((s) => {
    const parsed = parseSectionKey(s.header)
    if (!parsed || parsed.table !== 'models') return []
    const body = s.body
    const provider = readTomlField(body, 'provider')
    if (providerId && provider && provider !== providerId) return []
    // 若 alias 以 providerId/ 开头也认
    if (
      providerId
      && !provider
      && !parsed.key.startsWith(`${providerId}/`)
      && providerSections.length > 0
    ) {
      return []
    }
    const model = readTomlField(body, 'model')
      || (parsed.key.includes('/') ? parsed.key.slice(parsed.key.indexOf('/') + 1) : parsed.key)
    if (!model) return []
    return [{
      model,
      displayName: readTomlField(body, 'display_name'),
      contextWindow: readTomlField(body, 'max_context_size').replace(/_/g, '')
        || DEFAULT_KIMI_CONTEXT_WINDOW,
    }]
  })

  let defaultModel = ''
  if (defaultAlias.includes('/')) {
    defaultModel = defaultAlias.slice(defaultAlias.indexOf('/') + 1)
  } else if (defaultAlias) {
    defaultModel = defaultAlias
  }
  if (defaultModel && !models.some((m) => m.model === defaultModel)) {
    // default 指向的模型不在列表里时仍保留 wire id，列表至少有一项
  }
  if (!defaultModel) defaultModel = models[0]?.model ?? ''

  return {
    providerId,
    type,
    baseUrl: readTomlField(providerBody, 'base_url'),
    apiKey: readTomlField(providerBody, 'api_key'),
    models: models.length ? models : [emptyKimiModel()],
    defaultModel,
  }
}

/** 用结构化字段重建片段（高级编辑场景：丢弃非本供应商段，只保留干净片段）。 */
export function setKimiStructuredFields(
  currentToml: string,
  patch: Partial<KimiProviderFields>,
): string {
  const current = parseKimiConfigToml(currentToml)
  const next: KimiProviderFields = {
    providerId: patch.providerId ?? current.providerId,
    type: patch.type ?? current.type,
    baseUrl: patch.baseUrl ?? current.baseUrl,
    apiKey: patch.apiKey ?? current.apiKey,
    models: patch.models ?? current.models,
    defaultModel: patch.defaultModel ?? current.defaultModel,
  }
  return buildKimiConfigToml(next)
}

export function initialKimiToml(initial?: {
  configToml?: string
  name?: string
  nativeProviderId?: string
} | null): string {
  if (initial?.configToml?.trim()) return initial.configToml
  const providerId = initial?.nativeProviderId?.trim()
    || kimiProviderIdFromName(initial?.name ?? '')
    || 'relay'
  return buildKimiConfigToml({
    ...DEFAULT_KIMI_FIELDS,
    providerId,
  })
}

export type KimiTomlValidationError =
  | 'empty'
  | 'missing-provider-id'
  | 'invalid-provider-id'
  | 'missing-base-url'
  | 'missing-api-key'
  | 'missing-model'
  | 'duplicate-model'
  | 'invalid-context'
  | 'invalid-default'

/** openai / openai_responses 中转必须给 base_url；其它类型有 SDK 默认端点。 */
export function kimiTypeNeedsBaseUrl(type: KimiApiType): boolean {
  return type === 'openai' || type === 'openai_responses'
}

export function validateKimiConfigToml(configToml: string): KimiTomlValidationError | null {
  if (!configToml.trim()) return 'empty'
  const fields = parseKimiConfigToml(configToml)
  if (!fields.providerId.trim()) return 'missing-provider-id'
  if (!isValidKimiProviderId(fields.providerId)) return 'invalid-provider-id'
  if (kimiTypeNeedsBaseUrl(fields.type) && !fields.baseUrl.trim()) return 'missing-base-url'
  if (!fields.apiKey.trim()) return 'missing-api-key'
  const models = fields.models.map((m) => m.model.trim()).filter(Boolean)
  if (models.length === 0) return 'missing-model'
  if (new Set(models).size !== models.length) return 'duplicate-model'
  for (const item of fields.models) {
    if (!item.model.trim()) continue
    const ctx = item.contextWindow.trim().replace(/_/g, '')
    if (ctx && (!/^\d+$/.test(ctx) || Number(ctx) < 1)) return 'invalid-context'
  }
  if (fields.defaultModel.trim() && !models.includes(fields.defaultModel.trim())) {
    return 'invalid-default'
  }
  return null
}
