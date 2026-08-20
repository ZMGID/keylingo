import { act, fireEvent, render, screen } from '@testing-library/react'
import { describe, expect, it, vi } from 'vitest'
import { InputBar } from './InputBar'
import { deriveDshPresetModes, derivePermissionModes } from './permissionModes'
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

  it('dsh 会话在权限胶囊左边显示 Agent 模式胶囊', () => {
    const runtime: AgentRuntimeConfig = {
      kind: 'external',
      externalAgentId: 'dsh',
      externalSandbox: 'workspace-write',
      externalAgentPreset: 'standard',
    }
    const onPresetChange = vi.fn()
    const agents: DetectedExternalAgent[] = [{
      id: 'dsh',
      name: 'DeepSeek Harness',
      available: true,
      models: [],
      sandboxOptions: [
        { id: 'read-only', label: '只读' },
        { id: 'workspace-write', label: '工作区写 (默认)' },
        { id: 'danger-full-access', label: '完全' },
      ],
    }]
    const modes = derivePermissionModes({ target: 'composer', agentRuntime: runtime, agents })
    const presets = deriveDshPresetModes(runtime)
    render(
      <InputBar
        onSend={() => {}}
        modeOptions={modes.options}
        modeValue={modes.current}
        onModeChange={vi.fn()}
        presetOptions={presets.options}
        presetValue={presets.current}
        onPresetChange={onPresetChange}
      />,
    )
    expect(screen.getByTitle('切换 Agent 模式')).toHaveTextContent('标准模式')
    expect(screen.getByTitle('切换模式')).toHaveTextContent('工作区写 (默认)')

    act(() => {
      fireEvent.click(screen.getByTitle('切换 Agent 模式'))
    })
    const items = screen.getAllByRole('menuitemradio')
    expect(items.map((item) => item.textContent)).toEqual([
      '标准模式完整编码 Agent · Full coding agent',
      'PTC 模式Code Mode SDK · 多步工具写成一个程序',
      '极简模式仅 bash + 编辑器 · Bash and editor only',
      '创造模式编写 Agent preset · Author presets',
    ])
    const custom = deriveDshPresetModes(runtime, [
      { id: 'code-review', label: '代码审查', description: '只读评审' },
    ])
    expect(custom.options.map((option) => option.value)).toContain('code-review')
    act(() => {
      fireEvent.click(items[2])
    })
    expect(onPresetChange).toHaveBeenCalledWith('minimal')
  })

  it('已有对话时 Agent 模式菜单不可改档', () => {
    const runtime: AgentRuntimeConfig = { kind: 'external', externalAgentId: 'dsh', externalAgentPreset: 'code' }
    const onPresetChange = vi.fn()
    const presets = deriveDshPresetModes(runtime)
    render(
      <InputBar
        onSend={() => {}}
        presetOptions={presets.options}
        presetValue={presets.current}
        onPresetChange={onPresetChange}
        presetLocked
        presetLockedReason="已有对话内容后不能切换 Agent 模式"
      />,
    )
    act(() => {
      fireEvent.click(screen.getByTitle('已有对话内容后不能切换 Agent 模式'))
    })
    const items = screen.getAllByRole('menuitemradio')
    expect(items[0]).toBeDisabled()
    act(() => {
      fireEvent.click(items[0])
    })
    expect(onPresetChange).not.toHaveBeenCalled()
  })
})
