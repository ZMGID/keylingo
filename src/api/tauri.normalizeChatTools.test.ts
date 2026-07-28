import { describe, expect, it } from 'vitest'
import { normalizeChatTools, type ChatToolsConfig, type HookDef } from './tauri'

function hook(overrides: Partial<HookDef> = {}): HookDef {
  return {
    id: 'hook-1',
    name: '通知',
    description: '',
    event: 'agent_end',
    enabled: true,
    type: 'command',
    script: 'osascript -e \'display notification "x"\'',
    url: '',
    method: 'POST',
    headers: {},
    timeoutMs: 60_000,
    ...overrides,
  }
}

describe('normalizeChatTools', () => {
  it('保留 hooks（回归：曾被白名单重建静默丢弃，保存后 Hook 消失）', () => {
    const out = normalizeChatTools({ hooks: [hook()] })
    expect(out.hooks).toHaveLength(1)
    expect(out.hooks?.[0].event).toBe('agent_end')
    expect(out.hooks?.[0].script).toContain('osascript')
  })

  it('hooks 缺失或非数组时归一到空数组', () => {
    expect(normalizeChatTools({}).hooks).toEqual([])
    expect(normalizeChatTools({ hooks: 'nope' as unknown as HookDef[] }).hooks).toEqual([])
  })

  /**
   * 白名单重建的守门测试：喂一份「每个字段都非默认值」的完整配置，逐字段比对。
   * 这是防下一次遗漏的关键——单独给 hooks 写测试只能防住 hooks 自己。
   */
  it('normalize_chat_tools_keeps_every_field', () => {
    const full: ChatToolsConfig = {
      enabled: true,
      servers: [{ id: 's1' } as ChatToolsConfig['servers'][number]],
      hooks: [hook()],
      skillScanPaths: ['/tmp/skills'],
      skillAutoMatch: false,
      skillFallbackMode: 'skill_md_only',
      disabledSkillIds: ['pdf'],
      maxToolRounds: 7,
      toolTimeoutMs: 12_345,
      mcpIdleTimeoutMs: 54_321,
      approvalPolicy: 'always_confirm',
      subAgentConcurrency: 5,
      subAgentProviderId: 'prov',
      subAgentModel: 'model-x',
      requestDebugEnabled: true,
      nativeTools: {
        workingDirectory: '/tmp/work',
      } as ChatToolsConfig['nativeTools'],
    }

    const out = normalizeChatTools(full)
    for (const key of Object.keys(full) as (keyof ChatToolsConfig)[]) {
      // nativeTools 故意与 defaultNativeTools() 合并（补全缺失的开关），所以只断言
      // 传入的字段没被丢；其余字段要求逐一原样保留。
      if (key === 'nativeTools') {
        expect(out.nativeTools.workingDirectory).toBe('/tmp/work')
        continue
      }
      expect(out[key], `normalizeChatTools dropped or altered "${key}"`).toEqual(full[key])
    }
  })
})
