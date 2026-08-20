import { useEffect, useState } from 'react'
import { Code2, Eye, FilePen, Layers, ListChecks, Network, ShieldAlert, ShieldCheck, ShieldQuestion, Sparkles, Terminal, Wand2, Zap } from 'lucide-react'
import { APPROVAL_POLICY_OPTIONS } from './approvalPolicies'
import { chatApi, type DshAgentPresetOption } from './api'
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
  /** 内置 Agent 会话 + composer：Kivio 三档当前值。 */
  agentPlanMode?: AgentPlanMode | null
}

export interface PermissionModes {
  options: ModeOption[]
  current: string
}

/** Kivio Agent 三档 —— 仅内置 Agent 运行时显示；Kivio Chat 不显示此胶囊。 */
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

/** dsh 四档 Agent 模式 —— 仅 `externalAgentId === 'dsh'` 时显示，画在权限胶囊左边。 */
export const DSH_PRESET_OPTIONS: ModeOption[] = [
  {
    value: 'standard',
    label: '标准模式',
    description: '完整编码 Agent · Full coding agent',
    icon: Sparkles,
    tone: 'neutral',
  },
  {
    value: 'code',
    label: 'PTC 模式',
    description: 'Code Mode SDK · 多步工具写成一个程序',
    icon: Code2,
    tone: 'neutral',
  },
  {
    value: 'minimal',
    label: '极简模式',
    description: '仅 bash + 编辑器 · Bash and editor only',
    icon: Terminal,
    tone: 'neutral',
  },
  {
    value: 'cordis',
    label: '创造模式',
    description: '编写 Agent preset · Author presets',
    icon: Wand2,
    tone: 'violet',
  },
]

export function deriveDshPresetModes(
  agentRuntime: AgentRuntimeConfig,
  customPresets: DshAgentPresetOption[] = [],
): PermissionModes {
  const usesDsh = agentRuntime.kind === 'external'
    && (agentRuntime.externalAgentId ?? agentRuntime.external_agent_id) === 'dsh'
  if (!usesDsh) return { options: [], current: '' }

  const options = [...DSH_PRESET_OPTIONS]
  const seen = new Set(options.map((option) => option.value))
  for (const preset of customPresets) {
    const id = preset.id.trim()
    if (!id || seen.has(id)) continue
    seen.add(id)
    options.push({
      value: id,
      label: (preset.label || id).trim() || id,
      description: preset.description?.trim() || '自定义 Agent preset · Custom preset',
      icon: Layers,
      tone: 'neutral',
    })
  }

  const selected = agentRuntime.externalAgentPreset ?? agentRuntime.external_agent_preset ?? ''
  if (selected && !seen.has(selected)) {
    options.push({
      value: selected,
      label: selected,
      description: '自定义 Agent preset · Custom preset',
      icon: Layers,
      tone: 'neutral',
    })
  }
  const current = options.some((option) => option.value === selected)
    ? selected
    : DSH_PRESET_OPTIONS[0].value
  return { options, current }
}

/** dsh 会话挂载时扫一遍用户 preset 目录；失败则只保留官方四档。 */
export function useDshCustomPresets(agentRuntime: AgentRuntimeConfig): DshAgentPresetOption[] {
  const isDsh = agentRuntime.kind === 'external'
    && (agentRuntime.externalAgentId ?? agentRuntime.external_agent_id) === 'dsh'
  const [presets, setPresets] = useState<DshAgentPresetOption[]>([])
  useEffect(() => {
    if (!isDsh) {
      setPresets([])
      return
    }
    let active = true
    void chatApi.listDshAgentPresets()
      .then((list) => {
        if (active) setPresets(Array.isArray(list) ? list : [])
      })
      .catch(() => {
        if (active) setPresets([])
      })
    return () => {
      active = false
    }
  }, [isDsh])
  return isDsh ? presets : []
}

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
 * - Kivio Chat 运行时：不是 Agent，底栏不显示 Act/Plan/Orchestrate。
 * - 内置 Agent 会话：底栏是 Act / Plan / Orchestrate，顶栏是工具审批策略。
 */
export function derivePermissionModes({
  target,
  agentRuntime,
  agents = [],
  approvalPolicy,
  agentPlanMode,
}: PermissionModesInput): PermissionModes {
  const usesExternal = agentRuntime.kind === 'external' && !!agentRuntime.externalAgentId
  const usesChat = agentRuntime.kind === 'chat'

  if (usesExternal) {
    if (target === 'titlebar') return { options: [], current: '' }
    return externalSandboxModes(agentRuntime, agents)
  }

  // Kivio Chat is a separate runtime (not an agent strategy mode).
  if (usesChat) {
    return { options: [], current: '' }
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
