import { act, fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { InputBar } from './InputBar'
import { derivePermissionModes } from './permissionModes'
import type { AgentRuntimeConfig, DetectedExternalAgent } from './types'

vi.mock('@tauri-apps/plugin-dialog', () => ({ open: vi.fn() }))
vi.mock('@tauri-apps/api/webview', () => ({
  getCurrentWebview: () => ({ onDragDropEvent: () => Promise.resolve(() => {}) }),
}))
vi.mock('@tauri-apps/api/window', () => ({
  getCurrentWindow: () => ({ onFocusChanged: () => Promise.resolve(() => {}) }),
}))
vi.mock('../api/tauri', () => ({ api: {}, isTauriRuntime: () => false }))
vi.mock('./api', () => ({
  chatApi: {
    getProjects: () => Promise.resolve([]),
    listExternalCliSlashCommands: () => Promise.resolve({ commands: [] }),
  },
}))

const claudeAgents: DetectedExternalAgent[] = [
  {
    id: 'claude',
    name: 'Claude Code',
    available: true,
    models: [],
    sandboxOptions: [
      { id: 'plan', label: '计划 (只读)' },
      { id: 'default', label: '每次确认' },
      { id: 'acceptEdits', label: '接受编辑' },
      { id: 'bypassPermissions', label: '完全 (默认)' },
    ],
  },
]

function renderComposer(modes: { options: ReturnType<typeof derivePermissionModes>['options']; current: string }, onModeChange = vi.fn()) {
  render(
    <InputBar
      onSend={() => {}}
      modeOptions={modes.options}
      modeValue={modes.current}
      onModeChange={onModeChange}
    />,
  )
  return onModeChange
}

function openModeMenu(pillLabel: string) {
  act(() => {
    fireEvent.click(screen.getByTitle('切换模式'))
  })
  expect(screen.getByTitle('切换模式')).toHaveTextContent(pillLabel)
}

describe('InputBar 底栏模式胶囊', () => {
  it('内置模型会话仍然是 Act / Plan / Orchestrate 三档', () => {
    const runtime: AgentRuntimeConfig = { kind: 'builtin' }
    renderComposer(derivePermissionModes({
      target: 'composer',
      agentRuntime: runtime,
      agentPlanMode: 'act',
    }))
    openModeMenu('Act')
    const items = screen.getAllByRole('menuitemradio')
    expect(items.map((item) => item.textContent)).toEqual([
      'Act普通模式 · Normal',
      'Plan计划模式 · Enter plan mode',
      'Orchestrate主动派 Subagent · Proactive subagents',
    ])
    expect(items[0]).toHaveAttribute('aria-checked', 'true')
  })


  it('本地 CLI 会话显示该 CLI 的档位，点选回传档位 id', () => {
    const runtime: AgentRuntimeConfig = {
      kind: 'external',
      externalAgentId: 'claude',
      externalSandbox: 'plan',
    }
    const onModeChange = renderComposer(derivePermissionModes({
      target: 'composer',
      agentRuntime: runtime,
      agents: claudeAgents,
    }))
    openModeMenu('计划 (只读)')
    const items = screen.getAllByRole('menuitemradio')
    expect(items.map((item) => item.textContent)).toEqual([
      '计划 (只读)',
      '每次确认',
      '接受编辑',
      '完全 (默认)',
    ])
    expect(items[0]).toHaveAttribute('aria-checked', 'true')

    act(() => {
      fireEvent.click(items[2])
    })
    expect(onModeChange).toHaveBeenCalledWith('acceptEdits')
  })

  it('该 CLI 没有档位时胶囊整个不渲染', () => {
    const runtime: AgentRuntimeConfig = { kind: 'external', externalAgentId: 'opencode' }
    renderComposer(derivePermissionModes({
      target: 'composer',
      agentRuntime: runtime,
      agents: [{ id: 'opencode', name: 'OpenCode', available: true, models: [], sandboxOptions: [] }],
    }))
    expect(screen.queryByTitle('切换模式')).not.toBeInTheDocument()
  })
})
