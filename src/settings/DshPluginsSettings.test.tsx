import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { chatApi, type DshPluginSettingsSnapshot } from '../chat/api'
import { DshPluginsSettings } from './DshPluginsSettings'
import { dshPluginShortName } from './dshPluginNames'

vi.mock('../chat/api', () => ({
  chatApi: {
    dshPluginSettingsGet: vi.fn(),
    dshPluginSettingsSave: vi.fn(),
    dshPluginInventory: vi.fn(),
    dshOpenSettingsFile: vi.fn(),
  },
}))

const snapshot: DshPluginSettingsSnapshot = {
  settingsPath: '/tmp/.dsh/settings.yaml',
  shell: {
    timeoutMs: 90000,
    maxOutputBytes: null,
    timeoutMsDefault: 120000,
    maxOutputBytesDefault: 64000,
  },
  agentLoop: {
    maxParallelToolCalls: null,
    maxParallelToolCallsDefault: 10,
  },
  webSearch: {
    baseUrl: null,
    maxUses: null,
    apiKeyEnv: 'DEEPSEEK_API_KEY',
    apiKeyConfigured: false,
    apiKeyWritable: true,
    baseUrlDefault: 'https://api.deepseek.com/anthropic/v1',
    maxUsesDefault: 5,
  },
}

describe('dshPluginShortName', () => {
  it('strips official package prefixes the same way dsh web does', () => {
    expect(dshPluginShortName('@deepseek-ai/cordis-plugin-include')).toBe('include')
    expect(dshPluginShortName('@deepseek-ai/dsh-timer')).toBe('timer')
    expect(dshPluginShortName('@deepseek-ai/dsh-host-plugin-inventory')).toBe('plugin-inventory')
  })
})

describe('DshPluginsSettings', () => {
  beforeEach(() => {
    vi.mocked(chatApi.dshPluginSettingsGet).mockReset()
    vi.mocked(chatApi.dshPluginSettingsSave).mockReset()
    vi.mocked(chatApi.dshPluginInventory).mockReset()
    vi.mocked(chatApi.dshOpenSettingsFile).mockReset()
    vi.mocked(chatApi.dshPluginSettingsGet).mockResolvedValue(snapshot)
    vi.mocked(chatApi.dshPluginInventory).mockResolvedValue([
      { id: 'timer', moduleName: '@deepseek-ai/dsh-timer', enabled: true },
      { id: 'tool-bash', moduleName: '@deepseek-ai/dsh-tool-bash', enabled: false },
    ])
  })

  it('renders official config cards and saves a shell override', async () => {
    vi.mocked(chatApi.dshPluginSettingsSave).mockResolvedValue({
      ...snapshot,
      shell: { ...snapshot.shell, timeoutMs: 45000 },
    })
    render(<DshPluginsSettings lang="zh" />)
    await waitFor(() => expect(screen.getByText('终端')).toBeInTheDocument())
    expect(screen.getByText('Agent 循环')).toBeInTheDocument()
    expect(screen.getByText('网页搜索')).toBeInTheDocument()
    expect(screen.getByText('已覆盖')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '恢复默认' })).toBeInTheDocument()

    const timeout = screen.getByLabelText('命令超时（毫秒）')
    fireEvent.change(timeout, { target: { value: '45000' } })
    fireEvent.click(screen.getByRole('button', { name: '保存' }))
    await waitFor(() =>
      expect(chatApi.dshPluginSettingsSave).toHaveBeenCalledWith({
        shell: { timeoutMs: 45000, maxOutputBytes: null },
      }),
    )
  })

  it('lists composed plugins and filters by search', async () => {
    render(<DshPluginsSettings lang="zh" />)
    fireEvent.click(screen.getByRole('tab', { name: '插件列表' }))
    await waitFor(() => expect(screen.getByText('timer')).toBeInTheDocument())
    expect(screen.getByText('已启用')).toBeInTheDocument()
    expect(screen.getByText('已停用')).toBeInTheDocument()
    expect(screen.getByText('2')).toBeInTheDocument()

    fireEvent.change(screen.getByLabelText('搜索插件'), { target: { value: 'bash' } })
    expect(screen.queryByText('timer')).not.toBeInTheDocument()
    expect(screen.getByText('tool-bash')).toBeInTheDocument()
    expect(screen.getByText('1')).toBeInTheDocument()
  })

  it('surfaces the inventory error detail instead of swallowing it', async () => {
    vi.mocked(chatApi.dshPluginInventory).mockRejectedValue(
      'dsh --dump-config 失败：profile missing',
    )
    render(<DshPluginsSettings lang="zh" />)
    fireEvent.click(screen.getByRole('tab', { name: '插件列表' }))
    await waitFor(() => expect(screen.getByText('暂时无法读取插件。')).toBeInTheDocument())
    expect(screen.getByText('dsh --dump-config 失败：profile missing')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: '重试' })).toBeInTheDocument()
  })
})
