import type { LucideIcon } from 'lucide-react'
import { Bell, Bot, Clipboard, Clock, FileText, FolderOpen, GitBranch, Globe, Keyboard, MousePointerClick, Sparkles, Split, SquarePen, Terminal, Timer, Wrench } from 'lucide-react'
import { agentSelectedModel, normalizeAgent } from './agentModel'
import type { I18n } from '../../settings/i18n'
import type { AgentSlot, AutomationNodeType, FlowNodeData } from './types'

function clip(text: string, max = 32): string {
  const oneLine = text.replace(/\s+/g, ' ').trim()
  return oneLine.length > max ? `${oneLine.slice(0, max)}…` : oneLine
}

/** 卡片下方的副标题（n8n 的 operation 描述位）：从节点配置里提炼一句话。 */
export function nodeSummary(type: string, data: FlowNodeData, t: I18n): string {
  switch (type) {
    case 'trigger.schedule': {
      const schedule = data.schedule
      if (!schedule) return ''
      if (schedule.kind === 'interval') {
        return `${t.chatAutomationScheduleInterval} ${schedule.intervalMinutes} ${t.chatAutomationMinutes}`
      }
      const time = `${String(schedule.hour).padStart(2, '0')}:${String(schedule.minute).padStart(2, '0')}`
      const kind = schedule.kind === 'weekdays'
        ? t.chatAutomationScheduleWeekdays
        : t.chatAutomationScheduleDaily
      return `${kind} ${time}`
    }
    case 'trigger.hotkey':
      return data.hotkey?.accelerator?.trim() ?? ''
    case 'action.agent':
      return ''
    case 'agent.runtime':
      return agentSelectedModel(normalizeAgent(data.agent))
    case 'agent.context':
      return clip(normalizeAgent(data.agent).prompt)
    case 'agent.tool': {
      const ids = normalizeAgent(data.agent).toolIds
      return ids.length ? String(ids.length) : t.chatAutomationAgentToolsAll
    }
    case 'agent.skill':
      return normalizeAgent(data.agent).skillIds.join(', ')
    case 'action.notify':
      return clip(data.notify?.body ?? '')
    case 'action.set': {
      const fields = data.set?.fields ?? []
      const keys = fields.map((field) => field.key.trim()).filter(Boolean)
      return keys.length ? keys.join(', ') : ''
    }
    case 'logic.delay':
      return data.delay ? `${data.delay.seconds}s` : ''
    case 'action.clipboard':
      return data.clipboard?.op === 'read' ? t.chatAutomationClipboardRead : t.chatAutomationClipboardCopy
    case 'action.file':
      return [data.file?.op === 'write' ? t.chatAutomationFileWrite : t.chatAutomationFileRead, clip(data.file?.path ?? '', 20)]
        .filter((item) => item && item.trim())
        .join(' ')
    case 'action.command':
      return clip(data.command?.command ?? '')
    case 'action.http': {
      const http = data.http
      if (!http?.url?.trim()) return http?.method ?? ''
      let host = http.url.trim()
      try {
        host = new URL(host).host || host
      } catch {
        // 未填完整 URL 时原样展示
      }
      return `${http.method} ${clip(host, 24)}`
    }
    case 'logic.if': {
      const cond = data.if
      if (!cond) return ''
      const op = cond.op === 'equals'
        ? t.chatAutomationIfEquals
        : cond.op === 'notEmpty'
          ? t.chatAutomationIfNotEmpty
          : t.chatAutomationIfContains
      return cond.op === 'notEmpty' ? op : `${op} ${clip(cond.value, 18)}`
    }
    case 'logic.switch': {
      const n = data.switch?.cases?.length ?? 0
      return n ? `${n}` : ''
    }
    default:
      return ''
  }
}

export interface NodeCatalogEntry {
  type: AutomationNodeType
  kind: 'trigger' | 'action'
  icon: LucideIcon
  label: (t: I18n) => string
  hint: (t: I18n) => string
  defaultData: (t: I18n) => FlowNodeData
}

export const TRIGGER_CATALOG: NodeCatalogEntry[] = [
  {
    type: 'trigger.manual',
    kind: 'trigger',
    icon: MousePointerClick,
    label: (t) => t.chatAutomationTriggerManual,
    hint: (t) => t.chatAutomationTriggerManualHint,
    defaultData: (t) => ({ label: t.chatAutomationTriggerManual }),
  },
  {
    type: 'trigger.schedule',
    kind: 'trigger',
    icon: Clock,
    label: (t) => t.chatAutomationTriggerSchedule,
    hint: (t) => t.chatAutomationTriggerScheduleHint,
    defaultData: (t) => ({
      label: t.chatAutomationTriggerSchedule,
      schedule: { kind: 'daily', hour: 9, minute: 0, intervalMinutes: 60 },
    }),
  },
  {
    type: 'trigger.hotkey',
    kind: 'trigger',
    icon: Keyboard,
    label: (t) => t.chatAutomationTriggerHotkey,
    hint: (t) => t.chatAutomationTriggerHotkeyHint,
    defaultData: (t) => ({
      label: t.chatAutomationTriggerHotkey,
      hotkey: { accelerator: '' },
    }),
  },
]

export const CORE_CATALOG: NodeCatalogEntry[] = [
  {
    type: 'action.command',
    kind: 'action',
    icon: Terminal,
    label: (t) => t.chatAutomationActionCommand,
    hint: (t) => t.chatAutomationActionCommandHint,
    defaultData: (t) => ({
      label: t.chatAutomationActionCommand,
      command: { command: '', cwd: '', timeoutSeconds: 30, continueOnFail: false },
    }),
  },
  {
    type: 'action.agent',
    kind: 'action',
    icon: Bot,
    label: (t) => t.chatAutomationActionAgent,
    hint: (t) => t.chatAutomationActionAgentHint,
    defaultData: (t) => ({
      label: t.chatAutomationActionAgent,
    }),
  },
  {
    type: 'action.http',
    kind: 'action',
    icon: Globe,
    label: (t) => t.chatAutomationActionHttp,
    hint: (t) => t.chatAutomationActionHttpHint,
    defaultData: (t) => ({
      label: t.chatAutomationActionHttp,
      http: { method: 'GET', url: '', headers: '', body: '' },
    }),
  },
  {
    type: 'action.set',
    kind: 'action',
    icon: SquarePen,
    label: (t) => t.chatAutomationActionSet,
    hint: (t) => t.chatAutomationActionSetHint,
    defaultData: (t) => ({
      label: t.chatAutomationActionSet,
      set: { fields: [{ key: 'text', value: '{{output}}' }] },
    }),
  },
  {
    type: 'action.file',
    kind: 'action',
    icon: FolderOpen,
    label: (t) => t.chatAutomationActionFile,
    hint: (t) => t.chatAutomationActionFileHint,
    defaultData: (t) => ({
      label: t.chatAutomationActionFile,
      file: { op: 'write', path: 'output.txt', content: '{{output}}' },
    }),
  },
  {
    type: 'action.clipboard',
    kind: 'action',
    icon: Clipboard,
    label: (t) => t.chatAutomationActionClipboard,
    hint: (t) => t.chatAutomationActionClipboardHint,
    defaultData: (t) => ({
      label: t.chatAutomationActionClipboard,
      clipboard: { op: 'copy', text: '{{output}}' },
    }),
  },
  {
    type: 'action.notify',
    kind: 'action',
    icon: Bell,
    label: (t) => t.chatAutomationActionNotify,
    hint: (t) => t.chatAutomationActionNotifyHint,
    defaultData: (t) => ({
      label: t.chatAutomationActionNotify,
      notify: { body: '{{output}}' },
    }),
  },
]

export const FLOW_CATALOG: NodeCatalogEntry[] = [
  {
    type: 'logic.if',
    kind: 'action',
    icon: GitBranch,
    label: (t) => t.chatAutomationActionIf,
    hint: (t) => t.chatAutomationActionIfHint,
    defaultData: (t) => ({
      label: t.chatAutomationActionIf,
      if: { op: 'contains', value: '' },
    }),
  },
  {
    type: 'logic.switch',
    kind: 'action',
    icon: Split,
    label: (t) => t.chatAutomationActionSwitch,
    hint: (t) => t.chatAutomationActionSwitchHint,
    defaultData: (t) => ({
      label: t.chatAutomationActionSwitch,
      switch: {
        cases: [
          { id: '1', op: 'equals', value: '' },
          { id: '2', op: 'equals', value: '' },
        ],
      },
    }),
  },
  {
    type: 'logic.delay',
    kind: 'action',
    icon: Timer,
    label: (t) => t.chatAutomationActionDelay,
    hint: (t) => t.chatAutomationActionDelayHint,
    defaultData: (t) => ({
      label: t.chatAutomationActionDelay,
      delay: { seconds: 5 },
    }),
  },
]

export const ACTION_CATALOG: NodeCatalogEntry[] = [...CORE_CATALOG, ...FLOW_CATALOG]

export type CatalogGroupId = 'core' | 'flow' | 'trigger'

export interface CatalogGroup {
  id: CatalogGroupId
  title: (t: I18n) => string
  hint: (t: I18n) => string
  entries: NodeCatalogEntry[]
}

export function catalogGroups(
  kind: 'trigger' | 'action',
  presentTypes: string[],
): CatalogGroup[] {
  if (kind === 'trigger') {
    return [{
      id: 'trigger',
      title: (t) => t.chatAutomationKindTrigger,
      hint: (t) => t.chatAutomationEmptyCanvas,
      entries: TRIGGER_CATALOG,
    }]
  }
  const extraTriggers = TRIGGER_CATALOG.filter((entry) => !presentTypes.includes(entry.type))
  const groups: CatalogGroup[] = [
    {
      id: 'core',
      title: (t) => t.chatAutomationGroupCore,
      hint: (t) => t.chatAutomationGroupCoreHint,
      entries: CORE_CATALOG,
    },
    {
      id: 'flow',
      title: (t) => t.chatAutomationGroupFlow,
      hint: (t) => t.chatAutomationGroupFlowHint,
      entries: FLOW_CATALOG,
    },
  ]
  if (extraTriggers.length) {
    groups.push({
      id: 'trigger',
      title: (t) => t.chatAutomationAddAnotherTrigger,
      hint: (t) => t.chatAutomationAddAnotherTriggerHint,
      entries: extraTriggers,
    })
  }
  return groups
}

export const SLOT_CATALOG: Record<AgentSlot, NodeCatalogEntry> = {
  runtime: {
    type: 'agent.runtime',
    kind: 'action',
    icon: Bot,
    label: (t) => t.chatAutomationAgentSlotRuntime,
    hint: (t) => t.chatAutomationAgentRuntimeHint,
    defaultData: (t) => ({
      label: t.chatAutomationKivioAgent,
      agent: { prompt: '', runtimeKind: 'builtin', toolIds: [], skillIds: [] },
    }),
  },
  context: {
    type: 'agent.context',
    kind: 'action',
    icon: FileText,
    label: (t) => t.chatAutomationAgentSlotContext,
    hint: (t) => t.chatAutomationAgentContextHint,
    defaultData: (t) => ({
      label: t.chatAutomationAgentSlotContext,
      agent: { prompt: '' },
    }),
  },
  tool: {
    type: 'agent.tool',
    kind: 'action',
    icon: Wrench,
    label: (t) => t.chatAutomationAgentSlotTool,
    hint: (t) => t.chatAutomationAgentToolsHint,
    defaultData: (t) => ({
      label: t.chatAutomationAgentSlotTool,
      agent: { prompt: '', toolIds: [] },
    }),
  },
  skill: {
    type: 'agent.skill',
    kind: 'action',
    icon: Sparkles,
    label: (t) => t.chatAutomationAgentSlotSkill,
    hint: (t) => t.chatAutomationAgentSkillsHint,
    defaultData: (t) => ({
      label: t.chatAutomationAgentSlotSkill,
      agent: { prompt: '', skillIds: [] },
    }),
  },
}

export function catalogEntry(type: string): NodeCatalogEntry | undefined {
  return TRIGGER_CATALOG.find((entry) => entry.type === type)
    ?? ACTION_CATALOG.find((entry) => entry.type === type)
    ?? Object.values(SLOT_CATALOG).find((entry) => entry.type === type)
}
