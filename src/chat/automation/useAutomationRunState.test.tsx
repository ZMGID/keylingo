import { act, renderHook, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import type { AutomationRun, AutomationRunEvent } from './types'
import { useAutomationRunState } from './useAutomationRunState'

const mocks = vi.hoisted(() => ({ subscribe: vi.fn(), active: vi.fn(), list: vi.fn(), get: vi.fn(), unlisten: vi.fn() }))
vi.mock('../../api/tauri', () => ({ isTauriRuntime: () => true, api: { onAutomationRun: mocks.subscribe } }))
vi.mock('./api', () => ({ automationApi: { activeRun: mocks.active, listRuns: mocks.list, getRun: mocks.get } }))

const snapshot: AutomationRun = {
  id: 'run', automationId: 'auto', origin: 'schedule', status: 'running', startedAt: '2026-09-06T00:00:00Z',
  nodes: [
    { nodeId: 'trigger', nodeType: 'trigger.schedule', status: 'success', output: 'schedule' },
    { nodeId: 'agent', nodeType: 'action.agent', status: 'running' },
  ],
}
let listener: (event: AutomationRunEvent) => void

beforeEach(() => {
  vi.resetAllMocks()
  mocks.subscribe.mockImplementation(async (fn: typeof listener) => { listener = fn; return mocks.unlisten })
  mocks.list.mockResolvedValue([])
  mocks.active.mockResolvedValue(null)
  mocks.get.mockResolvedValue(snapshot)
})

describe('useAutomationRunState', () => {
  it('loads the latest completed run for input/output inspection after reopening', async () => {
    const completed = { ...snapshot, status: 'success', nodes: [{ nodeId: 'agent', nodeType: 'action.agent', status: 'success', result: { text: 'done', json: { ok: true } } }] }
    mocks.list.mockResolvedValue([completed])
    mocks.get.mockResolvedValue(completed)
    const { result } = renderHook(() => useAutomationRunState('auto'))
    await waitFor(() => expect(result.current.runData).toEqual(completed))
    expect(result.current.running).toBe(false)
  })

  it('does not display late data from the previous run after a new run starts', async () => {
    let resolve!: (value: AutomationRun) => void
    mocks.get.mockImplementation(() => new Promise<AutomationRun>((done) => { resolve = done }))
    const { result } = renderHook(() => useAutomationRunState('auto'))
    await waitFor(() => expect(mocks.active).toHaveBeenCalled())
    act(() => listener({ automationId: 'auto', runId: 'old', kind: 'node_finished', nodeId: 'agent', status: 'success' }))
    act(() => listener({ automationId: 'auto', runId: 'new', kind: 'run_started' }))
    await act(async () => { resolve(snapshot) })
    expect(result.current.runData).toBeNull()
    expect(result.current.running).toBe(true)
  })
  it('restores an existing background run and its node progress on mount', async () => {
    mocks.active.mockResolvedValue(snapshot)
    const { result, unmount } = renderHook(() => useAutomationRunState('auto'))
    await waitFor(() => expect(result.current.running).toBe(true))
    expect(result.current.liveStartedAt).toBe(snapshot.startedAt)
    expect(result.current.nodeStatus).toEqual({ trigger: 'success', agent: 'running' })
    expect(result.current.nodeOutput.trigger).toBe('schedule')
    expect(mocks.subscribe.mock.invocationCallOrder[0]).toBeLessThan(mocks.active.mock.invocationCallOrder[0])
    unmount()
    expect(mocks.unlisten).toHaveBeenCalledOnce()
  })

  it('does not resurrect a finished run when an older snapshot arrives late', async () => {
    let resolve!: (value: AutomationRun) => void
    mocks.active.mockImplementationOnce(() => new Promise<AutomationRun>((done) => { resolve = done }))
    const { result } = renderHook(() => useAutomationRunState('auto'))
    await waitFor(() => expect(mocks.active).toHaveBeenCalledOnce())
    act(() => listener({ automationId: 'auto', runId: 'run', kind: 'run_finished', status: 'cancelled' }))
    await act(async () => { resolve(snapshot) })
    await waitFor(() => expect(mocks.active).toHaveBeenCalledTimes(2))
    expect(result.current.running).toBe(false)
    expect(result.current.liveStartedAt).toBeNull()
  })

  it('ignores other automations and clears state when a restored run stops', async () => {
    mocks.active.mockResolvedValue(snapshot)
    const { result } = renderHook(() => useAutomationRunState('auto'))
    await waitFor(() => expect(result.current.running).toBe(true))
    act(() => listener({ automationId: 'other', runId: 'else', kind: 'run_finished', status: 'success' }))
    expect(result.current.running).toBe(true)
    act(() => {
      listener({ automationId: 'auto', runId: 'run', kind: 'node_finished', nodeId: 'agent', status: 'cancelled' })
      listener({ automationId: 'auto', runId: 'run', kind: 'run_finished', status: 'cancelled' })
    })
    expect(result.current.running).toBe(false)
    expect(result.current.nodeStatus.agent).toBe('skipped')
    expect(result.current.runError).toBe('')
  })

  it('does not apply an in-flight snapshot after unmount', async () => {
    let resolve!: (value: AutomationRun) => void
    mocks.active.mockImplementationOnce(() => new Promise<AutomationRun>((done) => { resolve = done }))
    const { unmount } = renderHook(() => useAutomationRunState('auto'))
    await waitFor(() => expect(mocks.active).toHaveBeenCalledOnce())
    unmount()
    await act(async () => { resolve(snapshot) })
    expect(mocks.unlisten).toHaveBeenCalledOnce()
    expect(mocks.active).toHaveBeenCalledOnce()
  })
})
