import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { WorkflowWorkbench } from './WorkflowWorkbench'
import { createBlankAutomation, createFlowNode, connectNodes } from './graph'
import type { AutomationRun } from './types'

const source = createFlowNode('trigger.manual', { label: '上游' }, { x: 0, y: 0 })
const node = createFlowNode('action.notify', { label: '通知', notify: { body: '' } }, { x: 0, y: 0 })
const graph = { ...createBlankAutomation(), nodes: [source, node], edges: [connectNodes(source.id, node.id)] }
const input = { text: 'sample', json: { product: '蓝色' }, sources: { [source.id]: { text: 'sample', json: { product: '蓝色' } } } }
const run: AutomationRun = { id: 'run', automationId: graph.id, origin: 'manual', status: 'success', startedAt: '2026-09-06', nodes: [
  { nodeId: source.id, nodeType: source.type, status: 'success', result: input },
  { nodeId: node.id, nodeType: node.type, status: 'success', input },
] }
function setup(record: AutomationRun | null = run) {
  const onTest = vi.fn().mockResolvedValue(undefined)
  const onChange = vi.fn()
  render(<WorkflowWorkbench graph={graph} node={node} run={record} running={false} issues={[]} onTest={onTest} onChange={onChange}><div>参数表单</div></WorkflowWorkbench>)
  return { onTest, onChange }
}
describe('node workbench', () => {
  it('edits the connected Context prompt when inserting from an Agent node', () => {
    const agent = createFlowNode('action.agent', { label: 'Agent' }, { x: 0, y: 0 })
    const context = createFlowNode('agent.context', { label: 'Context', agent: { prompt: '处理：' } }, { x: 0, y: 0 })
    const connected = { ...graph, nodes: [source, agent, context], edges: [connectNodes(source.id, agent.id), connectNodes(context.id, agent.id, 'slot', 'context')] }
    const onChange = vi.fn()
    render(<WorkflowWorkbench graph={connected} node={agent} run={run} running={false} issues={[]} onTest={vi.fn()} onChange={onChange}><div /></WorkflowWorkbench>)
    fireEvent.click(screen.getByText('上游'))
    fireEvent.click(screen.getByText('/json/product'))
    expect(onChange.mock.calls[0][0].id).toBe(context.id)
    expect(onChange.mock.calls[0][0].data.agent.prompt).toBe(`处理：{{nodes.${source.id}#/json/product}}`)
  })
  it('inserts a selected structured field into the parameter', () => {
    const { onChange } = setup()
    fireEvent.click(screen.getByText('上游'))
    fireEvent.click(screen.getByText('/json/product'))
    expect(onChange.mock.calls[0][0].data.notify.body).toBe(`{{nodes.${source.id}#/json/product}}`)
  })
  it('passes the exact cached input to the single-node test', async () => {
    const { onTest } = setup()
    fireEvent.click(screen.getByRole('tab', { name: '输入 / 测试' }))
    fireEvent.click(screen.getByRole('button', { name: '测试当前节点' }))
    await waitFor(() => expect(onTest).toHaveBeenCalledWith(input))
    expect(screen.getByRole('tab', { name: '输出' })).toHaveAttribute('aria-selected', 'true')
  })
  it('blocks malformed JSON and lets the user supply custom input without a prior run', async () => {
    const { onTest } = setup(null)
    fireEvent.click(screen.getByRole('tab', { name: '输入 / 测试' }))
    expect(screen.getByRole('button', { name: '测试当前节点' })).toBeDisabled()
    fireEvent.change(screen.getByLabelText('测试输入'), { target: { value: 'custom' } })
    fireEvent.change(screen.getByLabelText('JSON'), { target: { value: '{' } })
    fireEvent.click(screen.getByRole('button', { name: '测试当前节点' }))
    expect(await screen.findByRole('alert')).toBeInTheDocument()
    expect(onTest).not.toHaveBeenCalled()
    fireEvent.change(screen.getByLabelText('JSON'), { target: { value: '{"price":12}' } })
    fireEvent.change(screen.getByLabelText('文本输入'), { target: { value: '商品' } })
    fireEvent.click(screen.getByRole('button', { name: '测试当前节点' }))
    await waitFor(() => expect(onTest).toHaveBeenCalledWith({ text: '商品', json: { price: 12 } }))
  })
})
