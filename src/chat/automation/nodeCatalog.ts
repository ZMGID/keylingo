import type { LucideIcon } from 'lucide-react'
import { Bell, Bot, Clock, Keyboard, MousePointerClick } from 'lucide-react'
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
    type: 'action.notify',
    kind: 'action',
    icon: Bell,
    label: (t) => t.chatAutomationActionNotify,
    hint: (t) => t.chatAutomationActionNotifyHint,
    defaultData: (t) => ({
      label: t.chatAutomationActionNotify,
      notify: { body: '' },
    }),
  },
]

export function catalogEntry(type: string): NodeCatalogEntry | undefined {
  return TRIGGER_CATALOG.find((entry) => entry.type === type)
    ?? ACTION_CATALOG.find((entry) => entry.type === type)
}
