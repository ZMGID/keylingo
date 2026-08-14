import { describe, expect, it } from 'vitest'
import { deriveDshPresetModes, derivePermissionModes } from './permissionModes'
import type { AgentRuntimeConfig, DetectedExternalAgent } from './types'

const builtinRuntime: AgentRuntimeConfig = { kind: 'builtin' }
const chatRuntime: AgentRuntimeConfig = { kind: 'chat' }

function externalRuntime(id: string, sandbox?: string | null): AgentRuntimeConfig {
  return { kind: 'external', externalAgentId: id, externalSandbox: sandbox ?? null }
}

// 后端 detection::sandbox_options_for 的档位表（claude 四档 / codex 三档 / opencode 无档位）。
const agents: DetectedExternalAgent[] = [
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
  {
    id: 'codex',
    name: 'Codex',
    available: true,
    models: [],
    sandboxOptions: [
      { id: 'read-only', label: '只读' },
      { id: 'workspace-write', label: '工作区写 (默认)' },
      { id: 'danger-full-access', label: '完全' },
    ],
  },
  { id: 'opencode', name: 'OpenCode', available: true, models: [], sandboxOptions: [] },
]

describe('derivePermissionModes（底栏模式胶囊）', () => {
  it('claude 会话给它自己的四档', () => {
    const { options } = derivePermissionModes({
      target: 'composer',
      agentRuntime: externalRuntime('claude'),
      agents,
    })
    expect(options.map((o) => o.value)).toEqual(['plan', 'default', 'acceptEdits', 'bypassPermissions'])
    expect(options.map((o) => o.label)).toEqual(['计划 (只读)', '每次确认', '接受编辑', '完全 (默认)'])
  })

  it('codex 会话给它自己的三档', () => {
    const { options } = derivePermissionModes({
      target: 'composer',
      agentRuntime: externalRuntime('codex'),
      agents,
    })
    expect(options.map((o) => o.value)).toEqual(['read-only', 'workspace-write', 'danger-full-access'])
  })

  it('opencode 没有档位 → 空表（胶囊隐藏）', () => {
    const { options } = derivePermissionModes({
      target: 'composer',
      agentRuntime: externalRuntime('opencode'),
      agents,
    })
    expect(options).toEqual([])
  })

  it('还没探测到 agent 列表时也是空表，不会退回 Kivio 三档', () => {
    const { options } = derivePermissionModes({
      target: 'composer',
      agentRuntime: externalRuntime('claude'),
      agents: [],
    })
    expect(options).toEqual([])
  })

  it('未显式选过档位时跟随 CLI 标了「默认」的那档', () => {
    expect(derivePermissionModes({
      target: 'composer',
      agentRuntime: externalRuntime('claude'),
      agents,
    }).current).toBe('bypassPermissions')
    expect(derivePermissionModes({
      target: 'composer',
      agentRuntime: externalRuntime('codex'),
      agents,
    }).current).toBe('workspace-write')
  })

  it('显式选过就用选中的那档', () => {
    expect(derivePermissionModes({
      target: 'composer',
      agentRuntime: externalRuntime('claude', 'plan'),
      agents,
    }).current).toBe('plan')
  })

  it('内置 Agent 会话给 Kivio 三档', () => {
    const { options, current } = derivePermissionModes({
      target: 'composer',
      agentRuntime: builtinRuntime,
      agents,
      agentPlanMode: 'plan',
    })
    expect(options.map((o) => o.value)).toEqual(['act', 'plan', 'orchestrate'])
    expect(options.map((o) => o.label)).toEqual(['Act', 'Plan', 'Orchestrate'])
    expect(current).toBe('plan')
  })

  it('内置会话没有档位状态时回落 act', () => {
    expect(derivePermissionModes({
      target: 'composer',
      agentRuntime: builtinRuntime,
      agentPlanMode: null,
    }).current).toBe('act')
  })

  it('Kivio Chat 运行时底栏无 Act/Plan/Orchestrate（独立 runtime）', () => {
    expect(derivePermissionModes({
      target: 'composer',
      agentRuntime: chatRuntime,
      agents,
      agentPlanMode: 'act',
    }).options).toEqual([])
  })
})

describe('derivePermissionModes（顶栏权限按钮）', () => {
  it('内置会话给工具审批策略三档', () => {
    const { options, current } = derivePermissionModes({
      target: 'titlebar',
      agentRuntime: builtinRuntime,
      approvalPolicy: 'auto',
    })
    expect(options.map((o) => o.value)).toEqual([
      'always_confirm',
      'readonly_auto_sensitive_confirm',
      'auto',
    ])
    expect(current).toBe('auto')
  })

  it('没有显式策略时回落到「敏感确认」', () => {
    expect(derivePermissionModes({
      target: 'titlebar',
      agentRuntime: builtinRuntime,
    }).current).toBe('readonly_auto_sensitive_confirm')
  })

  it('本地 CLI 会话给空表（顶栏隐藏，档位归底栏胶囊一处管）', () => {
    for (const id of ['claude', 'codex', 'opencode']) {
      expect(derivePermissionModes({
        target: 'titlebar',
        agentRuntime: externalRuntime(id),
        agents,
      }).options).toEqual([])
    }
  })

  it('Kivio Chat 顶栏也无档位表', () => {
    expect(derivePermissionModes({
      target: 'titlebar',
      agentRuntime: chatRuntime,
    }).options).toEqual([])
  })
})

describe('deriveDshPresetModes（底栏 Agent 模式胶囊）', () => {
  it('dsh 会话给出四档 Agent 模式，未选时回落 standard', () => {
    expect(deriveDshPresetModes(externalRuntime('dsh')).options.map((o) => o.value)).toEqual([
      'standard',
      'code',
      'minimal',
      'cordis',
    ])
    expect(deriveDshPresetModes(externalRuntime('dsh')).current).toBe('standard')
    expect(deriveDshPresetModes({
      kind: 'external',
      externalAgentId: 'dsh',
      externalAgentPreset: 'minimal',
    }).current).toBe('minimal')
  })

  it('非 dsh 会话不显示 Agent 模式胶囊', () => {
    expect(deriveDshPresetModes(externalRuntime('claude')).options).toEqual([])
    expect(deriveDshPresetModes(builtinRuntime).options).toEqual([])
  })
})
