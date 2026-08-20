import { beforeEach, describe, expect, it, vi } from 'vitest'
import type {
  ChatRunEventEnvelope,
  ChatRunSnapshot,
} from '../generated/chatProtocol'
import {
  chatProtocolTesting,
  subscribeChatProtocolIssues,
  syncChatProtocol,
} from './chatProtocol'

const { invokeMock } = vi.hoisted(() => ({ invokeMock: vi.fn() }))
vi.mock('@tauri-apps/api/core', () => ({ invoke: invokeMock }))
vi.mock('@tauri-apps/api/event', () => ({ listen: async () => () => {} }))

function event(seq: number, type: ChatRunEventEnvelope['type'] = 'run_started') {
  const common = {
    protocolVersion: 1,
    scope: 'run' as const,
    conversationId: 'conversation',
    runId: 'run',
    messageId: 'message',
    seq,
    baseRevision: 0,
  }
  if (type === 'text_delta') {
    return { ...common, type, delta: `${seq}`, segment: null } as ChatRunEventEnvelope
  }
  if (type === 'run_completed') {
    return { ...common, type, full: 'done', conversationRevision: 2 } as ChatRunEventEnvelope
  }
  if (type === 'hook_failed') {
    return {
      ...common,
      type,
      hookName: 'after',
      event: 'agent_end',
      message: 'boom',
    } as ChatRunEventEnvelope
  }
  return { ...common, type: 'run_started', recovery: null } as ChatRunEventEnvelope
}

function snapshot(overrides: Partial<ChatRunSnapshot> = {}): ChatRunSnapshot {
  return {
    protocolVersion: 1,
    conversationId: 'conversation',
    runId: 'run',
    messageId: 'message',
    lastSeq: 7,
    baseRevision: 1,
    recovery: null,
    status: 'running',
    content: '',
    reasoning: '',
    segments: [],
    tools: [],
    contextUsage: null,
    subagents: [],
    compaction: null,
    todoState: null,
    planState: null,
    pendingInteractions: [],
    warnings: [],
    statusNote: null,
    terminal: null,
    ...overrides,
  }
}

describe('chat protocol sequencing', () => {
  // 缺口会真的触发一次 sync（applyEvent 期间进 liveDuringSync，sync 收尾时才排干），
  // 所以这里必须给 invoke 一个合法空结果，并在断言前 flush 掉那一轮异步。
  beforeEach(() => {
    chatProtocolTesting.reset()
    invokeMock.mockReset()
    invokeMock.mockResolvedValue({
      protocolVersion: 1,
      conversationRevision: 0,
      missingRunIds: [],
      runs: [],
    })
  })

  const flushSync = () => new Promise((resolve) => setTimeout(resolve, 0))

  it('drops duplicates and drains a buffered gap in order', async () => {
    const seen: number[] = []
    chatProtocolTesting.subscribe((item) => {
      if (item.scope === 'run') seen.push(item.seq)
    })
    chatProtocolTesting.ingest(event(1))
    chatProtocolTesting.ingest(event(3, 'text_delta'))
    chatProtocolTesting.ingest(event(1))
    chatProtocolTesting.ingest(event(2, 'text_delta'))
    await flushSync()
    expect(seen).toEqual([1, 2, 3])
  })

  it('rejects late nonduplicate events after a continuous terminal', () => {
    const seen: number[] = []
    chatProtocolTesting.subscribe((item) => {
      if (item.scope === 'run') seen.push(item.seq)
    })
    chatProtocolTesting.ingest(event(1))
    chatProtocolTesting.ingest(event(2, 'run_completed'))
    chatProtocolTesting.ingest(event(3, 'text_delta'))
    expect(seen).toEqual([1, 2])
  })

  it('still delivers hook_failed after the run is terminal', () => {
    const seen: string[] = []
    chatProtocolTesting.subscribe((item) => {
      if (item.scope === 'run') seen.push(item.type)
    })
    chatProtocolTesting.ingest(event(1))
    chatProtocolTesting.ingest(event(2, 'run_completed'))
    chatProtocolTesting.ingest(event(3, 'hook_failed'))
    chatProtocolTesting.ingest(event(0, 'hook_failed'))
    chatProtocolTesting.ingest(event(4, 'text_delta'))
    expect(seen).toEqual(['run_started', 'run_completed', 'hook_failed', 'hook_failed'])
  })

  it('commits a buffered terminal only after the sequence gap closes', async () => {
    const seen: number[] = []
    chatProtocolTesting.subscribe((item) => {
      if (item.scope === 'run') seen.push(item.seq)
    })
    chatProtocolTesting.ingest(event(1))
    chatProtocolTesting.ingest(event(3, 'run_completed'))
    chatProtocolTesting.ingest(event(4, 'text_delta'))
    chatProtocolTesting.ingest(event(2, 'text_delta'))
    await flushSync()
    expect(seen).toEqual([1, 2, 3])
  })

  it('keeps the stream alive when a subscriber throws', () => {
    const errorSpy = vi.spyOn(console, 'error').mockImplementation(() => {})
    const seen: number[] = []
    chatProtocolTesting.subscribe(() => {
      throw new Error('bad handler')
    })
    chatProtocolTesting.subscribe((item) => {
      if (item.scope === 'run') seen.push(item.seq)
    })

    chatProtocolTesting.ingest(event(1))
    chatProtocolTesting.ingest(event(2, 'text_delta'))
    chatProtocolTesting.ingest(event(3, 'run_completed'))

    expect(seen).toEqual([1, 2, 3])
    expect(errorSpy).toHaveBeenCalled()
    errorSpy.mockRestore()
  })

  it('drops the run state instead of buffering an unbounded gap', async () => {
    const seen: number[] = []
    chatProtocolTesting.subscribe((item) => {
      if (item.scope === 'run') seen.push(item.seq)
    })
    chatProtocolTesting.ingest(event(1))
    for (let seq = 3; seq < 700; seq += 1) chatProtocolTesting.ingest(event(seq, 'text_delta'))
    await flushSync()

    // 状态被丢掉后，序列从头开始也能重新收下（等待 sync 回快照）。
    chatProtocolTesting.ingest(event(1))
    expect(seen).toEqual([1, 1])
  })

  it('drops the sync-time queue instead of buffering it unbounded', async () => {
    const seen: number[] = []
    chatProtocolTesting.subscribe((item) => {
      if (item.scope === 'run') seen.push(item.seq)
    })
    chatProtocolTesting.ingest(event(1))

    // sync 在飞的时候 live 事件不走 pending，改道进 liveDuringSync——那道队列
    // 也得有上限，否则一次快速回答就能把内存撑起来。
    let releaseSync: (value: unknown) => void = () => {}
    invokeMock.mockReturnValueOnce(
      new Promise((resolve) => {
        releaseSync = resolve
      }),
    )
    const inFlight = syncChatProtocol('conversation')
    for (let seq = 2; seq < 700; seq += 1) chatProtocolTesting.ingest(event(seq, 'text_delta'))
    releaseSync({
      protocolVersion: 1,
      conversationRevision: 0,
      missingRunIds: [],
      runs: [],
    })
    await inFlight
    await flushSync()

    // 队列被丢掉：那 698 帧一个都没派发出来，只有最初那条 seq=1。
    expect(seen).toEqual([1])
    // run 状态也一并丢掉，序列从头开始能重新收下（等 sync 回快照）。
    chatProtocolTesting.ingest(event(1))
    expect(seen).toEqual([1, 1])
  })

  it('rejects regressed conversation revisions', () => {
    const revisions: number[] = []
    chatProtocolTesting.subscribe((item) => {
      if (item.scope === 'conversation') revisions.push(item.revision)
    })
    const conversationEvent = (revision: number) => ({
      protocolVersion: 1 as const,
      scope: 'conversation' as const,
      conversationId: 'conversation',
      revision,
      type: 'plan_updated' as const,
      planState: { mode: 'act' as const, status: 'empty' as const, plan: null, updatedAt: revision },
    })
    chatProtocolTesting.ingest(conversationEvent(2))
    chatProtocolTesting.ingest(conversationEvent(1))
    expect(revisions).toEqual([2])
  })

  it('uses terminal conversationRevision as a monotonic watermark', () => {
    const revisions: number[] = []
    chatProtocolTesting.subscribe((item) => {
      if (item.scope === 'conversation') revisions.push(item.revision)
    })
    chatProtocolTesting.ingest(event(1))
    chatProtocolTesting.ingest(event(2, 'run_completed'))
    chatProtocolTesting.ingest({
      protocolVersion: 1,
      scope: 'conversation',
      conversationId: 'conversation',
      revision: 1,
      type: 'plan_updated',
      planState: { mode: 'act', status: 'empty', plan: null, updatedAt: 1 },
    })
    expect(revisions).toEqual([])
  })

  it('rejects a replay whose declared range is not continuous', () => {
    chatProtocolTesting.ingest(event(1))
    expect(chatProtocolTesting.isContinuousReplay('conversation', {
      kind: 'events',
      runId: 'run',
      fromSeq: 2,
      throughSeq: 3,
      events: [event(3, 'text_delta')],
    })).toBe(false)
  })

  it('rejects semantically invalid snapshot event slots', () => {
    const invalid = snapshot({
      status: 'completed',
      terminal: {
        type: 'text_delta', delta: 'not terminal', segment: null,
      } as unknown as ChatRunSnapshot['terminal'],
    })
    expect(chatProtocolTesting.validateSync({
      protocolVersion: 1,
      conversationRevision: 1,
      missingRunIds: [],
      runs: [{ kind: 'snapshot', snapshot: invalid }],
    })).toBe(false)
    expect(chatProtocolTesting.isSemanticallyValidSnapshot('conversation', invalid)).toBe(false)
  })

  it('rejects invalid fan-out recovery indexes', () => {
    const invalid = snapshot({
      recovery: {
        groupId: 'group',
        groupSize: 2,
        armIndex: 2,
        providerId: 'provider',
        model: 'model',
      },
    })
    expect(chatProtocolTesting.isSemanticallyValidSnapshot('conversation', invalid)).toBe(false)
  })

  it('rejects unknown fields and protocol versions at the schema boundary', () => {
    expect(chatProtocolTesting.validate(event(1))).toBe(true)
    expect(chatProtocolTesting.validate({ ...event(1), extra: true })).toBe(false)
    expect(chatProtocolTesting.validate({ ...event(1), protocolVersion: 2 })).toBe(false)
    expect(chatProtocolTesting.validate({
      ...event(1),
      type: 'todo_updated',
      todoState: { items: [], updatedAt: 1, extra: true },
    })).toBe(false)
  })

  it('restores the complete segment timeline from a run snapshot', () => {
    const seen: ChatRunEventEnvelope[] = []
    chatProtocolTesting.subscribe((item) => {
      if (item.scope === 'run') seen.push(item)
    })

    chatProtocolTesting.applySnapshot(snapshot({
      reasoning: 'thinking',
      content: 'answer',
      segments: [
        {
          id: 'text', kind: 'text', phase: 'synthesis', order: 3,
          stepNumber: 1, round: 1, text: 'answer', toolCallId: null,
        },
        {
          id: 'reasoning', kind: 'reasoning', phase: 'tool_loop', order: 1,
          stepNumber: 1, round: 1, text: 'thinking', toolCallId: null,
        },
        {
          id: 'tool', kind: 'tool', phase: 'tool_loop', order: 2,
          stepNumber: 1, round: 1, text: null, toolCallId: 'call-1',
        },
      ],
    }))

    expect(seen.map((item) => item.type)).toEqual([
      'run_started', 'reasoning_delta', 'text_delta', 'text_delta',
    ])
    expect(seen.slice(1).map((item) => (
      item.type === 'text_delta' || item.type === 'reasoning_delta'
        ? item.segment?.id
        : null
    ))).toEqual(['reasoning', 'tool', 'text'])
  })

  it('marks snapshot events as restored deliveries', () => {
    const sources: string[] = []
    chatProtocolTesting.subscribe((_item, delivery) => sources.push(delivery.source))

    chatProtocolTesting.applySnapshot(snapshot({ content: 'restored' }))

    expect(sources).toEqual(['snapshot', 'snapshot'])
  })

  it('preserves the exact terminal event from a snapshot', () => {
    const errors: string[] = []
    chatProtocolTesting.subscribe((item) => {
      if (item.scope === 'run' && item.type === 'run_failed') errors.push(item.error)
    })
    chatProtocolTesting.applySnapshot(snapshot({
      status: 'failed',
      terminal: {
        type: 'run_failed',
        error: 'provider disconnected',
        full: 'partial',
        conversationRevision: 4,
      },
    }))
    expect(errors).toEqual(['provider disconnected'])
  })

  it('validates the complete sync result and rejects extra fields', () => {
    const valid = {
      protocolVersion: 1,
      conversationRevision: 3,
      missingRunIds: [],
      runs: [],
    }
    expect(chatProtocolTesting.validateSync(valid)).toBe(true)
    expect(chatProtocolTesting.validateSync({ ...valid, extra: true })).toBe(false)
    expect(chatProtocolTesting.validateSync({ ...valid, protocolVersion: 2 })).toBe(false)
  })
})

describe('chat protocol sync', () => {
  const syncResult = (conversationRevision = 0) => ({
    protocolVersion: 1,
    conversationRevision,
    missingRunIds: [],
    runs: [],
  })
  const sentCursors = (call: number) => (
    invokeMock.mock.calls[call][1] as { request: { cursors: unknown } }
  ).request.cursors

  beforeEach(() => {
    chatProtocolTesting.reset()
    invokeMock.mockReset()
  })

  it('seeds the revision on first sight instead of demanding a resync', async () => {
    const issues: string[] = []
    subscribeChatProtocolIssues((issue) => issues.push(issue))

    invokeMock.mockResolvedValue(syncResult(7))
    await syncChatProtocol('conversation')
    expect(issues).toEqual([])

    invokeMock.mockResolvedValue(syncResult(8))
    await syncChatProtocol('conversation')
    expect(issues).toEqual(['resync_required'])
  })

  it('stops sending cursors once a run reaches a terminal state', async () => {
    invokeMock.mockResolvedValue(syncResult())
    chatProtocolTesting.ingest(event(1))
    chatProtocolTesting.ingest({ ...event(1), runId: 'live-run' } as ChatRunEventEnvelope)

    await syncChatProtocol('conversation')
    expect(sentCursors(0)).toEqual([
      { runId: 'run', lastSeq: 1 },
      { runId: 'live-run', lastSeq: 1 },
    ])

    chatProtocolTesting.ingest(event(2, 'run_completed'))
    await syncChatProtocol('conversation')
    expect(sentCursors(1)).toEqual([{ runId: 'live-run', lastSeq: 1 }])
  })
})
