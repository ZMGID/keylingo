import { api } from '../../api/tauri'
import type { Automation, AutomationMeta } from './types'

export const automationApi = {
  list: () => api.automationList(),
  get: (id: string) => api.automationGet(id),
  save: (automation: Automation) => api.automationSave(automation),
  remove: (id: string) => api.automationDelete(id),
  setEnabled: (id: string, enabled: boolean) => api.automationSetEnabled(id, enabled),
}

export type { Automation, AutomationMeta }
