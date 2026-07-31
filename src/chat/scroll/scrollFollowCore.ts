// 底部钉住的滚动跟随核心。
//
// 架构对齐 Paseo web（strategy-web.tsx）+ 部分虚拟化（近底实挂载）：
// - 只在明确用户输入时解除跟随（滚轮上滚、触摸拖动、历史键、指针拖动）。
// - 重新跟随：仅当用户向下滚且已贴物理底（小阈值），不从 192px 外硬拽。
// - 跟随中的内容增长 → pin；跟随中的滚动噪声 gap 不 pin（避免与列表 remeasure 互抢）。
// - ResizeObserver（contentGrowth）不改变跟随状态，只在跟随时钉底。
//
// 纯函数、无 DOM：useScrollFollow 收集事实并执行 pin。
// "release"：消息导航器跳到上方时主动脱离，否则纠正器会钉回底部。

// "在底部"的容差。分数 devicePixelRatio 会把 scrollTop 留在 max 前 1–3px。
export const BOTTOM_ATTACH_THRESHOLD_PX = 12

// 拖滚动条到底部附近松手 → 恢复跟随。**只用于 pointerRelease**：滚动路径已改成只认真正贴底
// （大 reattach 区在滚动路径上会从 192px 外硬拽，是底部抽搐主因之一）。
export const POINTER_RELEASE_ZONE_PX = 192

// gap 在此 slop 内的抖动是布局噪声，不算滚动方向。
export const DIRECTION_SLOP_PX = 1

// 跟随中「滚动纠正再钉底」的最小 gap。更小的是测量噪声。
export const CORRECTION_MIN_PX = 32

export const POINTER_DRAG_SLOP_PX = 4

export const GESTURE_LATCH_MS = 500

export type FollowConfig = {
  attachThresholdPx: number
  releaseZonePx: number
  directionSlopPx: number
  correctionMinPx: number
  latchMs: number
}

export const DEFAULT_FOLLOW_CONFIG: FollowConfig = {
  attachThresholdPx: BOTTOM_ATTACH_THRESHOLD_PX,
  releaseZonePx: POINTER_RELEASE_ZONE_PX,
  directionSlopPx: DIRECTION_SLOP_PX,
  correctionMinPx: CORRECTION_MIN_PX,
  latchMs: GESTURE_LATCH_MS,
}

export type FollowState = {
  following: boolean
  pointerHeld: boolean
  pointerDragging: boolean
  dragTowardBottom: boolean | null
  latchUntil: number
  lastGap: number
}

export function createFollowState(): FollowState {
  return {
    following: true,
    pointerHeld: false,
    pointerDragging: false,
    dragTowardBottom: null,
    latchUntil: 0,
    lastGap: 0,
  }
}

export type FollowEvent =
  | {
      type: 'wheel'
      deltaX: number
      deltaY: number
      gap: number
      hasOverflow: boolean
      nestedCanConsume: boolean
      now: number
    }
  | {
      type: 'touchMove'
      fingerMovedDown: boolean | null
      gap: number
      hasOverflow: boolean
      now: number
    }
  | { type: 'scroll'; gap: number; now: number }
  | { type: 'pointerDown' }
  | { type: 'pointerDragStart' }
  | { type: 'pointerRelease'; gap: number }
  | { type: 'historyKey'; hasOverflow: boolean; now: number }
  | { type: 'followKey'; now: number }
  | { type: 'contentGrowth'; gap: number }
  | { type: 'forceFollow' }
  | { type: 'release' }

export type FollowStep = {
  state: FollowState
  // hook 的副作用：立即 scrollTop = scrollHeight。
  pin: boolean
}

export function isAtBottom(gap: number, config: FollowConfig = DEFAULT_FOLLOW_CONFIG) {
  return gap <= config.attachThresholdPx
}

// 触控板横向平移（宽代码块、表格）每帧带几 px 纵向漂移；只有以纵向为主的手势才能改变跟随状态。
export function isDominantVerticalWheel(deltaX: number, deltaY: number) {
  return Math.abs(deltaY) > Math.abs(deltaX)
}

export function reduceFollowEvent(
  state: FollowState,
  event: FollowEvent,
  config: FollowConfig = DEFAULT_FOLLOW_CONFIG,
): FollowStep {
  switch (event.type) {
    case 'wheel': {
      if (!isDominantVerticalWheel(event.deltaX, event.deltaY)) {
        return { state, pin: false }
      }
      if (event.deltaY < 0) {
        const next = { ...state, latchUntil: 0 }
        if (event.hasOverflow && !event.nestedCanConsume) {
          next.following = false
        }
        return { state: next, pin: false }
      }
      const next = { ...state, latchUntil: event.now + config.latchMs }
      if (!state.following && isAtBottom(event.gap, config)) {
        next.following = true
        return { state: next, pin: true }
      }
      return { state: next, pin: false }
    }

    case 'touchMove': {
      const movedAway = event.fingerMovedDown !== false
      const next = {
        ...state,
        latchUntil: movedAway ? 0 : event.now + config.latchMs,
      }
      if (event.hasOverflow && (movedAway || event.gap > config.attachThresholdPx)) {
        next.following = false
      }
      return { state: next, pin: false }
    }

    case 'scroll': {
      const { gap, now } = event
      const previousGap = state.lastGap
      const next = { ...state, lastGap: gap }

      if (isAtBottom(gap, config)) {
        next.dragTowardBottom = true
        if (state.following || now <= state.latchUntil) {
          next.following = true
        }
        return { state: next, pin: false }
      }

      const movedAway = gap > previousGap + config.directionSlopPx
      const movedTowardBottom = gap < previousGap - config.directionSlopPx

      if (state.pointerDragging && movedAway) {
        next.following = false
        next.dragTowardBottom = false
        return { state: next, pin: false }
      }

      if (state.following) {
        // 显著 gap 才 pin；小 gap 交给 contentGrowth / 下一帧，避免与 remeasure 互抢。
        return { state: next, pin: gap > config.correctionMinPx }
      }

      if (movedTowardBottom) {
        next.dragTowardBottom = true
        if (now <= state.latchUntil) {
          next.latchUntil = now + config.latchMs
        }
        // Paseo：仅「向下滚且已贴底」才重跟随；不 pin（由 contentGrowth / forceFollow 钉）。
        if (!state.pointerHeld && isAtBottom(gap, config)) {
          next.following = true
        }
      } else if (movedAway) {
        next.dragTowardBottom = false
      }
      return { state: next, pin: false }
    }

    case 'pointerDown': {
      return { state: { ...state, pointerHeld: true, dragTowardBottom: null }, pin: false }
    }

    case 'pointerDragStart': {
      if (!state.pointerHeld) {
        return { state, pin: false }
      }
      return { state: { ...state, pointerDragging: true }, pin: false }
    }

    case 'pointerRelease': {
      if (!state.pointerHeld) {
        return { state, pin: false }
      }
      const next = {
        ...state,
        pointerHeld: false,
        pointerDragging: false,
        dragTowardBottom: null,
      }
      const releaseZonePx = Math.max(config.releaseZonePx, config.attachThresholdPx)
      if (state.dragTowardBottom === true && event.gap <= releaseZonePx) {
        next.following = true
        return { state: next, pin: true }
      }
      return { state: next, pin: false }
    }

    case 'historyKey': {
      const next = { ...state, latchUntil: 0 }
      if (event.hasOverflow) {
        next.following = false
      }
      return { state: next, pin: false }
    }

    case 'followKey': {
      return { state: { ...state, latchUntil: event.now + config.latchMs }, pin: false }
    }

    case 'contentGrowth': {
      return { state: { ...state, lastGap: event.gap }, pin: state.following }
    }

    case 'forceFollow': {
      return {
        state: {
          ...state,
          following: true,
          pointerDragging: false,
          dragTowardBottom: null,
          latchUntil: 0,
        },
        pin: true,
      }
    }

    case 'release': {
      // Kivio 新增：主动脱离跟随（消息导航器跳转到上方消息时用），不钉底。
      return { state: { ...state, following: false, latchUntil: 0 }, pin: false }
    }
  }
}
