import { describe, expect, it } from 'vitest'
import { canConnect, connectNodes, createFlowNode, topologicalOrder } from './graph'
import { AUTOMATION_SCHEMA_VERSION, type Automation } from './types'

function node(id: string, type: 'trigger.manual' | 'action.agent' | 'action.notify', x = 0) {
  return createFlowNode(type, { label: id }, { x, y: 0 })
}

describe('automation graph', () => {
  it('只允许接到动作，且每节点单入单出', () => {
    const trigger = { ...node('t', 'trigger.manual'), id: 't' }
    const agent = { ...node('a', 'action.agent', 200), id: 'a' }
    const notify = { ...node('n', 'action.notify', 400), id: 'n' }
    const nodes = [trigger, agent, notify]
    expect(canConnect('t', 'a', nodes, [])).toBe(true)
    expect(canConnect('a', 't', nodes, [])).toBe(false)
    const edges = [connectNodes('t', 'a')]
    expect(canConnect('t', 'n', nodes, edges)).toBe(false)
    expect(canConnect('a', 'n', nodes, edges)).toBe(true)
    expect(canConnect('n', 'a', nodes, [...edges, connectNodes('a', 'n')])).toBe(false)
  })

  it('拓扑序沿边走', () => {
    const automation: Automation = {
      schemaVersion: AUTOMATION_SCHEMA_VERSION,
      id: 'x',
      name: '',
      enabled: false,
      nodes: [
        { ...node('t', 'trigger.manual'), id: 't' },
        { ...node('a', 'action.agent'), id: 'a' },
        { ...node('n', 'action.notify'), id: 'n' },
      ],
      edges: [connectNodes('t', 'a'), connectNodes('a', 'n')],
      viewport: { x: 0, y: 0, zoom: 1 },
      createdAt: '',
      updatedAt: '',
    }
    expect(topologicalOrder(automation)).toEqual(['t', 'a', 'n'])
  })
})
