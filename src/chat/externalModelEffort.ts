import type { AgentRuntimeConfig } from './types'

/** Antigravity 返回的档位变体已经在模型 ID 中指定强度。 */
export function modelIncludesEffort(agentId: string | null | undefined, modelId: string | null | undefined, label?: string): boolean {
  return agentId === 'antigravity' && !!modelId && modelId !== 'default' && (
    /-(low|medium|high)$/i.test(modelId) || /\((low|medium|high)\)\s*$/i.test(label ?? '')
  )
}

export function withExternalModel(runtime: AgentRuntimeConfig, model: string, reasoning?: string | null): AgentRuntimeConfig {
  return {
    ...runtime,
    kind: 'external',
    externalModel: model,
    externalReasoning: modelIncludesEffort(runtime.externalAgentId, model)
      ? null
      : reasoning === undefined ? runtime.externalReasoning ?? null : reasoning,
  }
}
