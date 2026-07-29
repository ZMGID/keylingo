import { useEffect, useState } from 'react'
import { Eye, FilePen, ListChecks, Network, ShieldAlert, ShieldCheck, ShieldQuestion, Zap } from 'lucide-react'
import { APPROVAL_POLICY_OPTIONS } from './approvalPolicies'
import { chatApi } from './api'
import type { AgentPlanMode, AgentRuntimeConfig, DetectedExternalAgent } from './types'

/** 胶囊配色语义：Act=neutral、Plan=emerald、Orchestrate=violet；本地 CLI 档位统一 neutral。 */
export type ModeTone = 'neutral' | 'emerald' | 'violet'

export interface ModeOption {
  value: string
  label: string
  /** 菜单里的副标题；本地 CLI 档位没有描述文本。 */
  description?: string
  icon: typeof Zap
  tone: ModeTone
}

/** 哪个控件在问档位：顶栏权限按钮，还是底栏模式胶囊。 */
export type ModeTarget = 'titlebar' | 'composer'

export interface PermissionModesInput {
  target: ModeTarget
  agentRuntime: AgentRuntimeConfig
  /** 探测到的本地 CLI 列表（档位表的唯一来源）；只有 composer 需要。 */
  agents?: DetectedExternalAgent[]
  /** 内置会话 + titlebar：工具审批策略当前值。 */
  approvalPolicy?: string | null
  /** 内置会话 + composer：Kivio 三档当前值。 */
  agentPlanMode?: AgentPlanMode | null
}

export interface PermissionModes {
  options: ModeOption[]
  current: string
}

/** Kivio 内置三档 —— 底栏胶囊在内置模型会话下的档位表。 */
export const AGENT_MODE_OPTIONS: ModeOption[] = [
  { value: 'act', label: 'Act', description: '普通模式 · Normal', icon: Zap, tone: 'neutral' },
  { value: 'plan', label: 'Plan', description: '计划模式 · Enter plan mode', icon: ListChecks, tone: 'emerald' },
  {
    value: 'orchestrate',
    label: 'Orchestrate',
    description: '主动派 Subagent · Proactive subagents',
    icon: Network,
    tone: 'violet',
  },
]

/** Distinct icon per permission level so the capsule reflects the active mode at a glance.
 *  Covers built-in approval policies (by value) and external CLI sandbox levels (by label). */
export function modeIcon(value: string, label: string) {
  if (value === 'always_confirm') return ShieldAlert
  if (value === 'readonly_auto_sensitive_confirm') return ShieldQuestion
  if (value === 'auto') return ShieldCheck
  if (/计划|只读|read|plan/i.test(label)) return Eye
  if (/编辑|edit/i.test(label)) return FilePen
  if (/完全|默认|full|default/i.test(label)) return ShieldCheck
  return ShieldAlert
}

function externalSandboxModes(
  agentRuntime: AgentRuntimeConfig,
  agents: DetectedExternalAgent[],
): PermissionModes {
  const agent = agents.find((item) => item.id === agentRuntime.externalAgentId)
  const raw = agent?.sandboxOptions ?? agent?.sandbox_options ?? []
  const options: ModeOption[] = raw.map((option) => ({
    value: option.id,
    label: option.label,
    icon: modeIcon(option.id, option.label),
    tone: 'neutral',
  }))
  // 未显式选过就跟随 CLI 自己标了「默认」的那档（claude=完全、codex=工作区写）。
  const fallback = raw.find((option) => option.label.includes('默认')) ?? raw[0]
  const current = agentRuntime.externalSandbox || fallback?.id || ''
  return { options, current }
}

/**
 * 档位推导的唯一入口。空 options 表示该控件此刻无档位可选 → 调用方不渲染。
 *
 * - 本地 CLI 会话：档位归**底栏胶囊**一处管（顶栏返回空表所以隐藏），避免两个控件
 *   写同一个设置；CLI 本身没有档位（如 opencode）时底栏也返回空表。
 * - 内置模型会话：底栏仍是 Kivio 的 Act / Plan / Orchestrate，顶栏仍是工具审批策略。
 */
export function derivePermissionModes({
  target,
  agentRuntime,
  agents = [],
  approvalPolicy,
  agentPlanMode,
}: PermissionModesInput): PermissionModes {
  const usesExternal = agentRuntime.kind === 'external' && !!agentRuntime.externalAgentId

  if (usesExternal) {
    if (target === 'titlebar') return { options: [], current: '' }
    return externalSandboxModes(agentRuntime, agents)
  }

  if (target === 'composer') {
    const current = AGENT_MODE_OPTIONS.some((option) => option.value === agentPlanMode)
      ? (agentPlanMode as string)
      : AGENT_MODE_OPTIONS[0].value
    return { options: AGENT_MODE_OPTIONS, current }
  }

  const options: ModeOption[] = APPROVAL_POLICY_OPTIONS.map((option) => ({
    value: option.value,
    label: option.label,
    description: option.description,
    icon: modeIcon(option.value, option.label),
    tone: 'neutral',
  }))
  return { options, current: approvalPolicy ?? APPROVAL_POLICY_OPTIONS[1]?.value ?? '' }
}

/** 本地 CLI 档位表要读探测到的 agents 列表（后端长 TTL 缓存，切会话不会重探）。 */
export function useDetectedExternalAgents(conversationId?: string | null): DetectedExternalAgent[] {
  const [agents, setAgents] = useState<DetectedExternalAgent[]>([])
  useEffect(() => {
    let active = true
    void chatApi.detectExternalAgents(false, conversationId)
      .then((list) => {
        if (active) setAgents(list)
      })
      .catch(() => {
        if (active) setAgents([])
      })
    return () => {
      active = false
    }
  }, [conversationId])
  return agents
}
