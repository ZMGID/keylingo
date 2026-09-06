import { describe, expect, it } from 'vitest'
import { createBlankAutomation, createFlowNode, connectNodes } from './graph'
import { dataFields, insertReference, localizeValidationIssue, upstreamNodes, workflowIssues } from './workflowData'

describe('workflow data mapping', () => {
  it('limits a context slot to its owning Agent’s upstream path', () => {
    const trigger = createFlowNode('trigger.manual', { label: 'start' }, { x: 0, y: 0 })
    const agent = createFlowNode('action.agent', { label: 'agent' }, { x: 0, y: 0 })
    const context = createFlowNode('agent.context', { label: 'prompt' }, { x: 0, y: 0 })
    const sibling = createFlowNode('action.set', { label: 'other' }, { x: 0, y: 0 })
    const graph = { ...createBlankAutomation(), nodes: [trigger, agent, context, sibling], edges: [
      connectNodes(trigger.id, agent.id), connectNodes(trigger.id, sibling.id), connectNodes(context.id, agent.id, 'slot', 'context'),
    ] }
    expect(upstreamNodes(graph, context.id).map((node) => node.id)).toEqual([trigger.id])
  })

  it('uses JSON pointers for arrays, punctuation and Unicode field names', () => {
    const fields = dataFields({ '商品/规格~': [{ name: '蓝色' }] })
    expect(fields).toContainEqual({ pointer: '/商品~1规格~0/0/name', value: '蓝色' })
  })

  it('inserts a reference into a selected list field without changing other fields', () => {
    const node = createFlowNode('action.set', { label: 'set', set: { fields: [{ key: 'a', value: 'prefix ' }, { key: 'b', value: 'keep' }] } }, { x: 0, y: 0 })
    const result = insertReference(node, 'set.fields.0.value', '{{nodes.a#/json/title}}')
    expect(result.data.set?.fields).toEqual([{ key: 'a', value: 'prefix {{nodes.a#/json/title}}' }, { key: 'b', value: 'keep' }])
    expect(node.data.set?.fields[0].value).toBe('prefix ')
  })

  it('identifies missing configuration and invalid references, but skips disabled nodes', () => {
    const node = createFlowNode('action.http', { label: 'http', http: { url: '', method: 'GET', headers: '{{nodes.missing#/text}}', body: '' } }, { x: 0, y: 0 })
    const graph = { ...createBlankAutomation(), nodes: [node] }
    expect(workflowIssues(graph).filter((issue) => issue.nodeId === node.id && issue.severity === 'error')).toHaveLength(2)
    node.data.disabled = true
    expect(workflowIssues(graph).filter((issue) => issue.nodeId === node.id)).toHaveLength(0)
  })

  it('reports an Agent without a prompt or Context locally in the active language', () => {
    const trigger = createFlowNode('trigger.manual', { label: 'start' }, { x: 0, y: 0 })
    const agent = createFlowNode('action.agent', { label: 'agent' }, { x: 0, y: 0 })
    const graph = {
      ...createBlankAutomation(),
      nodes: [trigger, agent],
      edges: [connectNodes(trigger.id, agent.id)],
    }
    expect(workflowIssues(graph)).toContainEqual({
      nodeId: agent.id,
      severity: 'warning',
      message: '填写 Agent 提示词，或连接一个 Context 节点；否则运行时会失败',
    })
    expect(workflowIssues(graph, true).find((issue) => issue.nodeId === agent.id)?.message)
      .toContain('Add an Agent prompt')
  })

  it('localizes backend validation messages before rendering them', () => {
    expect(localizeValidationIssue({
      nodeId: 'agent',
      severity: 'warning',
      message: 'action.agent has no prompt and no agent.context slot; the step will fail at run time',
    }).message).toBe('填写 Agent 提示词，或连接一个 Context 节点；否则运行时会失败')
  })
})
