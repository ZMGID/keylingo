import { useSyncExternalStore } from 'react'
import { createEmptyStreamSnapshot, type ConversationStreamSnapshot } from './conversationRuns'

// 多模型一问多答（任务 06-30）：单会话的「单条流」预览仍走 streamingStore（零回归）；
// 当一条 user 消息 fan-out 出 N 条并发 assistant 流时，N 条流靠后端事件里的 messageId 区分，
// 在这里按 messageId 二级聚合。MessageGroup 组件订阅本 store 渲染并排多列的实时流。
//
// 设计要点：
// - 键是 messageId（后端为每个臂生成独立 message_id/run_id），而非 conversationId。
// - 每个会话维护一个「活跃组」(activeGroup)：发送多模型问题时建组、登记期望列数与共享 group_id，
//   收到属于该会话的、未知 messageId 的流事件时按需建列。组结束（sendMessage 返回）时清掉。
// - 与 streamingStore 完全独立：单模型路径一行代码都不碰本 store。

export interface GroupColumnSnapshot extends ConversationStreamSnapshot {
  messageId: string
  providerId: string | null
  model: string | null
}

export interface ActiveGroupState {
  conversationId: string
  groupId: string
  // 期望的列数（= reply_models 数量），用于发送时先占位 N 个「思考中」骨架列。
  expectedColumns: number
  // 已知的列，按 messageId 收敛；首批为占位列（无 messageId），随流事件填充真实 messageId。
  columns: GroupColumnSnapshot[]
}

// conversationId → 活跃组。一个会话同一时刻最多一个活跃多答组（与「发送时 busy 拒绝」一致）。
const activeGroups = new Map<string, ActiveGroupState>()

const subs = new Set<() => void>()
const conversationSubs = new Map<string, Set<() => void>>()
const conversationVersions = new Map<string, number>()

// 版本号：任何变更都自增，作为 getSnapshot 的稳定标识，避免每帧分配新对象。
let version = 0

// 结构性变更（建组/认领列/结束组）立即 emit；内容 delta（touchGroup）经 rAF 合帧，
// 避免 N 列各自高频 setState（与单流 showStreamSnapshotIfCurrent 的合帧策略一致）。
function emit(conversationId?: string) {
  version += 1
  if (conversationId) {
    conversationVersions.set(conversationId, (conversationVersions.get(conversationId) ?? 0) + 1)
    for (const cb of conversationSubs.get(conversationId) ?? []) cb()
  }
  for (const cb of subs) cb()
}

// ---- 流式 delta 合帧（性能：任务 06-30 步骤 8 / R10）----
// 多答组流式时，每条流的每个 delta 都会 mutate 对应列并请求一次重渲。N 列并发下若每个
// delta 立即 emit()，订阅者（MessageGroup × N + MessageList）会被每秒成百次 setState 打爆。
// 这里把 touchGroup 的 emit 节流到每帧最多一次：delta 仍即时累积到列对象，只把「通知重渲」
// 合帧。done/结束等终止帧用 flushGroups 立即 flush。
let groupFlushRaf: number | null = null
let groupFlushPending = false

// 延迟解析全局 rAF（而非模块加载时绑定），让测试能 stub requestAnimationFrame；
// 无 rAF 环境（部分测试/SSR）降级为 0ms 宏任务，仍保留合帧语义（同步累积、异步通知）。
function rafSchedule(cb: () => void): number {
  if (typeof requestAnimationFrame === 'function') return requestAnimationFrame(cb)
  // ponytail: 无 rAF 时退化为 setTimeout，返回值统一当作不透明 handle。
  return setTimeout(cb, 0) as unknown as number
}

function rafCancel(handle: number): void {
  if (typeof cancelAnimationFrame === 'function') {
    cancelAnimationFrame(handle)
    return
  }
  clearTimeout(handle)
}

export function subscribeGroups(cb: () => void): () => void {
  subs.add(cb)
  return () => {
    subs.delete(cb)
  }
}

export function getGroupsVersion(): number {
  return version
}

function makeColumn(
  messageId: string,
  providerId: string | null,
  model: string | null,
): GroupColumnSnapshot {
  return {
    ...createEmptyStreamSnapshot(),
    messageId,
    providerId,
    model,
  }
}

export type GroupArmSeed = {
  providerId: string | null
  model: string | null
  messageId?: string
  streaming?: boolean
  content?: string
  reasoning?: string
  toolCalls?: import('./types').ToolCallRecord[]
  segments?: import('./types').ChatMessageSegment[]
}

/** 起一个多答组：登记会话、group_id、期望列数（先放 N 个占位骨架列）。 */
export function beginGroup(
  conversationId: string,
  groupId: string,
  arms: GroupArmSeed[],
): void {
  const columns = arms.map((arm, index) => {
    const column = makeColumn(
      arm.messageId || `pending-${groupId}-${index}`,
      arm.providerId,
      arm.model,
    )
    if (arm.streaming === false) column.streaming = false
    if (arm.content) column.content = arm.content
    if (arm.reasoning) column.reasoning = arm.reasoning
    if (arm.toolCalls) column.toolCalls = arm.toolCalls
    if (arm.segments) column.segments = arm.segments
    return column
  })
  activeGroups.set(conversationId, {
    conversationId,
    groupId,
    expectedColumns: arms.length,
    columns,
  })
  emit(conversationId)
}

/** Rebuild one fan-out arm from protocol recovery metadata after a window reload. */
export function restoreGroupArm(
  conversationId: string,
  groupId: string,
  groupSize: number,
  armIndex: number,
  messageId: string,
  providerId: string,
  model: string,
): GroupColumnSnapshot {
  let group = activeGroups.get(conversationId)
  if (!group || group.groupId !== groupId) {
    const expectedColumns = Math.max(1, groupSize)
    group = {
      conversationId,
      groupId,
      expectedColumns,
      columns: Array.from({ length: expectedColumns }, (_, index) => (
        makeColumn(`pending-${groupId}-${index}`, null, null)
      )),
    }
    activeGroups.set(conversationId, group)
  }
  const index = Math.min(Math.max(0, armIndex), group.columns.length - 1)
  const column = group.columns[index]
  column.messageId = messageId
  column.providerId = providerId
  column.model = model
  emit(conversationId)
  return column
}

/** 该会话当前是否处于多答组流式中。 */
export function hasActiveGroup(conversationId: string): boolean {
  return activeGroups.has(conversationId)
}

export function getActiveGroup(conversationId: string): ActiveGroupState | undefined {
  return activeGroups.get(conversationId)
}

/**
 * 为某会话的活跃组认领/获取一条列快照（按 messageId）。
 * 若该 messageId 未知：优先认领一个尚未绑定真实 id 的占位列（pending-*），否则新建一列。
 * 返回被原地 mutate 的列对象（调用方累积 delta 后调用 touchGroup 触发重渲）。
 */
export function ensureGroupColumn(
  conversationId: string,
  messageId: string,
  providerId?: string | null,
  model?: string | null,
): GroupColumnSnapshot | null {
  const group = activeGroups.get(conversationId)
  if (!group) return null
  const existing = group.columns.find((col) => col.messageId === messageId)
  if (existing) {
    if (providerId != null && existing.providerId == null) existing.providerId = providerId
    if (model != null && existing.model == null) existing.model = model
    return existing
  }
  // 认领一个还没绑真实 message_id 的占位列。
  const pending = group.columns.find((col) => col.messageId.startsWith(`pending-${group.groupId}-`))
  if (pending) {
    pending.messageId = messageId
    if (providerId != null) pending.providerId = providerId
    if (model != null) pending.model = model
    return pending
  }
  // 没有占位列可认领（实际臂数 > 期望，理论上不会发生）：直接追加一列。
  const col = makeColumn(messageId, providerId ?? null, model ?? null)
  group.columns.push(col)
  return col
}

/** 列内容被原地 mutate 后，请求一次重渲（rAF 合帧：每帧最多通知订阅者一次）。 */
const pendingConversationIds = new Set<string>()

export function touchGroup(conversationId?: string): void {
  if (conversationId) pendingConversationIds.add(conversationId)
  groupFlushPending = true
  if (groupFlushRaf != null) return
  groupFlushRaf = rafSchedule(() => {
    groupFlushRaf = null
    if (!groupFlushPending) return
    groupFlushPending = false
    if (pendingConversationIds.size === 0) emit()
    else {
      for (const id of pendingConversationIds) emit(id)
      pendingConversationIds.clear()
    }
  })
}

/** 立即 flush 待合帧的内容更新（done / 结束 / 测试需要同步可见时调用）。 */
export function flushGroups(conversationId?: string): void {
  if (groupFlushRaf != null) {
    rafCancel(groupFlushRaf)
    groupFlushRaf = null
  }
  if (!groupFlushPending) return
  groupFlushPending = false
  if (conversationId) pendingConversationIds.add(conversationId)
  if (pendingConversationIds.size === 0) emit()
  else {
    for (const id of pendingConversationIds) emit(id)
    pendingConversationIds.clear()
  }
}

/** 结束并清掉某会话的活跃组（sendMessage 返回 / 错误 / 取消时调用）。 */
export function endGroup(conversationId: string): void {
  flushGroups(conversationId)
  if (activeGroups.delete(conversationId)) {
    emit(conversationId)
  }
}

/** 清空所有活跃组（卸载 / 全局重置）。 */
export function resetGroups(): void {
  if (groupFlushRaf != null) {
    rafCancel(groupFlushRaf)
    groupFlushRaf = null
  }
  groupFlushPending = false
  if (activeGroups.size > 0) {
    activeGroups.clear()
    pendingConversationIds.clear()
    emit()
  }
}

// ---- React hook ----

// 订阅版本号即可：组件读 getActiveGroup(conversationId) 取最新列（mutate 的同一引用），
// 版本号变化驱动重渲。MessageGroup 内部据列内容渲染，无需快照拷贝。
export function useGroupsVersion(): number {
  return useSyncExternalStore(subscribeGroups, getGroupsVersion, getGroupsVersion)
}

export function subscribeGroup(conversationId: string, cb: () => void): () => void {
  const listeners = conversationSubs.get(conversationId) ?? new Set<() => void>()
  listeners.add(cb)
  conversationSubs.set(conversationId, listeners)
  return () => {
    listeners.delete(cb)
    if (listeners.size === 0) conversationSubs.delete(conversationId)
  }
}

export function getGroupVersion(conversationId: string): number {
  return conversationVersions.get(conversationId) ?? 0
}

export function useGroupVersion(conversationId: string | null | undefined): number {
  const key = conversationId ?? ''
  return useSyncExternalStore(
    (cb) => subscribeGroup(key, cb),
    () => getGroupVersion(key),
    () => getGroupVersion(key),
  )
}
