import { act, render, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { useState } from 'react'
import type { ReactFlowProps } from '@xyflow/react'
import type { AutomationRfNode } from './nodes/FlowNode'
import type { Automation } from './types'
import { AutomationEditor } from './AutomationEditor'
import { createBlankAutomation, ensureNodeSpacing } from './graph'

const capture = vi.hoisted(() => ({ flow: null as ReactFlowProps<AutomationRfNode> | null, saved: vi.fn() }))
vi.mock('@xyflow/react', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@xyflow/react')>()
  return {
    ...actual,
    ReactFlow: (props: ReactFlowProps<AutomationRfNode>) => { capture.flow = props; return <div /> },
    ReactFlowProvider: ({ children }: { children: React.ReactNode }) => children,
    useReactFlow: () => ({ screenToFlowPosition: (point: { x: number, y: number }) => point }),
  }
})
vi.mock('../../api/tauri', () => ({ api: {}, isTauriRuntime: () => false }))
vi.mock('./NodeInspector', () => ({ NodeInspector: () => null }))

function Host({ initial }: { initial: Automation }) {
  const [automation, setAutomation] = useState(initial)
  return <AutomationEditor automation={automation} onBack={() => {}} onFlushSave={async () => {}}
    onChange={(next) => { capture.saved(next); setAutomation(next) }} />
}

function fixture(): Automation {
  return {
    ...createBlankAutomation(),
    nodes: [
      { id: 'trigger', type: 'trigger.manual', data: { label: 'manual' }, position: { x: 0, y: 0 } },
      { id: 'next', type: 'action.notify', data: { label: 'notify' }, position: { x: 0, y: 0 } },
    ],
    edges: [{ id: 'edge', source: 'trigger', target: 'next' }],
  }
}

beforeEach(() => { capture.saved.mockClear(); capture.flow = null })

describe('AutomationEditor spacing persistence', () => {
  it('persists corrected imported positions and preserves them when reopened', async () => {
    const view = render(<Host initial={fixture()} />)
    await waitFor(() => expect(capture.saved).toHaveBeenCalled())
    const saved = capture.saved.mock.lastCall![0] as Automation
    expect(ensureNodeSpacing(saved.nodes)).toBe(saved.nodes)
    expect(saved.nodes[1].position).not.toEqual({ x: 0, y: 0 })
    view.unmount()
    capture.saved.mockClear()
    render(<Host initial={saved} />)
    expect(capture.flow!.nodes!.map((node) => node.position)).toEqual(saved.nodes.map((node) => node.position))
    expect(capture.saved).not.toHaveBeenCalled()
  })

  it('saves the collision-adjusted position when drag change and drag stop are batched', async () => {
    render(<Host initial={fixture()} />)
    await waitFor(() => expect(capture.saved).toHaveBeenCalled())
    const fixed = capture.flow!.nodes![0].position
    capture.saved.mockClear()
    act(() => {
      capture.flow!.onNodesChange!([{ id: 'next', type: 'position', position: fixed, dragging: true }])
      capture.flow!.onNodeDragStop!(new MouseEvent('mouseup'), capture.flow!.nodes![1], [])
    })
    const saved = capture.saved.mock.lastCall![0] as Automation
    expect(saved.nodes[0].position).toEqual(fixed)
    expect(saved.nodes[1].position).not.toEqual(fixed)
    expect(ensureNodeSpacing(saved.nodes)).toBe(saved.nodes)
    expect(capture.flow!.nodes![1].position).toEqual(saved.nodes[1].position)
  })
})
