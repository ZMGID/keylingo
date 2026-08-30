import type { ChatToolDefinition } from '../../api/tauri'

/** Native tools the registry marks read-only (mcp/native_registry.rs). Memory is excluded separately. */
const NATIVE_READ_ONLY = new Set([
  'web_search',
  'search_web',
  'web_fetch',
  'knowledge_search',
  'advisor',
  'read',
  'ls',
  'grep',
  'glob',
  'bash_output',
  'present_artifacts',
])

export function isMemoryTool(tool: { name?: string, id?: string }): boolean {
  const name = (tool.name || '').toLowerCase()
  const id = (tool.id || '').toLowerCase()
  return name.startsWith('memory_') || id.includes('memory_')
}

/** Workflow agents must not list/create/run automations (recursion). */
export function isAutomationControlTool(tool: { name?: string, id?: string }): boolean {
  const name = (tool.name || '').toLowerCase()
  const id = (tool.id || '').toLowerCase()
  return name.startsWith('automation_') || id.includes('automation_')
}

export function isSkillActivateTool(tool: { source?: string, name?: string }): boolean {
  return tool.source === 'skill' || tool.name === 'skill'
}

function annotationBool(annotations: unknown, key: string): boolean | undefined {
  if (!annotations || typeof annotations !== 'object') return undefined
  const value = (annotations as Record<string, unknown>)[key]
  return typeof value === 'boolean' ? value : undefined
}

export function isAutomationAlwaysOnTool(tool: ChatToolDefinition): boolean {
  if (isMemoryTool(tool) || isAutomationControlTool(tool)) return false
  if (isSkillActivateTool(tool)) return true
  if (tool.source === 'mcp') {
    return annotationBool(tool.annotations, 'readOnlyHint') === true
      && annotationBool(tool.annotations, 'destructiveHint') !== true
      && annotationBool(tool.annotations, 'openWorldHint') !== true
  }
  if (tool.source === 'native') return NATIVE_READ_ONLY.has(tool.name)
  return false
}

/** Write / side-effect tools the Tool slot can opt into. Skill + read-only are not in this list. */
export function isAutomationOptInTool(tool: ChatToolDefinition): boolean {
  return !isMemoryTool(tool) && !isAutomationControlTool(tool) && !isAutomationAlwaysOnTool(tool)
}

export function pruneAlwaysOnToolIds(toolIds: string[], tools: ChatToolDefinition[]): string[] {
  if (tools.length === 0) return toolIds
  return toolIds.filter((id) => {
    const tool = tools.find((item) => item.id === id || item.name === id)
    if (!tool) return true
    return isAutomationOptInTool(tool)
  })
}
