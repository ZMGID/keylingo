import { describe, expect, it } from 'vitest'
import { lastRuntimeForAgentFromStore, parseLastAgentRuntime } from './lastAgentRuntime'
import { BUILTIN_AGENT_RUNTIME, CHAT_AGENT_RUNTIME } from './api'

describe('parseLastAgentRuntime', () => {
  it('认 builtin / chat', () => {
    expect(parseLastAgentRuntime({ kind: 'builtin' })).toEqual(BUILTIN_AGENT_RUNTIME)
    expect(parseLastAgentRuntime({ kind: 'chat' })).toEqual(CHAT_AGENT_RUNTIME)
  })

  it('认外部 CLI 并补 default 模型', () => {
    expect(parseLastAgentRuntime({
      kind: 'external',
      externalAgentId: 'claude',
      externalModel: 'claude-opus-5',
      externalReasoning: 'high',
    })).toEqual({
      kind: 'external',
      externalAgentId: 'claude',
      externalModel: 'claude-opus-5',
      externalReasoning: 'high',
      externalSandbox: null,
      externalAgentPreset: null,
    })
    expect(parseLastAgentRuntime({
      kind: 'external',
      externalAgentId: 'claude',
    })?.externalModel).toBe('default')
  })

  it('缺 agent 或垃圾数据返回 null', () => {
    expect(parseLastAgentRuntime(null)).toBeNull()
    expect(parseLastAgentRuntime({ kind: 'external' })).toBeNull()
    expect(parseLastAgentRuntime({ kind: 'mystery' })).toBeNull()
    expect(parseLastAgentRuntime('claude')).toBeNull()
  })
})

describe('lastRuntimeForAgentFromStore', () => {
  const claudeLast = {
    kind: 'external',
    externalAgentId: 'claude',
    externalModel: 'claude-opus-5',
    externalReasoning: 'high',
  }

  it('旧数据没有 byAgent 时，只回落当前 last 那个代理', () => {
    expect(lastRuntimeForAgentFromStore(claudeLast, 'claude')?.externalModel).toBe('claude-opus-5')
    expect(lastRuntimeForAgentFromStore(claudeLast, 'dsh')).toBeNull()
  })

  it('切走之后仍能从 byAgent 取回 Claude 的模型和思考档', () => {
    const store = {
      kind: 'external',
      externalAgentId: 'dsh',
      externalModel: 'deepseek-v4-flash',
      externalReasoning: 'max',
      byAgent: {
        claude: { externalModel: 'claude-opus-5', externalReasoning: 'high' },
        dsh: { externalModel: 'deepseek-v4-flash', externalReasoning: 'max' },
      },
    }
    expect(lastRuntimeForAgentFromStore(store, 'claude')).toEqual({
      externalModel: 'claude-opus-5',
      externalReasoning: 'high',
      externalSandbox: null,
      externalAgentPreset: null,
    })
  })
})
