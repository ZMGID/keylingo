// 共享的 chatTools 相关常量 / 钳制 / 格式化 / MCP 服务器辅助函数。
// 从 SettingsShell 抽出，供 SettingsShell、McpCenter、SkillCenter 复用，避免重复实现。

import { type ChatMcpServer, type ChatToolsConfig, defaultNativeTools } from '../api/tauri'

export const CHAT_TOOL_DEFAULT_ROUNDS = 20
export const CHAT_TOOL_MIN_ROUNDS = 1
export const CHAT_TOOL_MAX_ROUNDS = 100
export const CHAT_TOOL_ROUND_PRESETS = [5, 10, 20, 50, 100]
export const CHAT_TOOL_TIMEOUT_PRESETS_MS = [30_000, 60_000, 120_000, 300_000]
// MCP 持久连接空闲超时预设（ms）。后端钳制范围 60s..24h，默认 10 分钟。
export const MCP_IDLE_TIMEOUT_PRESETS_MS = [60_000, 300_000, 600_000, 1_800_000, 3_600_000]
export const MCP_IDLE_TIMEOUT_MIN_MS = 60_000
export const MCP_IDLE_TIMEOUT_MAX_MS = 24 * 60 * 60 * 1_000
// 子 agent 并发预设。后端钳制范围 1..64，默认 12。
export const SUB_AGENT_CONCURRENCY_PRESETS = [3, 6, 12, 24, 48]
export const SUB_AGENT_CONCURRENCY_MIN = 1
export const SUB_AGENT_CONCURRENCY_MAX = 64

export function clampToolRounds(value: string | number | null | undefined): number {
  const parsed = Number(value ?? CHAT_TOOL_DEFAULT_ROUNDS)
  if (!Number.isFinite(parsed)) return CHAT_TOOL_DEFAULT_ROUNDS
  return Math.min(CHAT_TOOL_MAX_ROUNDS, Math.max(CHAT_TOOL_MIN_ROUNDS, Math.round(parsed)))
}

export function clampToolTimeoutMs(value: string | number | null | undefined): number {
  const parsed = Number(value ?? 60_000)
  if (!Number.isFinite(parsed)) return 60_000
  return Math.min(300_000, Math.max(1_000, Math.round(parsed)))
}

export function clampMcpIdleTimeoutMs(value: string | number | null | undefined): number {
  const parsed = Number(value ?? 600_000)
  if (!Number.isFinite(parsed)) return 600_000
  return Math.min(MCP_IDLE_TIMEOUT_MAX_MS, Math.max(MCP_IDLE_TIMEOUT_MIN_MS, Math.round(parsed)))
}

export function clampSubAgentConcurrency(value: string | number | null | undefined): number {
  const parsed = Number(value ?? 12)
  if (!Number.isFinite(parsed)) return 12
  return Math.min(SUB_AGENT_CONCURRENCY_MAX, Math.max(SUB_AGENT_CONCURRENCY_MIN, Math.round(parsed)))
}

export function formatToolRoundsLabel(rounds: number, lang: string): string {
  return lang === 'zh' ? `${rounds} 轮` : `${rounds} rounds`
}

export function formatToolTimeoutLabel(ms: number, lang: string): string {
  if (ms % 60_000 === 0) {
    const minutes = ms / 60_000
    return lang === 'zh' ? `${minutes} 分钟` : `${minutes} min`
  }
  if (ms % 1000 === 0) {
    const seconds = ms / 1000
    return lang === 'zh' ? `${seconds} 秒` : `${seconds} sec`
  }
  return `${ms} ms`
}

export function defaultChatTools(): ChatToolsConfig {
  return {
    enabled: false,
    servers: [],
    hooks: [],
    skillScanPaths: [],
    skillAutoMatch: true,
    skillFallbackMode: 'progressive',
    disabledSkillIds: [],
    maxToolRounds: CHAT_TOOL_DEFAULT_ROUNDS,
    toolTimeoutMs: 60_000,
    mcpIdleTimeoutMs: 600_000,
    approvalPolicy: 'readonly_auto_sensitive_confirm',
    subAgentConcurrency: 12,
    requestDebugEnabled: false,
    nativeTools: defaultNativeTools(),
  }
}

export function newMcpServer(): ChatMcpServer {
  return {
    id: `mcp-${Date.now()}`,
    name: 'New MCP Server',
    enabled: false,
    transport: 'stdio',
    url: '',
    command: '',
    args: [],
    env: {},
    headers: {},
    cwd: null,
    enabledTools: [],
  }
}

export function envToText(env: Record<string, string>): string {
  return Object.entries(env)
    .map(([key, value]) => `${key}=${value}`)
    .join('\n')
}

export function textToEnv(text: string): Record<string, string> {
  const env: Record<string, string> = {}
  for (const line of text.split('\n')) {
    const normalized = line.replace(/\r$/, '')
    if (!normalized.trim()) continue
    const separator = normalized.indexOf('=')
    const key = (separator >= 0 ? normalized.slice(0, separator) : normalized).trim()
    if (!key) continue
    env[key] = separator >= 0 ? normalized.slice(separator + 1) : ''
  }
  return env
}

export function argsToText(args: string[]): string {
  return args.join('\n')
}

export function textToArgs(text: string): string[] {
  return text
    .split('\n')
    .map((arg) => arg.replace(/\r$/, ''))
    .filter((arg) => arg !== '')
}
