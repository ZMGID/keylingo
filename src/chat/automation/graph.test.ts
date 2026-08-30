import { describe, expect, it } from 'vitest'
import {
  FLOW_NODE_GAP_Y,
  FLOW_ORIGIN,
  canConnect,
  connectNodes,
  createFlowNode,
  flowEdgeFromConnection,
  layoutFlow,
  pickAppendSource,
  topologicalOrder,
} from './graph'
import { AUTOMATION_SCHEMA_VERSION, type Automation } from './types'

function node(id: string, type: 'trigger.manual' | 'action.agent' | 'action.notify' | 'logic.if', x = 0) {
  return createFlowNode(type, { label: id }, { x, y: 0 })
}

describe('automation graph', () => {
  it('允许一连多，目标仍只能有一个入口', () => {
    const trigger = { ...node('t', 'trigger.manual'), id: 't' }
    const agent = { ...node('a', 'action.agent', 200), id: 'a' }
    const notify = { ...node('n', 'action.notify', 400), id: 'n' }
    const nodes = [trigger, agent, notify]
    expect(canConnect('t', 'a', nodes, [])).toBe(true)
    expect(canConnect('a', 't', nodes, [])).toBe(false)
    const edges = [connectNodes('t', 'a')]
    expect(canConnect('t', 'n', nodes, edges)).toBe(true)
    expect(canConnect('a', 'n', nodes, edges)).toBe(true)
    expect(canConnect('n', 'a', nodes, [...edges, connectNodes('a', 'n')])).toBe(false)
    expect(canConnect('t', 'a', nodes, edges)).toBe(false)
  })

  it('Switch 每个出口只能连一条边', () => {
    const trigger = { ...node('t', 'trigger.manual'), id: 't' }
    const sw = createFlowNode('logic.switch', {
      label: 's',
      switch: { cases: [{ id: '1', op: 'equals', value: 'a' }] },
    }, { x: 200, y: 0 })
    sw.id = 's'
    const a = { ...node('a', 'action.notify', 400), id: 'a' }
    const b = { ...node('b', 'action.notify', 400), id: 'b' }
    const nodes = [trigger, sw, a, b]
    const after = [connectNodes('t', 's')]
    expect(canConnect('s', 'a', nodes, after, '1')).toBe(true)
    const withOne = [...after, connectNodes('s', 'a', '1')]
    expect(canConnect('s', 'a', nodes, withOne, '1')).toBe(false)
    expect(canConnect('s', 'b', nodes, withOne, 'default')).toBe(true)
  })

  it('If 节点允许 true/false 两个出口', () => {
    const trigger = { ...node('t', 'trigger.manual'), id: 't' }
    const iff = { ...node('i', 'logic.if', 200), id: 'i' }
    const yes = { ...node('y', 'action.notify', 400), id: 'y' }
    const no = { ...node('n', 'action.notify', 400), id: 'n' }
    const nodes = [trigger, iff, yes, no]
    expect(canConnect('t', 'i', nodes, [])).toBe(true)
    const afterTrigger = [connectNodes('t', 'i')]
    expect(canConnect('i', 'y', nodes, afterTrigger, 'true')).toBe(true)
    const withTrue = [...afterTrigger, connectNodes('i', 'y', 'true')]
    expect(canConnect('i', 'y', nodes, withTrue, 'true')).toBe(false)
    expect(canConnect('i', 'n', nodes, withTrue, 'false')).toBe(true)
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

  it('线性图画成同一行', () => {
    const nodes = [
      { ...node('t', 'trigger.manual'), id: 't', position: { x: 0, y: 400 } },
      { ...node('a', 'action.agent'), id: 'a', position: { x: 10, y: 0 } },
      { ...node('n', 'action.notify'), id: 'n', position: { x: 99, y: 900 } },
    ]
    const laid = layoutFlow(nodes, [connectNodes('t', 'a'), connectNodes('a', 'n')])
    expect(laid.map((item) => item.position.y)).toEqual([
      FLOW_ORIGIN.y,
      FLOW_ORIGIN.y,
      FLOW_ORIGIN.y,
    ])
    expect(laid[0].position.x).toBe(FLOW_ORIGIN.x)
    expect(laid[1].position.x).toBeGreaterThan(laid[0].position.x)
    expect(laid[2].position.x).toBeGreaterThan(laid[1].position.x)
  })

  it('If 的 false 口向下分叉', () => {
    const nodes = [
      { ...node('t', 'trigger.manual'), id: 't' },
      { ...node('i', 'logic.if'), id: 'i' },
      { ...node('y', 'action.notify'), id: 'y' },
      { ...node('n', 'action.notify'), id: 'n' },
    ]
    const laid = layoutFlow(nodes, [
      connectNodes('t', 'i'),
      connectNodes('i', 'y', 'true'),
      connectNodes('i', 'n', 'false'),
    ])
    const yes = laid.find((item) => item.id === 'y')!
    const no = laid.find((item) => item.id === 'n')!
    const iff = laid.find((item) => item.id === 'i')!
    expect(yes.position.y).toBe(iff.position.y - FLOW_NODE_GAP_Y)
    expect(no.position.y).toBe(iff.position.y + FLOW_NODE_GAP_Y)
    expect(yes.position.x).toBe(no.position.x)
  })

  it('多个触发器可以接到同一步', () => {
    const manual = { ...node('m', 'trigger.manual'), id: 'm' }
    const schedule = createFlowNode('trigger.schedule', { label: 's' }, { x: 0, y: FLOW_NODE_GAP_Y })
    schedule.id = 's'
    const agent = { ...node('a', 'action.agent', 200), id: 'a' }
    const notify = { ...node('n', 'action.notify', 400), id: 'n' }
    const nodes = [manual, schedule, agent, notify]
    expect(canConnect('m', 'a', nodes, [])).toBe(true)
    expect(canConnect('s', 'a', nodes, [connectNodes('m', 'a')])).toBe(true)
    expect(canConnect('s', 'n', nodes, [connectNodes('a', 'n')])).toBe(false)
  })

  it('添加下一步接到还能出边的节点', () => {
    const trigger = { ...node('t', 'trigger.manual'), id: 't' }
    const agent = { ...node('a', 'action.agent'), id: 'a' }
    const nodes = [trigger, agent]
    expect(pickAppendSource(nodes, [])).toEqual({ nodeId: 'a' })
    expect(pickAppendSource(nodes, [connectNodes('t', 'a')])).toEqual({ nodeId: 'a' })
    expect(pickAppendSource(nodes, [connectNodes('t', 'a')], 't')).toEqual({ nodeId: 'a' })
    const notify = { ...node('n', 'action.notify'), id: 'n' }
    expect(pickAppendSource(
      [...nodes, notify],
      [connectNodes('t', 'a'), connectNodes('a', 'n')],
    )).toEqual({ nodeId: 'n' })
  })

  it('slot nodes plug into Agent without taking the main-flow input', () => {
    const trigger = { ...node('t', 'trigger.manual'), id: 't' }
    const agent = { ...node('a', 'action.agent', 200), id: 'a' }
    const runtime = createFlowNode('agent.runtime', { label: 'r' }, { x: 200, y: 200 })
    runtime.id = 'r'
    const nodes = [trigger, agent, runtime]
    expect(canConnect('t', 'a', nodes, [])).toBe(true)
    expect(canConnect('r', 'a', nodes, [connectNodes('t', 'a')], 'slot', 'runtime')).toBe(true)
    expect(canConnect('r', 'a', nodes, [connectNodes('t', 'a')], 'slot')).toBe(true)
    expect(canConnect('r', 'a', nodes, [connectNodes('t', 'a')], 'slot', 'context')).toBe(false)
    const plugged = [connectNodes('t', 'a'), connectNodes('r', 'a', 'slot', 'runtime')]
    expect(canConnect('r', 'a', nodes, plugged, 'slot', 'runtime')).toBe(false)
    expect(pickAppendSource(nodes, plugged)).toEqual({ nodeId: 'a' })
    const remapped = flowEdgeFromConnection('agent.runtime', 'r', 'a', 'slot', null)
    expect(remapped.targetHandle).toBe('runtime')
    expect(remapped.sourceHandle).toBe('slot')
    expect(canConnect('t', 'a', nodes, [remapped])).toBe(true)
  })
})
