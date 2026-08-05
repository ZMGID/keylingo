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
  },
  onExternalCliInstallLog: vi.fn().mockResolvedValue(() => {}),
  onExternalAgentsUpdated: vi.fn().mockResolvedValue(() => {}),
}))

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))

const mockDetect = vi.mocked(chatApi.detectExternalAgents)

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
    expect(mockDetect).toHaveBeenCalled()
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
    expect(screen.getByText('暂无第三方配置，点击上方「添加」创建一个。')).toBeInTheDocument()
  })
})
