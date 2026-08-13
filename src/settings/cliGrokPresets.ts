/**
 * Grok CLI 第三方供应商：落盘到 `~/.grok/config.toml`（与 cc-switch grokbuild 同通道）。
 *
 * 每条供应商存一份 config.toml 片段（至少含 `[models].default` + `[model."<id>"]`）；
 * 后端合并进现有文件，marketplace / ui / cli 等用户段不动。
 */

export type GrokApiBackend = 'chat_completions' | 'responses' | 'messages'

export type GrokProviderFields = {
  baseUrl: string
  apiKey: string
  model: string
  displayName: string
  apiBackend: GrokApiBackend
  /** 空串 = 不写 context_window */
  contextWindow: string
}

export const GROK_API_BACKENDS: GrokApiBackend[] = [
  'chat_completions',
  'responses',
  'messages',
]

export const DEFAULT_GROK_FIELDS: GrokProviderFields = {
  baseUrl: 'https://api.example.com/v1',
  apiKey: '',
  model: 'grok-4.6',
  displayName: '',
  apiBackend: 'responses',
  contextWindow: '500000',
}

const tomlString = (value: string): string => JSON.stringify(value)

function escapeTomlKey(key: string): string {
  // 简单 id 直接写；含点/横线等用引号键（与 grok 官方示例 `[model."grok-4.5"]` 一致）。
  if (/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) return key
  return tomlString(key)
}

/** 生成可落盘的 Grok config.toml 片段（只含 models / model）。 */
export function buildGrokConfigToml(fields: GrokProviderFields): string {
  const model = fields.model.trim() || 'grok-4.6'
  const key = escapeTomlKey(model)
  const lines = [
    '[models]',
    `default = ${tomlString(model)}`,
    '',
    `[model.${key}]`,
    `model = ${tomlString(model)}`,
  ]
  if (fields.baseUrl.trim()) {
    lines.push(`base_url = ${tomlString(fields.baseUrl.trim())}`)
  }
  if (fields.displayName.trim()) {
    lines.push(`name = ${tomlString(fields.displayName.trim())}`)
  }
  if (fields.apiBackend) {
    lines.push(`api_backend = ${tomlString(fields.apiBackend)}`)
  }
  const ctx = fields.contextWindow.trim()
  if (ctx && /^\d+$/.test(ctx)) {
    lines.push(`context_window = ${Number(ctx)}`)
  }
  if (fields.apiKey.trim()) {
    lines.push(`api_key = ${tomlString(fields.apiKey.trim())}`)
  }
  lines.push('')
  return lines.join('\n')
}

function unquoteToml(value: string): string {
  const trimmed = value.trim()
  if (
    (trimmed.startsWith('"') && trimmed.endsWith('"'))
    || (trimmed.startsWith("'") && trimmed.endsWith("'"))
  ) {
    try {
      // JSON 双引号串可直接 parse；单引号简单去壳。
      if (trimmed.startsWith('"')) return JSON.parse(trimmed) as string
      return trimmed.slice(1, -1)
    } catch {
      return trimmed.slice(1, -1)
    }
  }
  return trimmed
}

function parseSectionTable(text: string, header: RegExp): string {
  const lines = text.split(/\r?\n/)
  let capturing = false
  const body: string[] = []
  for (const line of lines) {
    if (header.test(line.trim())) {
      capturing = true
      continue
    }
    if (capturing) {
      if (/^\s*\[/.test(line)) break
      body.push(line)
    }
  }
  return body.join('\n')
}

function readTomlField(section: string, key: string): string {
  const re = new RegExp(`^\\s*${key}\\s*=\\s*(.+?)\\s*$`, 'im')
  const match = section.match(re)
  if (!match) return ''
  return unquoteToml(match[1] ?? '')
}

/**
 * 从 config.toml 抽出结构化字段。
 * 优先读 `[models].default` 指向的 `[model."…"]`；找不到就取第一个 model 表。
 */
export function parseGrokConfigToml(configToml: string): GrokProviderFields {
  if (!configToml.trim()) return { ...DEFAULT_GROK_FIELDS, baseUrl: '', model: '' }

  const modelsSection = parseSectionTable(configToml, /^\[models\]$/)
  let modelId = readTomlField(modelsSection, 'default')

  if (!modelId) {
    const first = configToml.match(/^\s*\[model\.(?:"([^"]+)"|'([^']+)'|([^\]]+))\]/m)
    modelId = first?.[1] || first?.[2] || first?.[3]?.trim() || ''
  }

  const modelSection = (() => {
    if (!modelId) {
      return parseSectionTable(configToml, /^\[model\./)
    }
    const lines = configToml.split(/\r?\n/)
    let capturing = false
    const body: string[] = []
    for (const line of lines) {
      const trimmed = line.trim()
      const m = trimmed.match(/^\[model\.(?:"([^"]+)"|'([^']+)'|([^\]]+))\]$/)
      if (m) {
        const id = m[1] || m[2] || m[3]?.trim() || ''
        capturing = id === modelId
        continue
      }
      if (capturing) {
        if (/^\s*\[/.test(line)) break
        body.push(line)
      }
    }
    return body.join('\n')
  })()

  const backendRaw = readTomlField(modelSection, 'api_backend')
  const apiBackend: GrokApiBackend = GROK_API_BACKENDS.includes(backendRaw as GrokApiBackend)
    ? (backendRaw as GrokApiBackend)
    : 'responses'

  return {
    baseUrl: readTomlField(modelSection, 'base_url'),
    apiKey: readTomlField(modelSection, 'api_key'),
    model: readTomlField(modelSection, 'model') || modelId,
    displayName: readTomlField(modelSection, 'name'),
    apiBackend,
    contextWindow: readTomlField(modelSection, 'context_window').replace(/_/g, ''),
  }
}

/** 用结构化字段改写 config.toml 的 models/model 段；其它段原样保留（高级编辑场景）。 */
export function setGrokStructuredFields(
  currentToml: string,
  patch: Partial<GrokProviderFields>,
): string {
  const current = parseGrokConfigToml(currentToml)
  const next: GrokProviderFields = {
    baseUrl: patch.baseUrl ?? current.baseUrl,
    apiKey: patch.apiKey ?? current.apiKey,
    model: patch.model ?? current.model,
    displayName: patch.displayName ?? current.displayName,
    apiBackend: patch.apiBackend ?? current.apiBackend,
    contextWindow: patch.contextWindow ?? current.contextWindow,
  }
  // 若原文只有 models/model（或为空），直接重建；否则保留非 model 段再拼。
  const hasExtra = /\[(?!models\b|model\b)[^\]]+\]/.test(currentToml)
  if (!hasExtra) {
    return buildGrokConfigToml(next)
  }
  // 去掉旧的 [models] 与所有 [model.*]，再追加新的。
  const kept: string[] = []
  let skip = false
  for (const line of currentToml.split(/\r?\n/)) {
    const trimmed = line.trim()
    if (/^\[models\]$/.test(trimmed) || /^\[model\./.test(trimmed)) {
      skip = true
      continue
    }
    if (skip && /^\s*\[/.test(line)) {
      skip = false
    }
    if (!skip) kept.push(line)
  }
  while (kept.length > 0 && kept[kept.length - 1]?.trim() === '') kept.pop()
  const fragment = buildGrokConfigToml(next).trimEnd()
  return `${kept.join('\n').replace(/\s+$/, '')}\n\n${fragment}\n`
}

export function initialGrokToml(initial?: { configToml?: string } | null): string {
  if (initial?.configToml?.trim()) return initial.configToml
  return buildGrokConfigToml(DEFAULT_GROK_FIELDS)
}

/** 轻量校验：能 parse 出 model + base_url 即过。 */
export function validateGrokConfigToml(configToml: string): string | null {
  if (!configToml.trim()) return 'empty'
  const fields = parseGrokConfigToml(configToml)
  if (!fields.model.trim()) return 'missing-model'
  if (!fields.baseUrl.trim()) return 'missing-base-url'
  return null
}
