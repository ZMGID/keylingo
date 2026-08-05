/**
 * Codex 第三方供应商预设。
 * 对齐 desktop-cc-gui 的 CODEX_PROVIDER_PRESETS + buildCodexProviderConfigToml。
 * 不含官方直连（与 Claude 弹窗一致：只做中转 / 自定义）。
 */

export const CODEX_CUSTOM_PRESET_ID = 'custom' as const

export type CodexPresetId =
  | typeof CODEX_CUSTOM_PRESET_ID
  | 'zhipu'
  | 'kimi'
  | 'kimi-coding'
  | 'deepseek'
  | 'atlas-cloud'
  | 'minimax'
  | 'xiaomi'
  | 'xiaomi-plan'
  | 'bailian'
  | 'bailian-coding'
  | 'longcat'
  | 'opencode-go'
  | 'openrouter'

/** 与 Claude 预设共用品牌图标映射键 */
export type CodexPresetBrand =
  | 'zhipu'
  | 'kimi'
  | 'deepseek'
  | 'minimax'
  | 'xiaomi'
  | 'bailian'
  | 'longcat'
  | 'opencode'
  | 'openrouter'
  | 'custom'
  | 'atlas'

export interface CodexProviderPreset {
  id: CodexPresetId
  /** 点选时若名称仍空，写入此显示名 */
  name: string
  /** i18n key suffix under externalAgentsPreset* */
  nameKey: string
  brand: CodexPresetBrand
  configToml: string
  authJson: string
}

const tomlString = (value: string): string => JSON.stringify(value)

/** 生成一份合法的 codex config.toml（必须带 model_providers.*.name，否则 CLI 起不来）。 */
export function buildCodexProviderConfigToml(
  providerName: string,
  baseUrl: string,
  model: string,
  wireApi: 'responses' | 'chat' = 'responses',
  providerId = 'custom',
): string {
  return `disable_response_storage = true
model = ${tomlString(model)}
model_reasoning_effort = "high"
model_provider = ${tomlString(providerId)}

[model_providers.${providerId}]
base_url = ${tomlString(baseUrl)}
name = ${tomlString(providerName)}
requires_openai_auth = true
wire_api = ${tomlString(wireApi)}`
}

export const DEFAULT_CODEX_AUTH_JSON = `{
  "OPENAI_API_KEY": ""
}`

/** 自定义占位：示例端点，用户改 base_url / key 即可。 */
export const DEFAULT_CODEX_CONFIG_TOML = buildCodexProviderConfigToml(
  'relay',
  'https://api.example.com/v1',
  'gpt-5.5',
  'responses',
  'relay',
)

export const CODEX_CUSTOM_PRESET: CodexProviderPreset = {
  id: CODEX_CUSTOM_PRESET_ID,
  name: '',
  nameKey: 'custom',
  brand: 'custom',
  configToml: DEFAULT_CODEX_CONFIG_TOML,
  authJson: DEFAULT_CODEX_AUTH_JSON,
}

/** 中转预设（含自定义）— 顺序对齐 cc-gui */
export const CODEX_RELAY_PRESETS: CodexProviderPreset[] = [
  {
    id: 'zhipu',
    name: 'Zhipu GLM',
    nameKey: 'zhipu',
    brand: 'zhipu',
    configToml: buildCodexProviderConfigToml(
      'zhipu_glm',
      'https://open.bigmodel.cn/api/coding/paas/v4',
      'glm-5.2',
      'chat',
    ),
    authJson: DEFAULT_CODEX_AUTH_JSON,
  },
  {
    id: 'kimi',
    name: 'Kimi',
    nameKey: 'kimi',
    brand: 'kimi',
    configToml: buildCodexProviderConfigToml(
      'kimi',
      'https://api.moonshot.cn/v1',
      'kimi-k3',
      'chat',
    ),
    authJson: DEFAULT_CODEX_AUTH_JSON,
  },
  {
    id: 'kimi-coding',
    name: 'Kimi Coding',
    nameKey: 'kimiCoding',
    brand: 'kimi',
    configToml: buildCodexProviderConfigToml(
      'kimi_coding',
      'https://api.kimi.com/coding/v1',
      'kimi-k3',
      'chat',
    ),
    authJson: DEFAULT_CODEX_AUTH_JSON,
  },
  {
    id: 'deepseek',
    name: 'DeepSeek',
    nameKey: 'deepseek',
    brand: 'deepseek',
    configToml: buildCodexProviderConfigToml(
      'deepseek',
      'https://api.deepseek.com',
      'deepseek-v4-flash',
      'chat',
    ),
    authJson: DEFAULT_CODEX_AUTH_JSON,
  },
  {
    id: 'atlas-cloud',
    name: 'Atlas Cloud',
    nameKey: 'atlasCloud',
    brand: 'atlas',
    configToml: buildCodexProviderConfigToml(
      'atlas_cloud',
      'https://api.atlascloud.ai/v1',
      'deepseek-ai/deepseek-v4-pro',
      'chat',
      'atlas_cloud',
    ),
    authJson: DEFAULT_CODEX_AUTH_JSON,
  },
  {
    id: 'minimax',
    name: 'MiniMax',
    nameKey: 'minimax',
    brand: 'minimax',
    configToml: buildCodexProviderConfigToml(
      'minimax',
      'https://api.minimaxi.com/v1',
      'MiniMax-M3',
    ),
    authJson: DEFAULT_CODEX_AUTH_JSON,
  },
  {
    id: 'xiaomi',
    name: 'Xiaomi MiMo',
    nameKey: 'xiaomi',
    brand: 'xiaomi',
    configToml: buildCodexProviderConfigToml(
      'xiaomi_mimo',
      'https://api.xiaomimimo.com/v1',
      'mimo-v2.5-pro',
    ),
    authJson: DEFAULT_CODEX_AUTH_JSON,
  },
  {
    id: 'xiaomi-plan',
    name: 'Xiaomi MiMo Plan',
    nameKey: 'xiaomiPlan',
    brand: 'xiaomi',
    configToml: buildCodexProviderConfigToml(
      'xiaomi_mimo_token_plan',
      'https://token-plan-cn.xiaomimimo.com/v1',
      'mimo-v2.5-pro',
    ),
    authJson: DEFAULT_CODEX_AUTH_JSON,
  },
  {
    id: 'bailian',
    name: 'Bailian',
    nameKey: 'bailian',
    brand: 'bailian',
    configToml: buildCodexProviderConfigToml(
      'bailian',
      'https://dashscope.aliyuncs.com/compatible-mode/v1',
      'qwen3-coder-plus',
    ),
    authJson: DEFAULT_CODEX_AUTH_JSON,
  },
  {
    id: 'bailian-coding',
    name: 'Bailian Coding',
    nameKey: 'bailianCoding',
    brand: 'bailian',
    configToml: buildCodexProviderConfigToml(
      'bailian_coding',
      'https://coding.dashscope.aliyuncs.com/v1',
      'qwen3-coder-plus',
    ),
    authJson: DEFAULT_CODEX_AUTH_JSON,
  },
  {
    id: 'longcat',
    name: 'LongCat',
    nameKey: 'longcat',
    brand: 'longcat',
    configToml: buildCodexProviderConfigToml(
      'longcat',
      'https://api.longcat.chat/openai/v1',
      'LongCat-2.0',
    ),
    authJson: DEFAULT_CODEX_AUTH_JSON,
  },
  {
    id: 'opencode-go',
    name: 'OpenCode Go',
    nameKey: 'opencodeGo',
    brand: 'opencode',
    configToml: buildCodexProviderConfigToml(
      'opencode_go',
      'https://opencode.ai/zen/go/v1',
      'glm-5.2',
      'chat',
    ),
    authJson: DEFAULT_CODEX_AUTH_JSON,
  },
  {
    id: 'openrouter',
    name: 'OpenRouter',
    nameKey: 'openrouter',
    brand: 'openrouter',
    configToml: buildCodexProviderConfigToml(
      'openrouter',
      'https://openrouter.ai/api/v1',
      'gpt-5.6-sol',
    ),
    authJson: DEFAULT_CODEX_AUTH_JSON,
  },
]

/** 预设列表：自定义置顶 */
export const CODEX_PRESET_BUTTONS: CodexProviderPreset[] = [
  CODEX_CUSTOM_PRESET,
  ...CODEX_RELAY_PRESETS,
]

/** 从 config.toml 抽 base_url，用于编辑回填时反推预设高亮。 */
export function extractCodexBaseUrl(configToml: string): string {
  const match = configToml.match(/base_url\s*=\s*"([^"]+)"/)
  return match?.[1]?.trim() ?? ''
}

/** 顶层 model = "…" */
export function extractCodexModel(configToml: string): string {
  const match = configToml.match(/^\s*model\s*=\s*"([^"]+)"/m)
  return match?.[1]?.trim() ?? ''
}

/** 重建 toml 时要保留的内部字段（表 id / 显示名 / wire_api）。 */
export function extractCodexMeta(configToml: string): {
  providerId: string
  providerName: string
  wireApi: 'responses' | 'chat'
} {
  const providerId =
    configToml.match(/model_provider\s*=\s*"([^"]+)"/)?.[1]?.trim() || 'relay'
  const tableName =
    configToml.match(/name\s*=\s*"([^"]+)"/)?.[1]?.trim() || providerId
  const wire = configToml.match(/wire_api\s*=\s*"([^"]+)"/)?.[1]?.trim()
  return {
    providerId,
    providerName: tableName,
    wireApi: wire === 'chat' ? 'chat' : 'responses',
  }
}

export function extractOpenAiApiKey(authJson: string): string {
  if (!authJson.trim()) return ''
  try {
    const parsed = JSON.parse(authJson) as Record<string, unknown>
    const key = parsed.OPENAI_API_KEY
    return typeof key === 'string' ? key.trim() : ''
  } catch {
    return ''
  }
}

/**
 * 结构化字段改动 → 重写 config.toml / auth.json。
 * 保留当前 toml 里的 providerId / wire_api / name，避免预设的 chat/responses 被冲掉。
 */
export function setCodexStructuredFields(
  currentToml: string,
  currentAuth: string,
  patch: { baseUrl?: string; model?: string; apiKey?: string },
): { configToml: string; authJson: string } {
  const meta = extractCodexMeta(currentToml || DEFAULT_CODEX_CONFIG_TOML)
  const baseUrl = patch.baseUrl ?? extractCodexBaseUrl(currentToml)
  const model = ((patch.model ?? extractCodexModel(currentToml)) || 'gpt-5.5').trim() || 'gpt-5.5'
  const configToml = buildCodexProviderConfigToml(
    meta.providerName,
    baseUrl.trim(),
    model,
    meta.wireApi,
    meta.providerId,
  )
  let authJson = currentAuth.trim() ? currentAuth : DEFAULT_CODEX_AUTH_JSON
  if (patch.apiKey !== undefined) {
    authJson = mergeOpenAiApiKey(authJson, patch.apiKey)
  }
  return { configToml, authJson }
}

/**
 * 应用预设。
 * - 保留用户已在 auth.json 里填过的 OPENAI_API_KEY（换预设不必重输）
 * - 自定义：只套默认模板，不强制改名称
 */
export function applyCodexPreset(
  presetId: CodexPresetId,
  currentAuthJson: string,
): { configToml: string; authJson: string; name?: string } {
  const preset =
    presetId === CODEX_CUSTOM_PRESET_ID
      ? CODEX_CUSTOM_PRESET
      : CODEX_RELAY_PRESETS.find((p) => p.id === presetId) ?? CODEX_CUSTOM_PRESET

  const preservedKey = extractOpenAiApiKey(currentAuthJson)
  let authJson = preset.authJson
  if (preservedKey) {
    authJson = mergeOpenAiApiKey(preset.authJson, preservedKey)
  }

  return {
    configToml: preset.configToml,
    authJson,
    name: preset.id === CODEX_CUSTOM_PRESET_ID ? undefined : preset.name,
  }
}

export function detectCodexPresetId(configToml: string): CodexPresetId {
  const baseUrl = extractCodexBaseUrl(configToml)
  if (!baseUrl) return CODEX_CUSTOM_PRESET_ID
  for (const preset of CODEX_RELAY_PRESETS) {
    const presetUrl = extractCodexBaseUrl(preset.configToml)
    if (presetUrl && baseUrl === presetUrl) return preset.id
  }
  return CODEX_CUSTOM_PRESET_ID
}

function mergeOpenAiApiKey(authJsonTemplate: string, apiKey: string): string {
  try {
    const parsed = JSON.parse(authJsonTemplate) as Record<string, unknown>
    parsed.OPENAI_API_KEY = apiKey
    return JSON.stringify(parsed, null, 2)
  } catch {
    return `{\n  "OPENAI_API_KEY": ${JSON.stringify(apiKey)}\n}`
  }
}

/** 新建默认：自定义模板（空 key） */
export function initialCodexTomlAuth(initial?: {
  configToml?: string
  authJson?: string
} | null): { configToml: string; authJson: string } {
  if (initial?.configToml?.trim() || initial?.authJson?.trim()) {
    return {
      configToml: initial?.configToml ?? '',
      authJson: initial?.authJson ?? '',
    }
  }
  return {
    configToml: DEFAULT_CODEX_CONFIG_TOML,
    authJson: DEFAULT_CODEX_AUTH_JSON,
  }
}

/**
 * 保存前校验 config.toml。
 * 不引入前端 toml 依赖：做与 materialize 对齐的关键结构检查 + 基础语法嗅探。
 * 返回 null = 通过；否则为错误说明（可直接给 UI）。
 */
export function validateCodexConfigToml(raw: string): string | null {
  const text = raw.replace(/^\uFEFF/, '').trim()
  if (!text) return 'empty'

  // 逐行嗅探明显坏语法（未闭合引号、非 key=value / 表头 / 注释）
  for (const line of text.split(/\r?\n/)) {
    const stripped = line.replace(/(^|[^\\])#.*$/, '$1').trim()
    if (!stripped) continue
    if (stripped.startsWith('[')) {
      // [table] / [[array]]
      if (!/^\[{1,2}[A-Za-z0-9_.-]+\]{1,2}$/.test(stripped)) {
        return `bad_table:${stripped}`
      }
      continue
    }
    // key = value
    if (!/^[A-Za-z0-9_.-]+\s*=\s*\S/.test(stripped)) {
      return `bad_line:${stripped}`
    }
    // 双引号字符串未闭合（忽略转义 \"）
    const eq = stripped.indexOf('=')
    const rhs = stripped.slice(eq + 1).trim()
    if (rhs.startsWith('"')) {
      let i = 1
      let closed = false
      while (i < rhs.length) {
        if (rhs[i] === '\\') {
          i += 2
          continue
        }
        if (rhs[i] === '"') {
          closed = true
          break
        }
        i += 1
      }
      if (!closed) return `unclosed_string:${stripped}`
    }
  }

  // materialize / codex 实际依赖的键
  if (!/^model\s*=/m.test(text)) return 'missing_model'
  if (!/model_provider\s*=/.test(text)) return 'missing_model_provider'
  if (!/base_url\s*=/.test(text)) return 'missing_base_url'
  // codex 校验每一张 model_providers 表必须有 name
  if (!/^name\s*=/m.test(text)) return 'missing_name'
  if (!/\[model_providers\./.test(text)) return 'missing_model_providers_table'

  return null
}
