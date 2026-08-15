import { fireEvent, render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi, beforeEach } from 'vitest'
import { ExternalAgentsSettings } from './ExternalAgentsSettings'
import { chatApi } from '../chat/api'
import type { Settings as SettingsData } from '../api/tauri'

vi.mock('../chat/api', () => ({
  chatApi: {
    detectExternalAgents: vi.fn(),
    detectExternalAgentModels: vi.fn().mockResolvedValue({ models: [], reasoningOptions: [] }),
    externalCliInstallInfo: vi.fn().mockResolvedValue({
      agentId: 'claude',
      localVersion: '1.0.0',
      latestVersion: '1.1.0',
      updateAvailable: true,
      command: 'npm install -g @anthropic-ai/claude-code@latest',
      docsUrl: 'https://docs.claude.com',
      configDir: '/home/u/.claude',
    }),
    externalCliInstall: vi.fn(),
    externalCliOpenConfigDir: vi.fn(),
    externalCliProviderCleanup: vi.fn(),
    externalCliScanCcSwitch: vi.fn().mockResolvedValue({ providers: [], skipped: 0 }),
    dshPluginSettingsGet: vi.fn().mockResolvedValue(null),
    dshPluginSettingsSave: vi.fn(),
    dshPluginInventory: vi.fn().mockResolvedValue([]),
    dshOpenSettingsFile: vi.fn(),
    dshOfficialCredentialStatus: vi.fn().mockResolvedValue({ configured: false, writable: true }),
    dshOfficialCredentialSave: vi.fn(),
  },
  onExternalCliInstallLog: vi.fn().mockResolvedValue(() => {}),
  onExternalAgentsUpdated: vi.fn().mockResolvedValue(() => {}),
}))

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))

const mockDetect = vi.mocked(chatApi.detectExternalAgents)
const mockInstallInfo = vi.mocked(chatApi.externalCliInstallInfo)
const mockInstall = vi.mocked(chatApi.externalCliInstall)
const mockOfficialKeyStatus = vi.mocked(chatApi.dshOfficialCredentialStatus)
const mockOfficialKeySave = vi.mocked(chatApi.dshOfficialCredentialSave)

function renderPanel(
  chat: Partial<NonNullable<SettingsData['chat']>> = {},
  updateChat = vi.fn(),
) {
  return {
    updateChat,
    ...render(
      <ExternalAgentsSettings
        lang="zh"
        settings={{ chat } as SettingsData}
        updateChat={updateChat}
      />,
    ),
  }
}

describe('ExternalAgentsSettings', () => {
  beforeEach(() => {
    mockDetect.mockReset()
    mockInstallInfo.mockReset()
    mockInstall.mockReset()
    mockOfficialKeyStatus.mockReset()
    mockOfficialKeySave.mockReset()
    mockOfficialKeyStatus.mockResolvedValue({ configured: false, writable: true })
    mockOfficialKeySave.mockResolvedValue({ configured: true, writable: true })
    mockDetect.mockResolvedValue([
      {
        id: 'claude',
        name: 'Claude Code',
        available: true,
        path: '/usr/local/bin/claude',
        version: '1.0.0',
        models: [{ id: 'default', label: 'Default' }],
        authStatus: 'ok',
      },
      {
        id: 'codex',
        name: 'Codex',
        available: false,
        models: [],
      },
    ])
    mockInstallInfo.mockResolvedValue({
      agentId: 'claude',
      localVersion: '1.0.0',
      latestVersion: '1.1.0',
      updateAvailable: true,
      command: 'npm install -g @anthropic-ai/claude-code@latest',
      docsUrl: 'https://docs.claude.com',
      configDir: '/home/u/.claude',
    })
    mockInstall.mockResolvedValue()
  })

  it('groups agents by install state and selects the first available one', async () => {
    renderPanel()

    await waitFor(() => {
      expect(screen.getAllByText('Claude Code').length).toBeGreaterThan(0)
    })

    expect(screen.getByText('已安装')).toBeInTheDocument()
    expect(screen.getByText('未安装')).toBeInTheDocument()
    // 首个可用的进详情面板：自定义路径这一行只在选中项上渲染。
    expect(screen.getByText('自定义路径')).toBeInTheDocument()
    expect(screen.queryByText('环境变量')).not.toBeInTheDocument()
    expect(screen.queryByText('配置目录')).not.toBeInTheDocument()
    expect(screen.queryByText('已检测到')).not.toBeInTheDocument()
    expect(mockDetect).toHaveBeenCalled()
  })

  it('shows an explicit update status and update action only when a newer version exists', async () => {
    renderPanel()

    await waitFor(() => {
      expect(screen.getByText('可更新到 1.1.0')).toBeInTheDocument()
    })
    expect(screen.getByRole('button', { name: '更新' })).toBeInTheDocument()
  })

  it('rescans all agents after an update command finishes', async () => {
    renderPanel()

    const update = await screen.findByRole('button', { name: '更新' })
    fireEvent.click(update)

    await waitFor(() => {
      expect(mockInstall).toHaveBeenCalledWith('claude')
      expect(mockDetect).toHaveBeenCalledWith(true)
    })
  })

  it('shows up-to-date status without an update action', async () => {
    mockInstallInfo.mockResolvedValue({
      agentId: 'claude',
      localVersion: '1.0.0',
      latestVersion: '1.0.0',
      updateAvailable: false,
      command: 'npm install -g @anthropic-ai/claude-code@latest',
      docsUrl: 'https://docs.claude.com',
      configDir: '/home/u/.claude',
    })

    renderPanel()

    await waitFor(() => {
      expect(screen.getByText('已是最新')).toBeInTheDocument()
    })
    expect(screen.queryByRole('button', { name: '更新' })).not.toBeInTheDocument()
  })

  it('does not offer an update when the latest version cannot be checked', async () => {
    mockInstallInfo.mockResolvedValue({
      agentId: 'claude',
      localVersion: '1.0.0',
      latestVersion: null,
      updateAvailable: false,
      command: 'npm install -g @anthropic-ai/claude-code@latest',
      docsUrl: 'https://docs.claude.com',
      configDir: '/home/u/.claude',
    })

    renderPanel()

    await waitFor(() => {
      expect(screen.getByText('无法确认最新版本')).toBeInTheDocument()
    })
    expect(screen.queryByRole('button', { name: '更新' })).not.toBeInTheDocument()
  })

  it('explains when an update exists but the install source cannot be updated safely', async () => {
    mockInstallInfo.mockResolvedValue({
      agentId: 'gemini',
      localVersion: '1.0.0',
      latestVersion: '1.1.0',
      updateAvailable: true,
      command: null,
      docsUrl: 'https://www.geminicli.com/docs/get-started/installation',
      configDir: '/home/u/.gemini',
    })

    renderPanel()

    await waitFor(() => {
      expect(screen.getByText('可更新到 1.1.0，请按官方文档手动更新')).toBeInTheDocument()
    })
    expect(screen.queryByRole('button', { name: '更新' })).not.toBeInTheDocument()
  })

  it('moves a disabled agent into its own group', async () => {
    renderPanel({ externalCliAgents: { claude: { disabled: true } } })

    await waitFor(() => {
      expect(screen.getByText('已停用')).toBeInTheDocument()
    })
    // 唯一的已安装项被停用了，左栏就不该再有「已安装」分组。
    expect(screen.queryByText('已安装')).not.toBeInTheDocument()
  })

  it('activating a provider writes currentProvider', async () => {
    const { updateChat } = renderPanel({
      externalCliAgents: {
        claude: {
          providers: [
            { id: 'relay-1', name: 'Loki', env: [{ key: 'ANTHROPIC_BASE_URL', value: 'https://relay' }] },
          ],
        },
      },
    })

    await waitFor(() => {
      expect(screen.getByText('Loki')).toBeInTheDocument()
    })
    // 卡片里那行「启用」是纯 label（旁边是 Toggle），只有供应商行的才是按钮。
    fireEvent.click(screen.getByRole('button', { name: '启用' }))
    expect(updateChat).toHaveBeenCalledWith(
      expect.objectContaining({
        externalCliAgents: expect.objectContaining({
          claude: expect.objectContaining({ currentProvider: 'relay-1' }),
        }),
      }),
    )
  })

  it('shows the empty state when no provider is configured', async () => {
    renderPanel()
    await waitFor(() => {
      expect(screen.getByText('所有供应商')).toBeInTheDocument()
    })
    expect(screen.getByText('暂无供应商，点击上方「添加」创建一个。')).toBeInTheDocument()
  })

  it('lists official DeepSeek as the in-use dsh provider', async () => {
    mockDetect.mockResolvedValue([
      {
        id: 'dsh',
        name: 'DeepSeek Harness',
        available: true,
        path: 'C:\\npm\\dsh.cmd',
        version: '0.1.0-rc.6',
        models: [],
        authStatus: 'ok',
      },
    ])
    mockInstallInfo.mockResolvedValue({
      agentId: 'dsh',
      localVersion: '0.1.0-rc.6',
      latestVersion: '0.1.0-rc.6',
      updateAvailable: false,
      command: 'npm install -g @deepseek-ai/dsh@latest',
      docsUrl: 'https://github.com/deepseek-ai/dsh',
      configDir: 'C:\\Users\\u\\.dsh',
    })

    renderPanel()
    await waitFor(() => {
      expect(screen.getByText('DeepSeek')).toBeInTheDocument()
    })
    expect(screen.getByText('官方提供方 · 2 个模型')).toBeInTheDocument()
    expect(screen.getByText('使用中')).toBeInTheDocument()
    expect(screen.queryByText('使用 CLI 自身配置')).not.toBeInTheDocument()
    expect(screen.queryByText('暂无供应商，点击上方「添加」创建一个。')).not.toBeInTheDocument()
  })

  it('opens dsh plugins on a secondary page', async () => {
    mockDetect.mockResolvedValue([
      {
        id: 'dsh',
        name: 'DeepSeek Harness',
        available: true,
        path: 'C:\\npm\\dsh.cmd',
        version: '0.1.0-rc.6',
        models: [],
        authStatus: 'ok',
      },
    ])
    mockInstallInfo.mockResolvedValue({
      agentId: 'dsh',
      localVersion: '0.1.0-rc.6',
      latestVersion: '0.1.0-rc.6',
      updateAvailable: false,
      command: 'npm install -g @deepseek-ai/dsh@latest',
      docsUrl: 'https://github.com/deepseek-ai/dsh',
      configDir: 'C:\\Users\\u\\.dsh',
    })

    renderPanel()
    await waitFor(() => {
      expect(screen.getByText('所有供应商')).toBeInTheDocument()
    })
    expect(screen.queryByRole('tab', { name: '插件配置' })).not.toBeInTheDocument()
    const plugins = screen.getByText('插件')
    const providers = screen.getByText('所有供应商')
    expect(plugins.compareDocumentPosition(providers) & Node.DOCUMENT_POSITION_FOLLOWING).toBeTruthy()

    fireEvent.click(plugins.closest('button')!)
    expect(screen.getByRole('tab', { name: '插件配置' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: /返回/ })).toBeInTheDocument()
    expect(screen.queryByText('所有供应商')).not.toBeInTheDocument()

    fireEvent.click(screen.getByRole('button', { name: /返回/ }))
    expect(screen.queryByRole('tab', { name: '插件配置' })).not.toBeInTheDocument()
    expect(screen.getByText('所有供应商')).toBeInTheDocument()
  })

  it('saves the official DeepSeek API key on the dsh page', async () => {
    mockDetect.mockResolvedValue([
      {
        id: 'dsh',
        name: 'DeepSeek Harness',
        available: true,
        path: 'C:\\npm\\dsh.cmd',
        version: '0.1.0-rc.6',
        models: [],
        authStatus: 'ok',
      },
    ])
    mockInstallInfo.mockResolvedValue({
      agentId: 'dsh',
      localVersion: '0.1.0-rc.6',
      latestVersion: '0.1.0-rc.6',
      updateAvailable: false,
      command: 'npm install -g @deepseek-ai/dsh@latest',
      docsUrl: 'https://github.com/deepseek-ai/dsh',
      configDir: 'C:\\Users\\u\\.dsh',
    })

    renderPanel()
    const input = await screen.findByLabelText('DeepSeek API 密钥')
    expect(screen.getByText('未配置')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: '获取密钥' })).toHaveAttribute(
      'href',
      'https://platform.deepseek.com/api_keys',
    )
    fireEvent.change(input, { target: { value: 'sk-test' } })
    fireEvent.click(screen.getByRole('button', { name: '保存密钥' }))
    await waitFor(() => {
      expect(mockOfficialKeySave).toHaveBeenCalledWith('sk-test')
    })
    expect(await screen.findByText('已配置')).toBeInTheDocument()
  })
})
