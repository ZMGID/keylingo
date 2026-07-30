import type { ConversationStreamSnapshot } from './conversationRuns'
import type { ChatSessionConsentPayload, ChatToolConfirmPayload } from '../api/tauri'

/**
 * 一个会话在前端持有的全部本地运行态。
 *
 * 这些字段原本是 Chat.tsx 里 6 个独立的 ref。它们不是 6 件事 —— 「清理一个会话」
 * 必须同时动其中若干个，Chat.tsx 里因此出现了 6 处手写的删除块，字段组合各不相同
 * （[CST] 三处、[CDEFST]、[CEST]、[CT]），差异全靠人记。
 *
 * 这里只把「清理」这个动作收敛成显式的谓词，ref 本身仍留在 Chat.tsx ——
 * 读取侧有 30 处、语义各异（判 busy / 取快照 / 恢复预览），打包进来只会变成
 * 30 个跨模块调用。
 */
export interface ConversationLocalState {
  /** 已 invoke 但尚未收到终局的会话。 */
  inFlight: Set<string>
  /** invoke 未返回前被延后的 done 处理。 */
  pendingStreamDone: Record<string, () => Promise<void>>
  streamSnapshots: Record<string, ConversationStreamSnapshot>
  streamErrors: Record<string, string>
  /**
   * 每会话一条待审批队列（不是单个）。claude 会在一条消息里并行调多个工具，
   * 后端也是按 request_id 并发挂着等的；这里若只留一个槽位，第二条询问会覆盖第一条，
   * 用户没看见的那条会在后端超时后被判成「用户拒绝」。
   */
  pendingToolConfirms: Record<string, ChatToolConfirmPayload[]>
  pendingSessionConsents: Record<string, ChatSessionConsentPayload>
}

/** 清理时可选择动哪些字段。默认只清「一轮结束」必然要清的三项。 */
export interface ClearScope {
  /** 同时移出 in-flight 集合（会话被删除/丢弃时）。 */
  inFlight?: boolean
  /** 同时丢弃被延后的 done 处理（会话被删除时；正常结束要保留以便 flush）。 */
  pendingStreamDone?: boolean
  /** 同时清错误（会话被删除时；正常结束要保留以便展示失败原因）。 */
  streamErrors?: boolean
}

/**
 * 清掉一个会话的本地运行态。
 *
 * 无条件清理的三项 —— 快照、待确认工具、待确认会话授权 —— 是「这一轮跑完了」
 * 的定义，6 处调用点全都要清。可选项按场景开启，见 ClearScope 各字段注释。
 */
export function clearConversationLocalState(
  state: ConversationLocalState,
  conversationId: string,
  scope: ClearScope = {},
): void {
  delete state.streamSnapshots[conversationId]
  delete state.pendingToolConfirms[conversationId]
  delete state.pendingSessionConsents[conversationId]
  if (scope.inFlight) state.inFlight.delete(conversationId)
  if (scope.pendingStreamDone) delete state.pendingStreamDone[conversationId]
  if (scope.streamErrors) delete state.streamErrors[conversationId]
}
