import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { WorkflowHooksPanel } from './WorkflowHooksPanel'
import { packageApi } from '../api/pluginPackages'

vi.mock('../api/pluginPackages', () => ({ packageApi: { getHooks: vi.fn(), saveHooks: vi.fn() } }))
beforeEach(() => { vi.resetAllMocks(); vi.mocked(packageApi.getHooks).mockResolvedValue({ enabled: false, hooks: {} }) })

describe('workflow hook configuration', () => {
  it('saves the explicit enabled state and hook object', async () => {
    render(<WorkflowHooksPanel lang="zh" />)
    const editor = screen.getByLabelText('工作流 Hook JSON')
    await waitFor(() => expect(editor).toHaveValue(JSON.stringify({ enabled: false, hooks: {} }, null, 2)))
    fireEvent.change(editor, { target: { value: '{"enabled":true,"hooks":{}}' } })
    fireEvent.click(screen.getByRole('button', { name: '保存工作流 Hooks' }))
    await screen.findByText('已保存')
    expect(packageApi.saveHooks).toHaveBeenCalledWith({ enabled: true, hooks: {} })
  })
  it('does not send invalid JSON to the backend', async () => {
    render(<WorkflowHooksPanel lang="zh" />)
    await waitFor(() => expect(screen.getByLabelText('工作流 Hook JSON')).not.toHaveValue(''))
    fireEvent.change(screen.getByLabelText('工作流 Hook JSON'), { target: { value: '{broken' } })
    fireEvent.click(screen.getByRole('button', { name: '保存工作流 Hooks' }))
    await screen.findByRole('alert')
    expect(packageApi.saveHooks).not.toHaveBeenCalled()
  })
})
