import {
  BUILTIN_AGENT_RUNTIME,
  CHAT_AGENT_RUNTIME,
  type AgentRuntimeConfig,
} from './api'

/**
 * 记住顶栏最后一次选的运行时（外部 CLI / 模型 / 思考档），作为欢迎页和新对话草稿。
 * 与 lastModel / lastThinkingLevel 同一口径：仅前端偏好，存 localStorage。
 *
 * `byAgent` 按代理各记一份。切走再切回时 RuntimePicker 会发 default，
 * 没有这张表就会把上次的模型和思考档清成 Auto。
 */
export const LAST_AGENT_RUNTIME_KEY = 'kivio.chat.lastAgentRuntime'

export type ExternalRuntimeSnapshot = {
  externalModel: string
  externalReasoning: string | null
  externalSandbox: string | null
  externalAgentPreset: string | null
}

function asNonEmptyString(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value : null
}

function snapshotFromRuntime(runtime: AgentRuntimeConfig): ExternalRuntimeSnapshot | null {
  if (runtime.kind !== 'external' || !runtime.externalAgentId) return null
  return {
    externalModel: runtime.externalModel ?? 'default',
    externalReasoning: runtime.externalReasoning ?? null,
    externalSandbox: runtime.externalSandbox ?? null,
    externalAgentPreset: runtime.externalAgentPreset ?? null,
  }
}

function parseSnapshot(raw: unknown): ExternalRuntimeSnapshot | null {
  if (!raw || typeof raw !== 'object') return null
  const value = raw as Record<string, unknown>
  return {
    externalModel: asNonEmptyString(value.externalModel) ?? 'default',
    externalReasoning: asNonEmptyString(value.externalReasoning),
    externalSandbox: asNonEmptyString(value.externalSandbox),
    externalAgentPreset: asNonEmptyString(value.externalAgentPreset),
  }
}

export function parseLastAgentRuntime(raw: unknown): AgentRuntimeConfig | null {
  if (!raw || typeof raw !== 'object') return null
  const value = raw as Record<string, unknown>
  if (value.kind === 'builtin') return { ...BUILTIN_AGENT_RUNTIME }
  if (value.kind === 'chat') return { ...CHAT_AGENT_RUNTIME }
  if (value.kind !== 'external') return null
  const agentId = asNonEmptyString(value.externalAgentId)
  if (!agentId) return null
  const snap = parseSnapshot(value)
  if (!snap) return null
  return {
    kind: 'external',
    externalAgentId: agentId,
    ...snap,
  }
}

/** 从已解析的偏好里取出某个外部代理上次的模型/思考档。旧数据没有 `byAgent` 时回落到 `last`。 */
export function lastRuntimeForAgentFromStore(
  raw: unknown,
  agentId: string,
): ExternalRuntimeSnapshot | null {
  if (!raw || typeof raw !== 'object' || !agentId) return null
  const value = raw as Record<string, unknown>
  const byAgent = value.byAgent
  if (byAgent && typeof byAgent === 'object') {
    const snap = parseSnapshot((byAgent as Record<string, unknown>)[agentId])
    if (snap) return snap
  }
  const last = parseLastAgentRuntime(raw)
  if (last?.kind === 'external' && last.externalAgentId === agentId) {
    return snapshotFromRuntime(last)
  }
  return null
}

function readStoreRaw(): unknown {
  const raw = window.localStorage.getItem(LAST_AGENT_RUNTIME_KEY)
  if (!raw) return null
  return JSON.parse(raw)
}

export function loadLastAgentRuntime(): AgentRuntimeConfig | null {
  try {
    return parseLastAgentRuntime(readStoreRaw())
  } catch {
    return null
  }
}

export function loadLastRuntimeForAgent(agentId: string): ExternalRuntimeSnapshot | null {
  try {
    return lastRuntimeForAgentFromStore(readStoreRaw(), agentId)
  } catch {
    return null
  }
}

export function rememberedExternalRuntime(agentId: string): AgentRuntimeConfig {
  const snap = loadLastRuntimeForAgent(agentId)
  return {
    kind: 'external',
    externalAgentId: agentId,
    externalModel: snap?.externalModel ?? 'default',
    externalReasoning: snap?.externalReasoning ?? null,
    externalSandbox: snap?.externalSandbox ?? null,
    externalAgentPreset: snap?.externalAgentPreset ?? null,
  }
}

export function saveLastAgentRuntime(runtime: AgentRuntimeConfig): void {
  try {
    const last = parseLastAgentRuntime(runtime)
    if (!last) return
    let byAgent: Record<string, ExternalRuntimeSnapshot> = {}
    try {
      const prev = readStoreRaw()
      if (prev && typeof prev === 'object') {
        const rawMap = (prev as Record<string, unknown>).byAgent
        if (rawMap && typeof rawMap === 'object') {
          for (const [id, snap] of Object.entries(rawMap as Record<string, unknown>)) {
            const parsed = parseSnapshot(snap)
            if (id && parsed) byAgent[id] = parsed
          }
        }
        const prevLast = parseLastAgentRuntime(prev)
        const prevSnap = prevLast ? snapshotFromRuntime(prevLast) : null
        if (prevLast?.kind === 'external' && prevLast.externalAgentId && prevSnap) {
          byAgent = { [prevLast.externalAgentId]: prevSnap, ...byAgent }
        }
      }
    } catch {
      byAgent = {}
    }
    const snap = snapshotFromRuntime(last)
    if (last.kind === 'external' && last.externalAgentId && snap) {
      byAgent[last.externalAgentId] = snap
    }
    window.localStorage.setItem(LAST_AGENT_RUNTIME_KEY, JSON.stringify({ ...last, byAgent }))
  } catch {
    /* ignore */
  }
}
