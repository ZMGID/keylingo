/**
 * Claude Code 第三方供应商预设。
 * 对齐 desktop-cc-gui 的 CLAUDE_PROVIDER_PRESETS（精简常用项，可后续再扩）。
 */

export const OFFICIAL_DIRECT_PRESET_ID = 'official_direct'
export const CUSTOM_PROXY_PRESET_ID = 'custom'
export const OFFICIAL_ANTHROPIC_BASE_URL = 'https://api.anthropic.com'

export type ClaudePresetId =
  | typeof OFFICIAL_DIRECT_PRESET_ID
  | typeof CUSTOM_PROXY_PRESET_ID
  | 'zhipu'
  | 'kimi'
  | 'kimi-coding'
  | 'deepseek'
  | 'minimax'
  | 'xiaomi'
  | 'xiaomi-plan'
  | 'bailian'
  | 'bailian-coding'
  | 'longcat'
  | 'opencode-go'
  | 'openrouter'

export type ClaudePresetBrand =
  | 'claude'
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

export interface ClaudeProviderPreset {
  id: ClaudePresetId
  /** i18n key suffix under externalAgentsPreset* */
  nameKey: string
  brand: ClaudePresetBrand
  env: Record<string, string>
}

/** 中转预设（不含官方 / 自定义） */
export const CLAUDE_RELAY_PRESETS: ClaudeProviderPreset[] = [
  {
    id: 'zhipu',
    nameKey: 'zhipu',
    brand: 'zhipu',
    env: {
      ANTHROPIC_BASE_URL: 'https://open.bigmodel.cn/api/anthropic',
      ANTHROPIC_DEFAULT_FABLE_MODEL: 'glm-5.2',
      ANTHROPIC_DEFAULT_HAIKU_MODEL: 'glm-5.2',
      ANTHROPIC_DEFAULT_SONNET_MODEL: 'glm-5.2',
      ANTHROPIC_DEFAULT_OPUS_MODEL: 'glm-5.2',
    },
  },
  {
    id: 'kimi',
    nameKey: 'kimi',
    brand: 'kimi',
    env: {
      ANTHROPIC_BASE_URL: 'https://api.moonshot.cn/anthropic',
      ANTHROPIC_DEFAULT_FABLE_MODEL: 'kimi-k3',
      ANTHROPIC_DEFAULT_HAIKU_MODEL: 'kimi-k3',
      ANTHROPIC_DEFAULT_SONNET_MODEL: 'kimi-k3',
      ANTHROPIC_DEFAULT_OPUS_MODEL: 'kimi-k3',
    },
  },
  {
    id: 'kimi-coding',
    nameKey: 'kimiCoding',
    brand: 'kimi',
    env: {
      ANTHROPIC_BASE_URL: 'https://api.kimi.com/coding/',
      ANTHROPIC_DEFAULT_FABLE_MODEL: 'kimi-k3',
      ANTHROPIC_DEFAULT_HAIKU_MODEL: 'kimi-k3',
      ANTHROPIC_DEFAULT_SONNET_MODEL: 'kimi-k3',
      ANTHROPIC_DEFAULT_OPUS_MODEL: 'kimi-k3',
      CLAUDE_CODE_MAX_CONTEXT_TOKENS: '262144',
      CLAUDE_CODE_AUTO_COMPACT_WINDOW: '262144',
    },
  },
  {
    id: 'deepseek',
    nameKey: 'deepseek',
    brand: 'deepseek',
    env: {
      ANTHROPIC_BASE_URL: 'https://api.deepseek.com/anthropic',
      ANTHROPIC_DEFAULT_FABLE_MODEL: 'deepseek-v4-pro[1m]',
      ANTHROPIC_DEFAULT_HAIKU_MODEL: 'deepseek-v4-flash',
      ANTHROPIC_DEFAULT_SONNET_MODEL: 'deepseek-v4-pro[1m]',
      ANTHROPIC_DEFAULT_OPUS_MODEL: 'deepseek-v4-pro[1m]',
      CLAUDE_CODE_EFFORT_LEVEL: 'max',
    },
  },
  {
    id: 'minimax',
    nameKey: 'minimax',
    brand: 'minimax',
    env: {
      ANTHROPIC_BASE_URL: 'https://api.minimaxi.com/anthropic',
      API_TIMEOUT_MS: '3000000',
      CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1',
      ANTHROPIC_DEFAULT_FABLE_MODEL: 'MiniMax-M2.1',
      ANTHROPIC_DEFAULT_HAIKU_MODEL: 'MiniMax-M2.1',
      ANTHROPIC_DEFAULT_SONNET_MODEL: 'MiniMax-M2.1',
      ANTHROPIC_DEFAULT_OPUS_MODEL: 'MiniMax-M2.1',
    },
  },
  {
    id: 'xiaomi',
    nameKey: 'xiaomi',
    brand: 'xiaomi',
    env: {
      ANTHROPIC_BASE_URL: 'https://api.xiaomimimo.com/anthropic',
      ANTHROPIC_DEFAULT_FABLE_MODEL: 'mimo-v2.5-pro',
      ANTHROPIC_DEFAULT_HAIKU_MODEL: 'mimo-v2.5-pro',
      ANTHROPIC_DEFAULT_SONNET_MODEL: 'mimo-v2.5-pro',
      ANTHROPIC_DEFAULT_OPUS_MODEL: 'mimo-v2.5-pro',
    },
  },
  {
    id: 'xiaomi-plan',
    nameKey: 'xiaomiPlan',
    brand: 'xiaomi',
    env: {
      ANTHROPIC_BASE_URL: 'https://token-plan-cn.xiaomimimo.com/anthropic',
      ANTHROPIC_DEFAULT_FABLE_MODEL: 'mimo-v2.5-pro',
      ANTHROPIC_DEFAULT_HAIKU_MODEL: 'mimo-v2.5-pro',
      ANTHROPIC_DEFAULT_SONNET_MODEL: 'mimo-v2.5-pro',
      ANTHROPIC_DEFAULT_OPUS_MODEL: 'mimo-v2.5-pro',
    },
  },
  {
    id: 'bailian',
    nameKey: 'bailian',
    brand: 'bailian',
    env: {
      ANTHROPIC_BASE_URL: 'https://dashscope.aliyuncs.com/apps/anthropic',
    },
  },
  {
    id: 'bailian-coding',
    nameKey: 'bailianCoding',
    brand: 'bailian',
    env: {
      ANTHROPIC_BASE_URL: 'https://coding.dashscope.aliyuncs.com/apps/anthropic',
    },
  },
  {
    id: 'longcat',
    nameKey: 'longcat',
    brand: 'longcat',
    env: {
      ANTHROPIC_BASE_URL: 'https://api.longcat.chat/anthropic',
      ANTHROPIC_DEFAULT_FABLE_MODEL: 'LongCat-2.0',
      ANTHROPIC_DEFAULT_HAIKU_MODEL: 'LongCat-2.0',
      ANTHROPIC_DEFAULT_SONNET_MODEL: 'LongCat-2.0',
      ANTHROPIC_DEFAULT_OPUS_MODEL: 'LongCat-2.0',
      CLAUDE_CODE_MAX_OUTPUT_TOKENS: '131072',
      CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC: '1',
    },
  },
  {
    id: 'opencode-go',
    nameKey: 'opencodeGo',
    brand: 'opencode',
    env: {
      ANTHROPIC_BASE_URL: 'https://opencode.ai/zen/go',
      ANTHROPIC_DEFAULT_FABLE_MODEL: 'deepseek-v4-flash',
      ANTHROPIC_DEFAULT_HAIKU_MODEL: 'deepseek-v4-flash',
      ANTHROPIC_DEFAULT_SONNET_MODEL: 'deepseek-v4-flash',
      ANTHROPIC_DEFAULT_OPUS_MODEL: 'deepseek-v4-flash',
    },
  },
  {
    id: 'openrouter',
    nameKey: 'openrouter',
    brand: 'openrouter',
    env: {
      ANTHROPIC_BASE_URL: 'https://openrouter.ai/api',
      ANTHROPIC_DEFAULT_FABLE_MODEL: 'anthropic/claude-fable-5',
      ANTHROPIC_DEFAULT_HAIKU_MODEL: 'anthropic/claude-haiku-4.5',
      ANTHROPIC_DEFAULT_SONNET_MODEL: 'anthropic/claude-sonnet-4.5',
      ANTHROPIC_DEFAULT_OPUS_MODEL: 'anthropic/claude-opus-4.5',
    },
  },
]

export const CLAUDE_CUSTOM_PRESET: ClaudeProviderPreset = {
  id: CUSTOM_PROXY_PRESET_ID,
  nameKey: 'custom',
  brand: 'custom',
  env: {},
}

/** Claude 四档模型映射键 */
export const CLAUDE_TIER_KEYS = [
  { key: 'ANTHROPIC_DEFAULT_OPUS_MODEL', label: 'Opus' },
  { key: 'ANTHROPIC_DEFAULT_SONNET_MODEL', label: 'Sonnet' },
  { key: 'ANTHROPIC_DEFAULT_HAIKU_MODEL', label: 'Haiku' },
  { key: 'ANTHROPIC_DEFAULT_FABLE_MODEL', label: 'Fable' },
] as const

export const CLAUDE_BASE_URL = 'ANTHROPIC_BASE_URL'
/** Claude Code / 中转主用键 */
export const CLAUDE_TOKEN = 'ANTHROPIC_AUTH_TOKEN'
/** 旧配置 / cc-switch / 部分文档仍用这个键；回填时兼容，保存时统一写 AUTH_TOKEN */
export const CLAUDE_TOKEN_ALT = 'ANTHROPIC_API_KEY'

/** 读 API Key：优先 AUTH_TOKEN，其次 API_KEY。 */
export function readClaudeApiKey(env: EnvPair[]): string {
  const auth = env.find((p) => p.key === CLAUDE_TOKEN)?.value?.trim()
  if (auth) return auth
  return env.find((p) => p.key === CLAUDE_TOKEN_ALT)?.value?.trim() ?? ''
}

/**
 * 写 API Key：统一落到 AUTH_TOKEN，并去掉 API_KEY，避免两键并存各说各话。
 * 空值则两个键都删。
 */
export function writeClaudeApiKey(env: EnvPair[], value: string): EnvPair[] {
  const next = env.filter((p) => p.key !== CLAUDE_TOKEN && p.key !== CLAUDE_TOKEN_ALT)
  if (value.trim()) next.push({ key: CLAUDE_TOKEN, value })
  return next
}

export function isOfficialAnthropicEndpoint(baseUrl?: string): boolean {
  const normalized = (baseUrl || '').trim().toLowerCase()
  if (!normalized) return false
  try {
    const url = new URL(normalized)
    return url.hostname === 'api.anthropic.com'
  } catch {
    return false
  }
}

/** 从已有 env 反推当前命中的预设（编辑回填 / 手改 URL 时用）。空 URL = 自定义。 */
export function detectClaudePresetId(env: Record<string, string | undefined>): ClaudePresetId {
  const baseUrl = (env.ANTHROPIC_BASE_URL || '').trim()
  if (!baseUrl) return CUSTOM_PROXY_PRESET_ID
  for (const preset of CLAUDE_RELAY_PRESETS) {
    const presetUrl = (preset.env.ANTHROPIC_BASE_URL || '').trim()
    if (presetUrl && baseUrl === presetUrl) return preset.id
  }
  // 官方 anthropic.com 不再单独成预设 UI，归到自定义
  return CUSTOM_PROXY_PRESET_ID
}

export type EnvPair = { key: string; value: string }

export function envPairsToRecord(env: EnvPair[]): Record<string, string> {
  const out: Record<string, string> = {}
  for (const pair of env) {
    if (!pair.key.trim()) continue
    out[pair.key] = pair.value
  }
  return out
}

export function recordToEnvPairs(env: Record<string, string>): EnvPair[] {
  return Object.entries(env)
    .filter(([, value]) => value !== undefined && value !== null)
    .map(([key, value]) => ({ key, value: String(value) }))
}

/**
 * 应用预设到 env。
 * - 保留用户已填的 API Key（换预设时不必重输）
 * - 官方直连：只锁 BASE_URL，清掉档位覆盖
 * - 中转：用预设 env 覆盖；自定义：只清 BASE_URL/档位相关，其它键保留
 */
export function applyClaudePreset(
  presetId: ClaudePresetId,
  current: EnvPair[],
): EnvPair[] {
  const token = readClaudeApiKey(current)
  // 统一掉旧 API_KEY 键，避免和 AUTH_TOKEN 并存
  const currentRecord = envPairsToRecord(
    current.filter((p) => p.key !== CLAUDE_TOKEN_ALT),
  )
  delete currentRecord[CLAUDE_TOKEN_ALT]

  if (presetId === OFFICIAL_DIRECT_PRESET_ID) {
    const next: Record<string, string> = {
      ...currentRecord,
      [CLAUDE_BASE_URL]: OFFICIAL_ANTHROPIC_BASE_URL,
    }
    for (const tier of CLAUDE_TIER_KEYS) delete next[tier.key]
    if (token) next[CLAUDE_TOKEN] = token
    else delete next[CLAUDE_TOKEN]
    return recordToEnvPairs(next)
  }

  if (presetId === CUSTOM_PROXY_PRESET_ID) {
    const next: Record<string, string> = { ...currentRecord }
    // 自定义：不强制改 URL，只确保结构在
    if (token) next[CLAUDE_TOKEN] = token
    else delete next[CLAUDE_TOKEN]
    return recordToEnvPairs(next)
  }

  const preset = CLAUDE_RELAY_PRESETS.find((p) => p.id === presetId)
  if (!preset) return current

  // 中转：用预设 env 为底，叠回 token；去掉上一个预设可能留下的档位/特殊键冲突
  const next: Record<string, string> = { ...preset.env }
  if (token) next[CLAUDE_TOKEN] = token
  // 保留用户手动加过的非冲突键（非 BASE_URL / 非四档 / 非预设已写键 / 非 token 双键）
  for (const [key, value] of Object.entries(currentRecord)) {
    if (key === CLAUDE_TOKEN || key === CLAUDE_TOKEN_ALT) continue
    if (key === CLAUDE_BASE_URL) continue
    if (CLAUDE_TIER_KEYS.some((t) => t.key === key)) continue
    if (key in preset.env) continue
    next[key] = value
  }
  return recordToEnvPairs(next)
}
