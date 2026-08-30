import { describe, expect, it } from 'vitest'
import type { ChatToolDefinition } from '../../api/tauri'
import {
  isAutomationAlwaysOnTool,
  isAutomationOptInTool,
  isSkillActivateTool,
  pruneAlwaysOnToolIds,
} from './agentTools'

function tool(partial: Partial<ChatToolDefinition> & { id: string, name: string }): ChatToolDefinition {
  return {
    description: '',
    source: 'native',
    sensitive: false,
    inputSchema: {},
    ...partial,
  }
}

describe('agentTools', () => {
  it('treats the skill loader as always-on, not an opt-in checkbox', () => {
    const skill = tool({ id: 'skill__activate', name: 'skill', source: 'skill' })
    expect(isSkillActivateTool(skill)).toBe(true)
    expect(isAutomationAlwaysOnTool(skill)).toBe(true)
    expect(isAutomationOptInTool(skill)).toBe(false)
  })

  it('auto-loads native read-only tools and keeps write tools opt-in', () => {
    const read = tool({ id: 'native__read', name: 'read' })
    const bashOutput = tool({ id: 'native__bash_output', name: 'bash_output' })
    const bash = tool({ id: 'native__run_command', name: 'bash', sensitive: true })
    const memory = tool({ id: 'native__memory_read', name: 'memory_read' })
    expect(isAutomationAlwaysOnTool(read)).toBe(true)
    expect(isAutomationAlwaysOnTool(bashOutput)).toBe(true)
    expect(isAutomationOptInTool(bash)).toBe(true)
    expect(isAutomationAlwaysOnTool(memory)).toBe(false)
    expect(isAutomationOptInTool(memory)).toBe(false)
  })

  it('treats MCP readOnlyHint tools as always-on', () => {
    const ro = tool({
      id: 'mcp__notion__search',
      name: 'search',
      source: 'mcp',
      annotations: { readOnlyHint: true },
    })
    const write = tool({
      id: 'mcp__notion__update',
      name: 'update',
      source: 'mcp',
      annotations: { readOnlyHint: false },
    })
    expect(isAutomationAlwaysOnTool(ro)).toBe(true)
    expect(isAutomationOptInTool(write)).toBe(true)
  })

  it('drops skill and read-only ids from the Tool-slot whitelist', () => {
    const catalog = [
      tool({ id: 'skill__activate', name: 'skill', source: 'skill' }),
      tool({ id: 'native__read', name: 'read' }),
      tool({ id: 'native__run_command', name: 'bash', sensitive: true }),
    ]
    expect(pruneAlwaysOnToolIds(
      ['skill__activate', 'native__read', 'native__run_command'],
      catalog,
    )).toEqual(['native__run_command'])
  })
})
