import type { LucideIcon } from 'lucide-react'
import { Bell, Bot, Clock, GitBranch, Globe, Keyboard, MousePointerClick } from 'lucide-react'
import type { I18n } from '../../settings/i18n'
import type { AutomationNodeType, FlowNodeData } from './types'

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

export const ACTION_CATALOG: NodeCatalogEntry[] = [
  {
    type: 'action.agent',
    kind: 'action',
    icon: Bot,
    label: (t) => t.chatAutomationActionAgent,
    hint: (t) => t.chatAutomationActionAgentHint,
    defaultData: (t) => ({
      label: t.chatAutomationActionAgent,
      agent: { prompt: '', skillId: null },
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

export function catalogEntry(type: string): NodeCatalogEntry | undefined {
  return TRIGGER_CATALOG.find((entry) => entry.type === type)
    ?? ACTION_CATALOG.find((entry) => entry.type === type)
}
