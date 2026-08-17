import { fireEvent, render, screen, waitFor, within } from '@testing-library/react'
import { beforeEach, describe, expect, it, vi } from 'vitest'
import { chatApi, type PiExtensionInventory } from '../chat/api'
import { PiExtensionsSettings } from './PiExtensionsSettings'
import { open } from '@tauri-apps/plugin-dialog'

vi.mock('../chat/api', () => ({
  chatApi: {
    piExtensionsInventory: vi.fn(),
    piExtensionSetEnabled: vi.fn(),
    piExtensionInstall: vi.fn(),
    piExtensionUpdate: vi.fn(),
    piExtensionRemove: vi.fn(),
    piExtensionOpen: vi.fn(),
    piExtensionsOpenDir: vi.fn(),
  },
}))

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
const inventory: PiExtensionInventory = {
  agentDir: '/home/u/.pi/agent',
  extensionsDir: '/home/u/.pi/agent/extensions',
  packages: [
    {
      source: 'npm:pi-mcp-adapter',
      name: 'pi-mcp-adapter',
      version: '2.25.0',
      description: 'MCP adapter extension',
      path: '/home/u/.pi/agent/npm/node_modules/pi-mcp-adapter',
      enabled: true,
      canToggle: true,
      hasExtensions: true,
      extensionEntries: 1,
      resources: ['extensions', 'skills'],
    },
    {
      source: 'npm:pi-curated-themes',
      name: 'pi-curated-themes',
      version: '0.2.1',
      description: 'Themes and skills',
      path: '/home/u/.pi/agent/npm/node_modules/pi-curated-themes',
      enabled: true,
      canToggle: false,
      hasExtensions: false,
      extensionEntries: 0,
      resources: ['skills', 'themes'],
    },
    {
      source: 'npm:custom-filtered',
      name: 'custom-filtered',
      version: '1.0.0',
      description: null,
      path: '/home/u/.pi/agent/npm/node_modules/custom-filtered',
      enabled: true,
      canToggle: false,
      hasExtensions: true,
      extensionEntries: 2,
      resources: ['extensions'],
    },
  ],
  localExtensions: [
    {
      relativePath: 'local-tool.ts',
      name: 'local-tool',
      path: '/home/u/.pi/agent/extensions/local-tool.ts',
      enabled: false,
      kind: 'file',
    },
  ],
}

describe('PiExtensionsSettings', () => {
  beforeEach(() => {
    vi.mocked(chatApi.piExtensionsInventory).mockReset()
    vi.mocked(chatApi.piExtensionSetEnabled).mockReset()
    vi.mocked(chatApi.piExtensionInstall).mockReset()
    vi.mocked(chatApi.piExtensionUpdate).mockReset()
    vi.mocked(chatApi.piExtensionRemove).mockReset()
    vi.mocked(chatApi.piExtensionOpen).mockReset()
    vi.mocked(chatApi.piExtensionsOpenDir).mockReset()
    vi.mocked(open).mockReset()
    vi.mocked(chatApi.piExtensionsInventory).mockResolvedValue(inventory)
    vi.mocked(chatApi.piExtensionSetEnabled).mockResolvedValue()
    vi.mocked(chatApi.piExtensionInstall).mockResolvedValue({ output: 'installed' })
    vi.mocked(chatApi.piExtensionUpdate).mockResolvedValue({ output: 'updated' })
    vi.mocked(chatApi.piExtensionRemove).mockResolvedValue({ output: 'removed' })
  })

  it('lists packages and local extensions with safe toggle states', async () => {
    render(<PiExtensionsSettings lang="zh" onBack={vi.fn()} />)

    const packageRow = (await screen.findByText('pi-mcp-adapter')).closest<HTMLElement>('.kv-row')!
    expect(within(packageRow).getByText('v2.25.0')).toBeInTheDocument()
    expect(within(packageRow).getByRole('switch')).toHaveAttribute('aria-checked', 'true')

    const resourceRow = screen.getByText('pi-curated-themes').closest<HTMLElement>('.kv-row')!
    expect(within(resourceRow).getByText('资源包')).toBeInTheDocument()
    expect(within(resourceRow).queryByRole('switch')).not.toBeInTheDocument()

    const filteredRow = screen.getByText('custom-filtered').closest<HTMLElement>('.kv-row')!
    expect(within(filteredRow).getByText('pi config')).toBeInTheDocument()
    expect(within(filteredRow).queryByRole('switch')).not.toBeInTheDocument()

    const localRow = screen.getByText('local-tool').closest<HTMLElement>('.kv-row')!
    expect(within(localRow).getByRole('switch')).toHaveAttribute('aria-checked', 'false')
  })

  it('toggles extensions and installs a package source', async () => {
    render(<PiExtensionsSettings lang="zh" onBack={vi.fn()} />)

    const packageRow = (await screen.findByText('pi-mcp-adapter')).closest<HTMLElement>('.kv-row')!
    fireEvent.click(within(packageRow).getByRole('switch'))
    await waitFor(() => {
      expect(chatApi.piExtensionSetEnabled).toHaveBeenCalledWith(
        'package',
        'npm:pi-mcp-adapter',
        false,
      )
    })

    const source = screen.getByPlaceholderText('npm:包名、git:仓库地址或本地路径')
    fireEvent.change(source, { target: { value: 'npm:example-extension' } })
    fireEvent.click(screen.getByRole('button', { name: '安装' }))
    await waitFor(() => {
      expect(chatApi.piExtensionInstall).toHaveBeenCalledWith('npm:example-extension')
    })
  })

  it('starts the local package picker from the Pi global directory', async () => {
    vi.mocked(open).mockResolvedValue('C:\\packages\\demo')
    render(<PiExtensionsSettings lang="zh" onBack={vi.fn()} />)

    await screen.findByText('pi-mcp-adapter')
    fireEvent.click(screen.getByRole('button', { name: '选择本地 Package 目录' }))

    await waitFor(() => {
      expect(open).toHaveBeenCalledWith({
        multiple: false,
        directory: true,
        defaultPath: inventory.agentDir,
      })
    })
    expect(screen.getByPlaceholderText('npm:包名、git:仓库地址或本地路径')).toHaveValue(
      'C:\\packages\\demo',
    )
  })
})
