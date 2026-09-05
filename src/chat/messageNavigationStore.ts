import { useSyncExternalStore } from 'react'

/**
 * 列表短生命周期「强制 eager hydrate」状态。
 *
 * 用于：
 * - 右侧消息导航跳转 settle
 * - 回到底部 settle
 * - 流式结束 → 历史气泡首挂（否则 DeferredCodeBlock 180ms 延迟会再抽一下）
 *
 * 模块级 flag 可被 useState 初始化器同步读到，避免「先挂载、后 React 订阅到 eager」的竞态。
 */
let eagerHydrate = false
let settleGeneration = 0
let timedEndTimer: ReturnType<typeof setTimeout> | null = null

const listeners = new Set<() => void>()

function emit() {
  for (const listener of listeners) listener()
}

function clearTimedEndTimer() {
  if (timedEndTimer == null) return
  clearTimeout(timedEndTimer)
  timedEndTimer = null
}

export function isMessageNavigationEagerHydrate(): boolean {
  return eagerHydrate
}

export function beginMessageNavigationHydrate(): number {
  clearTimedEndTimer()
  settleGeneration += 1
  if (!eagerHydrate) {
    eagerHydrate = true
    emit()
  }
  return settleGeneration
}

/**
 * 流式 live → 历史气泡 的短窗 eager。
 * 在结束交接的 layout effect 中开启，保证内容在绘制前完成 hydrate。
 */
export const STREAM_SETTLE_EAGER_MS = 800

export function beginStreamSettleEagerHydrate(
  durationMs: number = STREAM_SETTLE_EAGER_MS,
): number {
  const generation = beginMessageNavigationHydrate()
  clearTimedEndTimer()
  timedEndTimer = setTimeout(() => {
    timedEndTimer = null
    endMessageNavigationHydrate(generation)
  }, durationMs)
  return generation
}

export function endMessageNavigationHydrate(generation: number): void {
  if (generation !== settleGeneration) return
  clearTimedEndTimer()
  if (!eagerHydrate) return
  eagerHydrate = false
  emit()
}

export function getMessageNavigationEagerHydrate(): boolean {
  return eagerHydrate
}

export function useMessageNavigationEagerHydrate(): boolean {
  return useSyncExternalStore(
    (listener) => {
      listeners.add(listener)
      return () => listeners.delete(listener)
    },
    getMessageNavigationEagerHydrate,
    getMessageNavigationEagerHydrate,
  )
}

/** 测试 / 会话切换时清掉残留 settle。 */
export function resetMessageNavigationStore(): void {
  clearTimedEndTimer()
  settleGeneration += 1
  if (!eagerHydrate) return
  eagerHydrate = false
  emit()
}
