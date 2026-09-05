import { beforeEach, describe, expect, it, vi } from 'vitest'
import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { PluginPackages } from './PluginPackages'
import { packageApi, type PluginPackage } from '../api/pluginPackages'

vi.mock('../api/pluginPackages', () => ({ packageApi: { list: vi.fn(), import: vi.fn(), setEnabled: vi.fn(), remove: vi.fn() } }))
vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
vi.mock('../api/settingsCache', () => ({ refreshSettings: vi.fn() }))
const plugin: PluginPackage = { id: 'id', name: 'example', version: '1', description: 'test', source: '/plugins/example', revision: null, format: 'claude', enabled: false, components: { hooks: 1 }, diagnostics: [] }
beforeEach(() => { vi.resetAllMocks(); vi.mocked(packageApi.list).mockResolvedValue([]) })

describe('plugin package management', () => {
  it('imports without implicitly enabling executable hooks', async () => {
    vi.mocked(packageApi.import).mockImplementation(async () => { vi.mocked(packageApi.list).mockResolvedValue([plugin]); return plugin })
    render(<PluginPackages lang="zh" />)
    await screen.findByText('尚未导入通用插件。')
    fireEvent.change(screen.getByLabelText('插件来源'), { target: { value: '/plugins/example' } })
    fireEvent.click(screen.getByRole('button', { name: '导入' }))
    await screen.findByText('example')
    expect(packageApi.import).toHaveBeenCalledWith('/plugins/example', undefined)
    expect(packageApi.setEnabled).not.toHaveBeenCalled()
    fireEvent.click(screen.getByRole('button', { name: '启用' }))
    await waitFor(() => expect(packageApi.setEnabled).toHaveBeenCalledWith('id', true))
  })
  it('shows unsupported capabilities and keeps enable unavailable', async () => {
    vi.mocked(packageApi.list).mockResolvedValue([{ ...plugin, diagnostics: ['Unsupported hook event: Stop'] }])
    render(<PluginPackages lang="zh" />)
    await screen.findByText('Unsupported hook event: Stop')
    expect(screen.getByRole('button', { name: '启用' })).toBeDisabled()
    expect(screen.getByRole('button', { name: '移除' })).toBeEnabled()
  })
})
