import { useSyncExternalStore } from 'react'

/**
 * 右侧消息导航跳转时的短生命周期状态。
 *
 * 跳到虚拟列表尚未挂载的行时，目标气泡会在 scrollToIndex 之后才挂上；
 * 若 ChatHeavyIsland 仍按 idle/delay 延迟 hydrate，占位高度 → 真高度 的二次
 * 纠正会让视口「抽一下」。会话切换已有 conversationOpening 强制 eager 路径，
 * 导航跳转复用同一语义：settle 期间全局 eager hydrate，落稳后再关掉。
 *
 * 模块级 flag 可被 useState 初始化器同步读到，避免「先 scroll 挂载、后 React
 * 订阅到 eager」的竞态。
 */
let eagerHydrate = false
let settleGeneration = 0

const listeners = new Set<() => void>()

function emit() {
  for (const listener of listeners) listener()
}

export function isMessageNavigationEagerHydrate(): boolean {
  return eagerHydrate
}

export function beginMessageNavigationHydrate(): number {
  settleGeneration += 1
  if (!eagerHydrate) {
    eagerHydrate = true
    emit()
  }
  return settleGeneration
}

export function endMessageNavigationHydrate(generation: number): void {
  if (generation !== settleGeneration) return
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
  settleGeneration += 1
  if (!eagerHydrate) return
  eagerHydrate = false
  emit()
}
