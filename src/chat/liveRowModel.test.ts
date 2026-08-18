import { describe, expect, it } from 'vitest'
import { createLiveRowModel } from './liveRowModel'

function sync(
  model: ReturnType<typeof createLiveRowModel>,
  partial: Partial<Parameters<ReturnType<typeof createLiveRowModel>['sync']>[0]> & {
    liveActive: boolean
  },
) {
  return model.sync({
    conversationId: 'c1',
    liveGroupId: null,
    preferredTwinId: null,
    historyAssistantIds: [],
    historyGroupIds: [],
    ...partial,
  })
}

describe('createLiveRowModel', () => {
  it('settling a live turn keys the committed twin with the live row key', () => {
    const model = createLiveRowModel()

    const streaming = sync(model, {
      liveActive: true,
      historyAssistantIds: [],
    })
    expect(streaming.liveKey).toMatch(/^live-turn-/)
    const liveKey = streaming.liveKey!

    const settled = sync(model, {
      liveActive: false,
      preferredTwinId: 'a1',
      historyAssistantIds: ['a1'],
    })
    expect(settled.liveKey).toBeNull()
    expect(model.resolveMessageKey('a1')).toBe(liveKey)
    expect(model.resolveMessageKey('other')).toBe('other')
  })

  it('persist lag: alias still lands when history commits a build later', () => {
    const model = createLiveRowModel()

    const streaming = sync(model, { liveActive: true })
    const liveKey = streaming.liveKey!

    // Run ended but twin has not landed yet.
    sync(model, {
      liveActive: false,
      historyAssistantIds: [],
    })
    expect(model.resolveMessageKey('a1')).toBe('a1')

    sync(model, {
      liveActive: false,
      preferredTwinId: 'a1',
      historyAssistantIds: ['a1'],
    })
    expect(model.resolveMessageKey('a1')).toBe(liveKey)
  })

  it('a new turn supersedes an unresolved settle so aliases never cross turns', () => {
    const model = createLiveRowModel()

    sync(model, { liveActive: true })
    sync(model, { liveActive: false, historyAssistantIds: [] }) // twin never landed

    const second = sync(model, {
      liveActive: true,
      historyAssistantIds: [],
    })
    const secondLiveKey = second.liveKey!

    sync(model, {
      liveActive: false,
      preferredTwinId: 'a2',
      historyAssistantIds: ['a2'],
    })
    expect(model.resolveMessageKey('a2')).toBe(secondLiveKey)
  })

  it('multi-model group reuses live-group key for the settled group row', () => {
    const model = createLiveRowModel()

    const streaming = sync(model, {
      liveActive: true,
      liveGroupId: 'g1',
    })
    expect(streaming.liveKey).toBe('live-group-g1')

    sync(model, {
      liveActive: false,
      liveGroupId: null,
      historyGroupIds: ['g1'],
      historyAssistantIds: ['a1', 'a2'],
    })
    expect(model.resolveGroupKey('g1')).toBe('live-group-g1')
    // Default group key when no origin:
    expect(model.resolveGroupKey('other')).toBe('group-other')
  })

  it('conversation switch drops aliases', () => {
    const model = createLiveRowModel()

    const streaming = sync(model, {
      conversationId: 'c1',
      liveActive: true,
    })
    const liveKey = streaming.liveKey!
    sync(model, {
      conversationId: 'c1',
      liveActive: false,
      preferredTwinId: 'a1',
      historyAssistantIds: ['a1'],
    })
    model.sync({
      conversationId: 'c1',
      liveActive: false,
      liveGroupId: null,
      preferredTwinId: null,
      historyAssistantIds: ['a1'],
      historyGroupIds: [],
    })
    expect(model.resolveMessageKey('a1')).toBe(liveKey)

    // New conversation:
    const next = model.sync({
      conversationId: 'c2',
      liveActive: true,
      liveGroupId: null,
      preferredTwinId: null,
      historyAssistantIds: [],
      historyGroupIds: [],
    })
    expect(next.liveKey).toMatch(/^live-turn-/)
    expect(model.resolveMessageKey('a1')).toBe('a1')
  })
})
