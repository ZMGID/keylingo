import { render, screen, waitFor } from '@testing-library/react'
import { describe, expect, it, vi, beforeEach } from 'vitest'
import { ExternalAgentsSettings } from './ExternalAgentsSettings'
import { chatApi } from '../chat/api'

vi.mock('../chat/api', () => ({
  chatApi: {
    detectExternalAgents: vi.fn(),
    detectExternalAgentModels: vi.fn().mockResolvedValue({ models: [], reasoningOptions: [] }),
  },
}))

const mockDetect = vi.mocked(chatApi.detectExternalAgents)

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

  it('renders installed and not-installed tags after scan', async () => {
    render(
      <ExternalAgentsSettings
        lang="zh"
      />,
    )

    await waitFor(() => {
      expect(screen.getByText('Claude Code')).toBeInTheDocument()
    })

    expect(screen.getByText('已安装')).toBeInTheDocument()
    expect(screen.getByText('未安装')).toBeInTheDocument()
    expect(mockDetect).toHaveBeenCalled()
  })
})
