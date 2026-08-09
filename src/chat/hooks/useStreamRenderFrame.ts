import { useCallback, useEffect, useRef } from 'react'
import type { ConversationStreamSnapshot } from '../conversationRuns'

interface UseStreamRenderFrameParams {
  /** 把快照真正写进 state。 */
  applySnapshot: (snapshot: ConversationStreamSnapshot) => void
  /** 读当前会话 id：只有快照属于当前会话才渲染，避免串会话。 */
  currentConversationIdRef: React.MutableRefObject<string | null>
}

function streamRenderInterval(snapshot: ConversationStreamSnapshot): number {
  // Tests deliberately keep the historical rAF-only semantics. In production,
  // growing Markdown benefits more from a stable 32–220ms cadence than from
  // parsing once per display frame.
  if (import.meta.env.MODE === 'test') return 0
  const contentSize = snapshot.content.length + snapshot.reasoning.length
  const structuredSize = snapshot.toolCalls.length * 512 + snapshot.segments.length * 64
  const totalSize = contentSize + structuredSize
  if (totalSize >= 250_000) return 220
  if (totalSize >= 120_000) return 180
  if (totalSize >= 60_000) return 140
  if (snapshot.toolCalls.length > 0 || snapshot.segments.length > 8) return 120
  if (totalSize >= 12_000) return 80
  return 32
}

/**
 * 流式渲染合帧。
 *
 * 事件本身仍即时累积到 snapshot 对象，这里只把「渲染」节流到每帧一次 ——
 * token 级 setState 会让长回复卡顿。
 *
 * 两个 ref（挂起帧 / rAF 句柄）只服务这一件事，故整体搬出。搬迁前「取消挂起帧」
 * 的逻辑在 5 处内联重复，这里收敛成 cancelPendingFrame 一个出口。
 */
export function useStreamRenderFrame({
  applySnapshot,
  currentConversationIdRef,
}: UseStreamRenderFrameParams) {
  const pendingRef = useRef<{ conversationId: string; snapshot: ConversationStreamSnapshot } | null>(null)
  const rafRef = useRef<number | null>(null)
  const timerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const lastFlushAtRef = useRef(0)

  const cancelScheduledFrame = useCallback(() => {
    if (rafRef.current != null) {
      cancelAnimationFrame(rafRef.current)
      rafRef.current = null
    }
    if (timerRef.current != null) {
      clearTimeout(timerRef.current)
      timerRef.current = null
    }
  }, [])

  /** 取消挂起帧且不应用（切换会话/剔除 ghost/清空预览时用）。 */
  const cancelPendingFrame = useCallback(() => {
    cancelScheduledFrame()
    pendingRef.current = null
  }, [cancelScheduledFrame])

  /** 只在挂起帧属于指定会话时取消（剔除某个 ghost 会话时用）。 */
  const cancelPendingFrameFor = useCallback((conversationId: string) => {
    if (pendingRef.current?.conversationId === conversationId) {
      cancelPendingFrame()
    }
  }, [cancelPendingFrame])

  /** 立即把挂起帧刷出去（done/结束、卸载、切换会话前调用），保证不丢最后一帧。 */
  const flushStreamRender = useCallback(() => {
    cancelScheduledFrame()
    const pending = pendingRef.current
    pendingRef.current = null
    if (!pending) return
    if (currentConversationIdRef.current !== pending.conversationId) return
    lastFlushAtRef.current = performance.now()
    applySnapshot(pending.snapshot)
  }, [applySnapshot, cancelScheduledFrame, currentConversationIdRef])

  const schedulePendingFrame = useCallback(() => {
    if (rafRef.current != null || timerRef.current != null) return
    const pending = pendingRef.current
    if (!pending) return
    const wait = Math.max(0, streamRenderInterval(pending.snapshot) - (
      performance.now() - lastFlushAtRef.current
    ))
    if (wait > 0) {
      timerRef.current = setTimeout(() => {
        timerRef.current = null
        schedulePendingFrame()
      }, wait)
      return
    }
    rafRef.current = requestAnimationFrame(() => {
      rafRef.current = null
      const next = pendingRef.current
      pendingRef.current = null
      if (!next) return
      if (currentConversationIdRef.current !== next.conversationId) return
      lastFlushAtRef.current = performance.now()
      applySnapshot(next.snapshot)
    })
  }, [applySnapshot, currentConversationIdRef])

  /**
   * 合帧入口。immediate=true（done 等终止帧）立即 flush，不再等下一帧。
   */
  const showStreamSnapshotIfCurrent = useCallback((
    conversationId: string,
    snapshot: ConversationStreamSnapshot,
    immediate = false,
  ) => {
    if (currentConversationIdRef.current !== conversationId) return
    pendingRef.current = { conversationId, snapshot }
    if (immediate) {
      flushStreamRender()
      return
    }
    schedulePendingFrame()
  }, [currentConversationIdRef, flushStreamRender, schedulePendingFrame])

  // 卸载时取消挂起帧，避免 rAF 回调在组件消失后仍 setState。
  useEffect(() => () => {
    cancelScheduledFrame()
  }, [cancelScheduledFrame])

  return {
    cancelPendingFrame,
    cancelPendingFrameFor,
    flushStreamRender,
    showStreamSnapshotIfCurrent,
  }
}
