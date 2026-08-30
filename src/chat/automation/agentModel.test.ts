import { describe, expect, it } from 'vitest'
import { collapseToolIds, composeAgent, explodeInlineAgents, isAgentSlotFilled, normalizeAgent, toAgentData, agentSelectedModel } from './agentModel'

describe('normalizeAgent', () => {
  it('defaults to builtin Kivio Agent with an empty prompt', () => {
    const agent = normalizeAgent(undefined)
    expect(agent.runtimeKind).toBe('builtin')
    expect(agent.prompt).toBe('')
    expect(agent.toolIds).toEqual([])
    expect(agent.skillIds).toEqual([])
  })

  it('merges legacy skillId into skillIds without duplicating', () => {
    expect(normalizeAgent({ prompt: '', skillId: 'pdf' }).skillIds).toEqual(['pdf'])
    expect(normalizeAgent({ prompt: '', skillId: 'pdf', skillIds: ['pdf', 'docx'] }).skillIds)
      .toEqual(['pdf', 'docx'])
  })

  it('round-trips through toAgentData', () => {
    const agent = normalizeAgent({
      prompt: 'hello',
      runtimeKind: 'external',
      externalAgentId: 'claude',
      externalModel: 'sonnet',
      toolIds: ['native__read', 'native__read'],
      skillIds: ['pdf'],
    })
    expect(agent.toolIds).toEqual(['native__read'])
    expect(toAgentData(agent).externalAgentId).toBe('claude')
    expect(toAgentData(agent).skillId).toBe('pdf')
  })

  it('drops the other runtime family’s model so it cannot leak into the subtitle', () => {
    const data = toAgentData({
      prompt: '',
      runtimeKind: 'external',
      externalAgentId: 'claude',
      externalModel: 'sonnet',
      providerId: 'openai',
      model: 'claude-fable-5',
      toolIds: [],
      skillIds: [],
    })
    expect(data.model).toBeNull()
    expect(data.providerId).toBeNull()
    expect(data.externalModel).toBe('sonnet')
  })

  it('reads only the model that belongs to the current runtime', () => {
    expect(agentSelectedModel(normalizeAgent({
      prompt: '',
      runtimeKind: 'external',
      externalAgentId: 'claude',
      externalModel: 'sonnet',
      model: 'claude-fable-5',
    }))).toBe('sonnet')
    expect(agentSelectedModel(normalizeAgent({
      prompt: '',
      runtimeKind: 'builtin',
      model: 'kivio-model',
      externalModel: 'sonnet',
    }))).toBe('kivio-model')
  })

  it('marks required slots empty until configured', () => {
    const empty = normalizeAgent({ prompt: '' })
    expect(isAgentSlotFilled('runtime', empty)).toBe(true)
    expect(isAgentSlotFilled('context', empty)).toBe(false)
    expect(isAgentSlotFilled('tool', empty)).toBe(false)
    expect(isAgentSlotFilled('skill', empty)).toBe(false)
    const hangingCli = normalizeAgent({ prompt: 'x', runtimeKind: 'external' })
    expect(isAgentSlotFilled('runtime', hangingCli)).toBe(false)
  })

  it('collapses a full selection back to the empty (all-tools) whitelist', () => {
    expect(collapseToolIds(['a', 'b'], ['a', 'b'])).toEqual([])
    expect(collapseToolIds(['a'], ['a', 'b'])).toEqual(['a'])
    expect(collapseToolIds(['a', 'b'], [])).toEqual(['a', 'b'])
  })

  it('composes plugged slot nodes over inline agent data', () => {
    const nodes = [
      { id: 'a', type: 'action.agent', data: { label: 'A', agent: { prompt: 'old' } } },
      { id: 'r', type: 'agent.runtime', data: { label: 'R', agent: { prompt: '', runtimeKind: 'chat' as const } } },
      { id: 'c', type: 'agent.context', data: { label: 'C', agent: { prompt: 'new' } } },
    ]
    const edges = [
      { source: 'r', target: 'a', targetHandle: 'runtime' },
      { source: 'c', target: 'a', targetHandle: 'context' },
    ]
    const agent = composeAgent('a', nodes, edges)
    expect(agent.runtimeKind).toBe('chat')
    expect(agent.prompt).toBe('new')
  })

  it('explodes inline agent config into slot nodes once', () => {
    const nodes = [{
      id: 'a',
      type: 'action.agent' as const,
      position: { x: 0, y: 0 },
      data: { label: 'A', agent: { prompt: 'do it', runtimeKind: 'builtin' as const } },
    }]
    const first = explodeInlineAgents(nodes, [])
    expect(first.changed).toBe(true)
    expect(first.nodes.some((node) => node.type === 'agent.runtime')).toBe(true)
    expect(first.nodes.some((node) => node.type === 'agent.context')).toBe(true)
    expect(first.edges.every((edge) => edge.target === 'a')).toBe(true)
    const second = explodeInlineAgents(first.nodes, first.edges)
    expect(second.changed).toBe(false)
  })

  it('does not copy a Kivio model onto an exploded CLI runtime node', () => {
    const nodes = [{
      id: 'a',
      type: 'action.agent' as const,
      position: { x: 0, y: 0 },
      data: {
        label: 'A',
        agent: {
          prompt: 'do it',
          runtimeKind: 'external' as const,
          externalAgentId: 'claude',
          externalModel: 'sonnet',
          providerId: 'openai',
          model: 'claude-fable-5',
        },
      },
    }]
    const runtime = explodeInlineAgents(nodes, []).nodes.find((node) => node.type === 'agent.runtime')
    expect(runtime?.data.agent?.externalModel).toBe('sonnet')
    expect(runtime?.data.agent?.model ?? null).toBeNull()
    expect(runtime?.data.agent?.providerId ?? null).toBeNull()
  })
})
