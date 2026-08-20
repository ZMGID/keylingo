import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import Ajv2020 from 'ajv/dist/2020'
import schema from '../generated/chatProtocol.schema.json'
import syncSchema from '../generated/chatSync.schema.json'
import {
  CHAT_PROTOCOL_VERSION,
  type ChatProtocolEvent,
  type ChatRunEvent,
  type ChatRunEventEnvelope,
  type ChatRunSnapshot,
  type ChatSyncResult,
} from '../generated/chatProtocol'

export type ChatProtocolIssue = 'version_mismatch' | 'invalid_event' | 'resync_required'
export type ChatProtocolDelivery = { source: 'live' | 'snapshot' }
type Subscriber = (event: ChatProtocolEvent, delivery: ChatProtocolDelivery) => void

type RunState = {
  conversationId: string
  lastSeq: number
  terminal: boolean
  pending: Map<number, ChatRunEventEnvelope>
}

const MAX_PENDING_EVENTS = 500

const ajv = new Ajv2020({
  allErrors: true,
  strict: false,
  formats: { float: true, uint8: true, uint32: true, uint64: true, int64: true },
})
const validateEvent = ajv.compile(schema)
const validateSync = ajv.compile(syncSchema)
const subscribers = new Set<Subscriber>()
const issueSubscribers = new Set<(issue: ChatProtocolIssue, conversationId?: string) => void>()
const runs = new Map<string, RunState>()
const syncing = new Set<string>()
const liveDuringSync = new Map<string, ChatRunEventEnvelope[]>()
const conversationRevisions = new Map<string, number>()
const syncRetryAttempts = new Map<string, number>()
const syncRetryTimers = new Map<string, ReturnType<typeof setTimeout>>()
let nativeListener: Promise<() => void> | null = null

function reportIssue(issue: ChatProtocolIssue, conversationId?: string) {
  for (const subscriber of issueSubscribers) subscriber(issue, conversationId)
}

function dispatch(event: ChatProtocolEvent, delivery: ChatProtocolDelivery = { source: 'live' }) {
  // 一个订阅者抛错不能连累其他订阅者：共用一条通道后这里是单点。
  for (const subscriber of subscribers) {
    try {
      subscriber(event, delivery)
    } catch (error) {
      console.error('Chat protocol subscriber threw', error, event)
    }
  }
}

function runState(event: ChatRunEventEnvelope): RunState {
  const existing = runs.get(event.runId)
  if (existing) return existing
  const created: RunState = {
    conversationId: event.conversationId,
    lastSeq: 0,
    terminal: false,
    pending: new Map(),
  }
  runs.set(event.runId, created)
  return created
}

function isTerminal(
  event: ChatRunEventEnvelope,
): event is Extract<ChatRunEventEnvelope, { type: 'run_completed' | 'run_failed' | 'run_cancelled' }> {
  return event.type === 'run_completed'
    || event.type === 'run_failed'
    || event.type === 'run_cancelled'
}

function advanceTerminalRevision(event: ChatRunEventEnvelope) {
  if (!isTerminal(event)) return
  const revision = event.conversationRevision
  conversationRevisions.set(
    event.conversationId,
    Math.max(conversationRevisions.get(event.conversationId) ?? 0, revision),
  )
}

function scheduleSync(conversationId: string, delayMs = 0) {
  if (syncRetryTimers.has(conversationId)) return
  const timer = setTimeout(() => {
    syncRetryTimers.delete(conversationId)
    void syncChatProtocol(conversationId)
  }, delayMs)
  syncRetryTimers.set(conversationId, timer)
}

function retrySync(conversationId: string) {
  const attempts = (syncRetryAttempts.get(conversationId) ?? 0) + 1
  syncRetryAttempts.set(conversationId, attempts)
  if (attempts <= 3) {
    scheduleSync(conversationId, 250 * attempts)
    return
  }
  // 放弃补洞时得把该会话的 run 状态一起丢掉，否则前端 reload 也带不走缺口，
  // 下次 sync 还是带着老 cursor 去要补丁。丢掉后后端会回快照。
  for (const [runId, state] of runs) {
    if (state.conversationId === conversationId) runs.delete(runId)
  }
  syncRetryAttempts.delete(conversationId)
  reportIssue('resync_required', conversationId)
}

function applyRunEvent(event: ChatRunEventEnvelope) {
  const state = runState(event)
  // HookFailed 是旁路警告，可在 run 已终态、甚至 hub 已 prune（seq=0 直播帧）之后到达。
  // 不当成内容序列的缺口，也不因 terminal 丢弃——否则黄条永远出不来。
  if (event.type === 'hook_failed') {
    if (event.seq > state.lastSeq) state.lastSeq = event.seq
    dispatch(event)
    return
  }
  if (event.seq <= state.lastSeq) return
  if (state.terminal) return
  if (event.seq > state.lastSeq + 1) {
    if (state.pending.size >= MAX_PENDING_EVENTS) {
      // 缺口补不上时 pending 会把整个回答（几千个 delta）攒进内存且永不派发。
      // 丢掉这个 run 的状态：下次 sync 不带它的 cursor，后端直接回快照重建。
      runs.delete(event.runId)
      void syncChatProtocol(event.conversationId)
      return
    }
    state.pending.set(event.seq, event)
    void syncChatProtocol(event.conversationId)
    return
  }
  // 先推进 lastSeq/terminal 再派发：即使派发路径出问题，序列也不会卡在原地
  // 把之后每一帧都判成 gap。
  let next: ChatRunEventEnvelope | undefined = event
  while (next) {
    advanceTerminalRevision(next)
    state.lastSeq = next.seq
    state.terminal = isTerminal(next)
    dispatch(next)
    if (state.terminal) return
    next = state.pending.get(state.lastSeq + 1)
    if (next) state.pending.delete(next.seq)
  }
}

function isContinuousReplay(
  conversationId: string,
  replay: Extract<ChatSyncResult['runs'][number], { kind: 'events' }>,
): boolean {
  const state = runs.get(replay.runId)
  const expectedFrom = (state?.lastSeq ?? 0) + 1
  if (replay.fromSeq !== expectedFrom) return false
  let expectedSeq = expectedFrom
  for (const event of replay.events) {
    if (
      event.conversationId !== conversationId
      || event.runId !== replay.runId
      || event.seq !== expectedSeq
    ) return false
    expectedSeq += 1
  }
  return replay.throughSeq === expectedSeq - 1
}

function isSemanticallyValidSnapshot(conversationId: string, snapshot: ChatRunSnapshot): boolean {
  const terminalType = snapshot.terminal?.type
  const terminalMatchesStatus = snapshot.status === 'running'
    ? snapshot.terminal === null
    : snapshot.status === 'completed'
      ? terminalType === 'run_completed'
      : snapshot.status === 'failed'
        ? terminalType === 'run_failed'
        : terminalType === 'run_cancelled'
  const recovery = snapshot.recovery
  const validRecovery = recovery === null || (
    recovery.groupId.length > 0
    && recovery.groupSize > 0
    && recovery.armIndex < recovery.groupSize
    && recovery.providerId.length > 0
    && recovery.model.length > 0
  )
  return snapshot.conversationId === conversationId
    && validRecovery
    && terminalMatchesStatus
    && snapshot.subagents.every((event) => event.type === 'subagent_updated')
    && (snapshot.compaction === null || snapshot.compaction.type === 'compaction_updated')
    && snapshot.pendingInteractions.every((event) => (
      event.type === 'session_consent_requested'
      || event.type === 'tool_approval_requested'
      || event.type === 'user_prompt_requested'
    ))
    && snapshot.warnings.every((event) => event.type === 'hook_failed')
}

function applyEvent(event: ChatProtocolEvent) {
  if (event.scope === 'conversation') {
    const current = conversationRevisions.get(event.conversationId) ?? 0
    if (event.revision < current) return
    conversationRevisions.set(event.conversationId, event.revision)
    dispatch(event)
    return
  }
  if (syncing.has(event.conversationId)) {
    const queue = liveDuringSync.get(event.conversationId) ?? []
    // 同 MAX_PENDING_EVENTS 那道护栏：sync 在飞的时候 live 事件改道进这里，
    // 一次快速回答几千个 delta 照样能把内存撑起来。攒满就整队丢掉 + 丢 run 状态，
    // sync 结束后第一个 live 事件必然判成 gap，走快照重建。
    if (queue.length >= MAX_PENDING_EVENTS) {
      liveDuringSync.delete(event.conversationId)
      for (const [runId, state] of runs) {
        if (state.conversationId === event.conversationId) runs.delete(runId)
      }
      return
    }
    queue.push(event)
    liveDuringSync.set(event.conversationId, queue)
    return
  }
  applyRunEvent(event)
}

function syntheticEnvelope(snapshot: ChatRunSnapshot, event: ChatRunEvent): ChatRunEventEnvelope {
  return {
    protocolVersion: CHAT_PROTOCOL_VERSION,
    scope: 'run',
    conversationId: snapshot.conversationId,
    runId: snapshot.runId,
    messageId: snapshot.messageId,
    seq: snapshot.lastSeq,
    baseRevision: snapshot.baseRevision,
    ...event,
  }
}

function applySnapshot(snapshot: ChatRunSnapshot) {
  const state: RunState = {
    conversationId: snapshot.conversationId,
    lastSeq: snapshot.lastSeq,
    terminal: snapshot.status !== 'running',
    pending: new Map(),
  }
  runs.set(snapshot.runId, state)
  const restored = { source: 'snapshot' as const }
  dispatch(syntheticEnvelope(snapshot, { type: 'run_started', recovery: snapshot.recovery }), restored)
  const orderedSegments = [...snapshot.segments].sort((left, right) => left.order - right.order)
  let restoredContent = ''
  let restoredReasoning = ''
  for (const segment of orderedSegments) {
    const delta = segment.text ?? ''
    if (segment.kind === 'reasoning') {
      restoredReasoning += delta
      dispatch(syntheticEnvelope(snapshot, { type: 'reasoning_delta', delta, segment }), restored)
    } else {
      if (segment.kind === 'text') restoredContent += delta
      dispatch(syntheticEnvelope(snapshot, { type: 'text_delta', delta: segment.kind === 'text' ? delta : '', segment }), restored)
    }
  }
  if (!restoredReasoning && snapshot.reasoning) {
    dispatch(syntheticEnvelope(snapshot, { type: 'reasoning_delta', delta: snapshot.reasoning, segment: null }), restored)
  } else if (snapshot.reasoning.startsWith(restoredReasoning) && snapshot.reasoning.length > restoredReasoning.length) {
    dispatch(syntheticEnvelope(snapshot, {
      type: 'reasoning_delta', delta: snapshot.reasoning.slice(restoredReasoning.length), segment: null,
    }), restored)
  }
  if (!restoredContent && snapshot.content) {
    dispatch(syntheticEnvelope(snapshot, { type: 'text_delta', delta: snapshot.content, segment: null }), restored)
  } else if (snapshot.content.startsWith(restoredContent) && snapshot.content.length > restoredContent.length) {
    dispatch(syntheticEnvelope(snapshot, {
      type: 'text_delta', delta: snapshot.content.slice(restoredContent.length), segment: null,
    }), restored)
  }
  for (const tool of snapshot.tools) {
    dispatch(syntheticEnvelope(snapshot, { type: 'tool_updated', tool }), restored)
  }
  if (snapshot.contextUsage) {
    dispatch(syntheticEnvelope(snapshot, { type: 'context_usage_updated', usage: snapshot.contextUsage }), restored)
  }
  for (const subagent of snapshot.subagents) {
    dispatch(syntheticEnvelope(snapshot, subagent), restored)
  }
  if (snapshot.compaction) {
    dispatch(syntheticEnvelope(snapshot, snapshot.compaction), restored)
  }
  if (snapshot.todoState) {
    dispatch(syntheticEnvelope(snapshot, { type: 'todo_updated', todoState: snapshot.todoState }), restored)
  }
  if (snapshot.planState) {
    dispatch(syntheticEnvelope(snapshot, { type: 'plan_updated', planState: snapshot.planState }), restored)
  }
  for (const pending of snapshot.pendingInteractions) {
    dispatch(syntheticEnvelope(snapshot, pending), restored)
  }
  for (const warning of snapshot.warnings) {
    dispatch(syntheticEnvelope(snapshot, warning), restored)
  }
  if (snapshot.terminal) dispatch(syntheticEnvelope(snapshot, snapshot.terminal), restored)
}

async function ensureListener() {
  nativeListener ??= listen<unknown>('chat-protocol', ({ payload }) => {
    if (typeof payload !== 'object' || payload === null) {
      reportIssue('invalid_event')
      return
    }
    const version = (payload as { protocolVersion?: unknown }).protocolVersion
    if (version !== CHAT_PROTOCOL_VERSION) {
      reportIssue('version_mismatch')
      return
    }
    if (!validateEvent(payload)) {
      console.error('Rejected invalid chat protocol event', validateEvent.errors, payload)
      const conversationId = (payload as { conversationId?: unknown }).conversationId
      reportIssue('invalid_event', typeof conversationId === 'string' ? conversationId : undefined)
      if (typeof conversationId === 'string') void syncChatProtocol(conversationId)
      return
    }
    applyEvent(payload as ChatProtocolEvent)
  })
  await nativeListener
}

export async function subscribeChatProtocol(subscriber: Subscriber) {
  subscribers.add(subscriber)
  await ensureListener()
  return () => subscribers.delete(subscriber)
}

export function subscribeChatProtocolIssues(
  subscriber: (issue: ChatProtocolIssue, conversationId?: string) => void,
) {
  issueSubscribers.add(subscriber)
  return () => issueSubscribers.delete(subscriber)
}

export async function syncChatProtocol(conversationId: string) {
  if (!conversationId || syncing.has(conversationId)) return
  syncing.add(conversationId)
  let acceptedSync = false
  try {
    await ensureListener()
    // 只给还在跑的 run 带 cursor：后端 5 分钟后会剪掉已完成的 run，
    // 再带着它们的 cursor 去 sync 只会被算进 missingRunIds，白触发一次全量 reload。
    // run 的状态本身要留着（快照恢复/去重还要用），所以是不带 cursor，不是删。
    const cursors = [...runs.entries()]
      .filter(([, state]) => state.conversationId === conversationId && !state.terminal)
      .map(([runId, state]) => ({ runId, lastSeq: state.lastSeq }))
    const result = await invoke<unknown>('chat_sync_state', {
      request: { protocolVersion: CHAT_PROTOCOL_VERSION, conversationId, cursors },
    })
    const resultVersion = typeof result === 'object' && result !== null
      ? (result as { protocolVersion?: unknown }).protocolVersion
      : undefined
    if (resultVersion !== CHAT_PROTOCOL_VERSION) {
      reportIssue('version_mismatch')
      return
    }
    if (!validateSync(result)) {
      console.error('Rejected invalid chat protocol sync result', validateSync.errors, result)
      reportIssue('invalid_event')
      retrySync(conversationId)
      return
    }
    const validated = result as ChatSyncResult
    const invalidRun = validated.runs.find((run) => (
      run.kind === 'snapshot'
        ? !isSemanticallyValidSnapshot(conversationId, run.snapshot)
        : !isContinuousReplay(conversationId, run)
    ))
    if (invalidRun) {
      console.error('Rejected invalid chat protocol sync run', invalidRun)
      reportIssue('invalid_event', conversationId)
      retrySync(conversationId)
      return
    }
    acceptedSync = true
    // 本窗口没见过这个会话时，直接把后端 revision 收下当基线。
    // 任何有内容的会话 revision 都 > 0，把「没见过」当「落后」会导致
    // 第一次切进任何会话都必定触发一次多余的全量 reload。
    const knownRevision = conversationRevisions.get(conversationId)
    if (knownRevision !== undefined && validated.conversationRevision > knownRevision) {
      reportIssue('resync_required', conversationId)
    }
    conversationRevisions.set(
      conversationId,
      Math.max(knownRevision ?? 0, validated.conversationRevision),
    )
    if (validated.missingRunIds.length > 0) {
      for (const runId of validated.missingRunIds) runs.delete(runId)
      reportIssue('resync_required', conversationId)
    }
    for (const run of validated.runs) {
      if (run.kind === 'snapshot') {
        applySnapshot(run.snapshot)
      } else {
        for (const event of run.events) applyRunEvent(event)
      }
    }
  } catch (error) {
    if (String(error).toLowerCase().includes('protocol version mismatch')) {
      reportIssue('version_mismatch')
      return
    }
    console.error('Failed to synchronize chat protocol state', error)
    retrySync(conversationId)
  } finally {
    syncing.delete(conversationId)
    const queued = liveDuringSync.get(conversationId) ?? []
    liveDuringSync.delete(conversationId)
    queued.sort((left, right) => left.seq - right.seq)
    for (const event of queued) applyRunEvent(event)
    const unresolvedGap = [...runs.values()].some(
      (state) => state.conversationId === conversationId && state.pending.size > 0 && !state.terminal,
    )
    if (acceptedSync && unresolvedGap) retrySync(conversationId)
    else if (acceptedSync) syncRetryAttempts.delete(conversationId)
  }
}

export const chatProtocolTesting = {
  reset() {
    runs.clear()
    syncing.clear()
    liveDuringSync.clear()
    conversationRevisions.clear()
    syncRetryAttempts.clear()
    for (const timer of syncRetryTimers.values()) clearTimeout(timer)
    syncRetryTimers.clear()
    subscribers.clear()
    issueSubscribers.clear()
  },
  subscribe(subscriber: Subscriber) {
    subscribers.add(subscriber)
    return () => subscribers.delete(subscriber)
  },
  ingest(event: ChatProtocolEvent) {
    applyEvent(event)
  },
  applySnapshot(snapshot: ChatRunSnapshot) {
    applySnapshot(snapshot)
  },
  validate(payload: unknown) {
    return validateEvent(payload)
  },
  validateSync(payload: unknown) {
    return validateSync(payload)
  },
  isContinuousReplay,
  isSemanticallyValidSnapshot,
}
