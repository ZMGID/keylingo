import { expect, it } from 'vitest'
import { withExternalModel } from './externalModelEffort'
import type { AgentRuntimeConfig } from './types'

const runtime: AgentRuntimeConfig = { kind: 'external', externalAgentId: 'antigravity', externalReasoning: 'high' }

it('removes stale overrides when selecting an Antigravity effort variant', () => {
  expect(withExternalModel(runtime, 'gemini-3.8-flash-low').externalReasoning).toBeNull()
  expect(withExternalModel(runtime, 'gpt-oss-120b-medium', 'high').externalReasoning).toBeNull()
})

it('distinguishes explicit clearing from retaining an unspecified effort', () => {
  expect(withExternalModel(runtime, 'claude-sonnet-4-6', null).externalReasoning).toBeNull()
  expect(withExternalModel(runtime, 'claude-sonnet-4-6').externalReasoning).toBe('high')
  expect(withExternalModel({ ...runtime, externalAgentId: 'codex' }, 'custom-low', 'high').externalReasoning).toBe('high')
})
