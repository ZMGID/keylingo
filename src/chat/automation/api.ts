import { api } from '../../api/tauri'
import type {
  Automation,
  AutomationMeta,
  AutomationRunStarted,
  AutomationRunSummary,
} from './types'

export const automationApi = {
  list: () => api.automationList(),
  get: (id: string) => api.automationGet(id),
  save: (automation: Automation) => api.automationSave(automation),
  remove: (id: string) => api.automationDelete(id),
  setEnabled: (id: string, enabled: boolean) => api.automationSetEnabled(id, enabled),
  run: (id: string, untilNodeId?: string) => api.automationRun(id, untilNodeId),
  cancel: (id: string) => api.automationCancel(id),
  listRuns: (id: string) => api.automationRunsList(id),
  activeRun: (id: string) => api.automationActiveRun(id),
  getRun: (id: string, runId: string) => api.automationRunGet(id, runId),
  testNode: (id: string, nodeId: string, input: import('./types').NodeOutput) => api.automationTestNode(id, nodeId, input),
  validate: (automation: Automation) => api.automationValidate(automation),
  exportToFile: (id: string, path: string) => api.automationExport(id, path),
  importFromFile: (path: string) => api.automationImport(path),
}

export type { Automation, AutomationMeta, AutomationRunStarted, AutomationRunSummary }
