import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { open } from '@tauri-apps/plugin-dialog'
import { chatApi, type PiSkillInventory } from '../chat/api'
import { PiSkillsSettings } from './PiSkillsSettings'

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
vi.mock('../chat/api', () => ({
  chatApi: {
    piSkillsInventory: vi.fn(),
    piSkillSetEnabled: vi.fn(),
    piSkillCommandsSetEnabled: vi.fn(),
    piSkillAddPath: vi.fn(),
    piSkillRemovePath: vi.fn(),
    piSkillRemove: vi.fn(),
    piSkillOpen: vi.fn(),
    piSkillsOpenDir: vi.fn(),
  },
}))

const inventory: PiSkillInventory = {
  agentDir: 'C:\\Users\\u\\.pi\\agent',
  piSkillsDir: 'C:\\Users\\u\\.pi\\agent\\skills',
  agentsSkillsDir: 'C:\\Users\\u\\.agents\\skills',
  skillCommandsEnabled: true,
  configuredPaths: [
    { path: 'C:\\Users\\u\\.codex\\skills', exists: true },
    { path: 'D:\\missing\\skills', exists: false },
  ],
  skills: [
    {
      name: 'local-review',
      description: 'Review local code changes.',
      path: 'C:\\Users\\u\\.pi\\agent\\skills\\local-review\\SKILL.md',
      sourceKind: 'pi',
      packageSource: null,
      packageRoot: null,
      enabled: true,
      canToggle: true,
      canRemove: true,
    },
    {
      name: 'shared-docs',
      description: 'Shared documentation workflow.',
      path: 'C:\\Users\\u\\.agents\\skills\\shared-docs\\SKILL.md',
      sourceKind: 'agents',
      packageSource: null,
      packageRoot: null,
      enabled: false,
      canToggle: true,
      canRemove: true,
    },
    {
      name: 'package-search',
      description: 'Search from a Package.',
      path: 'C:\\Users\\u\\.pi\\agent\\npm\\pkg\\skills\\search\\SKILL.md',
      sourceKind: 'package',
      packageSource: 'npm:pi-skills',
      packageRoot: 'C:\\Users\\u\\.pi\\agent\\npm\\pkg',
      enabled: true,
      canToggle: true,
      canRemove: false,
    },
    {
      name: 'codex-skill',
      description: null,
      path: 'C:\\Users\\u\\.codex\\skills\\codex-skill\\SKILL.md',
      sourceKind: 'configured',
      packageSource: null,
      packageRoot: null,
      enabled: true,
      canToggle: true,
      canRemove: false,
    },
  ],
}

describe('PiSkillsSettings', () => {
  beforeEach(() => {
    vi.mocked(open).mockReset()
    vi.mocked(chatApi.piSkillsInventory).mockReset()
    vi.mocked(chatApi.piSkillSetEnabled).mockReset()
    vi.mocked(chatApi.piSkillCommandsSetEnabled).mockReset()
    vi.mocked(chatApi.piSkillAddPath).mockReset()
    vi.mocked(chatApi.piSkillRemovePath).mockReset()
    vi.mocked(chatApi.piSkillRemove).mockReset()
    vi.mocked(chatApi.piSkillOpen).mockReset()
    vi.mocked(chatApi.piSkillsOpenDir).mockReset()
    vi.mocked(chatApi.piSkillsInventory).mockResolvedValue(inventory)
    vi.mocked(chatApi.piSkillSetEnabled).mockResolvedValue()
    vi.mocked(chatApi.piSkillCommandsSetEnabled).mockResolvedValue()
    vi.mocked(chatApi.piSkillAddPath).mockResolvedValue()
    vi.mocked(chatApi.piSkillRemovePath).mockResolvedValue()
    vi.mocked(chatApi.piSkillRemove).mockResolvedValue()
    vi.mocked(chatApi.piSkillOpen).mockResolvedValue()
    vi.mocked(chatApi.piSkillsOpenDir).mockResolvedValue()
  })

  it('groups all Pi global Skill sources and exposes safe actions', async () => {
    render(<PiSkillsSettings lang="zh" onBack={vi.fn()} />)

    expect(await screen.findByText('Pi 全局 Skill')).toBeInTheDocument()
    expect(screen.getByText('共享 .agents Skill')).toBeInTheDocument()
    expect(screen.getByText('Package Skill')).toBeInTheDocument()
    expect(screen.getByText('额外扫描路径 Skill')).toBeInTheDocument()

    const local = screen.getByText('local-review').closest<HTMLElement>('.kv-row')!
    expect(within(local).getByRole('switch')).toHaveAttribute('aria-checked', 'true')
    expect(within(local).getByRole('button', { name: '删除本地 Skill' })).toBeInTheDocument()

    const packageSkill = screen.getByText('package-search').closest<HTMLElement>('.kv-row')!
    expect(within(packageSkill).getByText('npm:pi-skills')).toBeInTheDocument()
    expect(within(packageSkill).queryByRole('button', { name: '删除本地 Skill' })).not.toBeInTheDocument()
    expect(screen.getByText('路径不存在')).toBeInTheDocument()
  })

  it('toggles an individual Skill and the global slash-command setting', async () => {
    render(<PiSkillsSettings lang="zh" onBack={vi.fn()} />)
    const local = (await screen.findByText('local-review')).closest<HTMLElement>('.kv-row')!
    fireEvent.click(within(local).getByRole('switch'))

    await waitFor(() => {
      expect(chatApi.piSkillSetEnabled).toHaveBeenCalledWith(inventory.skills[0], false)
    })

    const commandRow = screen.getByText('注册 /skill:name 命令').closest<HTMLElement>('.kv-row')!
    fireEvent.click(within(commandRow).getByRole('switch'))
    await waitFor(() => {
      expect(chatApi.piSkillCommandsSetEnabled).toHaveBeenCalledWith(false)
    })
  })

  it('adds a scan path from the Pi directory and removes configured paths', async () => {
    vi.mocked(open).mockResolvedValue('C:\\Users\\u\\.claude\\skills')
    render(<PiSkillsSettings lang="zh" onBack={vi.fn()} />)
    await screen.findByText('local-review')

    fireEvent.click(screen.getByRole('button', { name: '添加扫描路径' }))
    await waitFor(() => {
      expect(open).toHaveBeenCalledWith({
        multiple: false,
        directory: true,
        defaultPath: inventory.agentDir,
      })
      expect(chatApi.piSkillAddPath).toHaveBeenCalledWith('C:\\Users\\u\\.claude\\skills')
    })

    const pathRow = screen
      .getByText('C:\\Users\\u\\.codex\\skills')
      .closest<HTMLElement>('.kv-row')!
    fireEvent.click(within(pathRow).getByRole('button', { name: '移除扫描路径' }))
    await waitFor(() => {
      expect(chatApi.piSkillRemovePath).toHaveBeenCalledWith('C:\\Users\\u\\.codex\\skills')
    })
  })
})
