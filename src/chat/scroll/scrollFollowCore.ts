// 底部钉住的滚动跟随核心。
//
// 架构对齐 Paseo web（strategy-web.tsx）+ 部分虚拟化（近底实挂载）：
// - 只在明确用户输入时解除跟随（滚轮上滚、触摸拖动、历史键、外部把视口拉离底部）。
// - 重新跟随：仅当用户向下滚且已贴物理底（小阈值），不从 192px 外硬拽。
// - 跟随中的内容增长 → pin；跟随中的滚动噪声 gap 不 pin（避免与列表 remeasure 互抢）。
// - ResizeObserver（contentGrowth）不改变跟随状态，只在跟随时钉底。
//
// **滚动事件必须带来源**（`source`）。参考 stackblitz-labs/use-stick-to-bottom 的
// `ignoreScrollToTop` / `resizeDifference` 两个机制：分不清「这次 scrollTop 是我自己写的」
// 和「外部把视口拉走了」，就会在拖原生滚动条 / 页内查找 / iframe 滚动链的时候和外部
// 反复互写 scrollTop —— 表现是贴底时整个列表抽搐、拖不动。来源由 useScrollFollow 判定。
//
// 纯函数、无 DOM：useScrollFollow 收集事实并执行 pin。
// "release"：消息导航器跳到上方时主动脱离，否则纠正器会钉回底部。

// "在底部"的容差。分数 devicePixelRatio 会把 scrollTop 留在 max 前 1–3px。
export const BOTTOM_ATTACH_THRESHOLD_PX = 12

// gap 在此 slop 内的抖动是布局噪声，不算滚动方向。
export const DIRECTION_SLOP_PX = 1

// 跟随中「滚动纠正再钉底」的最小 gap。更小的是测量噪声。
export const CORRECTION_MIN_PX = 32

export const GESTURE_LATCH_MS = 500

export type FollowConfig = {
  attachThresholdPx: number
  directionSlopPx: number
  correctionMinPx: number
  latchMs: number
}

export const DEFAULT_FOLLOW_CONFIG: FollowConfig = {
  attachThresholdPx: BOTTOM_ATTACH_THRESHOLD_PX,
  directionSlopPx: DIRECTION_SLOP_PX,
  correctionMinPx: CORRECTION_MIN_PX,
  latchMs: GESTURE_LATCH_MS,
}

export type FollowState = {
  following: boolean
  pointerHeld: boolean
  latchUntil: number
  lastGap: number
}

export function createFollowState(): FollowState {
  return {
    following: true,
    pointerHeld: false,
    latchUntil: 0,
    lastGap: 0,
  }
}

/**
 * 滚动事件的来源。
 * - `self`：本 hook 自己写的 scrollTop（钉底 / 回到底部动画），或内容尺寸变化引起的滚动。
 *   这类只做"纠正"，绝不能当成用户离开意图。
 * - `user`：其余一切 —— 滚轮、触摸、拖原生滚动条、页内查找、focus 滚动、iframe 滚动链。
 *   其中滚轮/触摸/键盘另有专门事件，走到这里的通常是**拿不到手势事件**的那些。
 */
export type ScrollSource = 'self' | 'user'


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
  | { type: 'scroll'; gap: number; now: number; source: ScrollSource }
  | { type: 'pointerDown' }
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
        // 重新跟随的三条路：本来就在跟随 / 刚有过向下手势（latch）/ 用户自己把视口带回了底部。
        // 第三条必须限定 source==='user'：self 来源（内容收缩、我们自己钉底）滚到底部不算
        // 用户意图，否则读者在上方看历史时一次内容收缩就会把他拽回底部。
        // 按住指针期间先不接（拖选文本会带出滚动），松手后下一次滚动再接。
        const userReturned = event.source === 'user' && !state.pointerHeld
        if (state.following || now <= state.latchUntil || userReturned) {
          next.following = true
        }
        return { state: next, pin: false }
      }

      if (state.following) {
        // 我们自己写的 scrollTop（钉底 / 回到底部动画），或内容尺寸变化引起的滚动：
        // 只做纠正，不改跟随状态。显著 gap 才 pin，小 gap 交给 contentGrowth / 下一帧，
        // 避免与 virtua remeasure 互抢。
        if (event.source === 'self') {
          return { state: next, pin: gap > config.correctionMinPx }
        }
        // 外部把视口拉离了底部。能走到这里的是**拿不到手势事件**的那些路径：
        // 拖原生滚动条（原生滚动条不派发 DOM 指针事件，也没有 wheel）、页内查找跳转、
        // 上方消息里的 focus 滚动、iframe 内部滚动到头后链给外层。
        // 继续钉底就会和外部反复互写 scrollTop —— 表现是抽搐、滚动条拖不动。
        //
        // 不再叠 correctionMinPx：上面的 isAtBottom 已经把底部容差区筛掉了，能到这儿就是
        // 真的离开了底部。叠上去会留一条 12–32px 的死带 —— 慢速拖原生滚动条每帧只挪几 px，
        // 单次事件永远累积不到 32，于是一直被下一次 contentGrowth 钉回去、拇指跳回鼠标下。
        // 万一某个 self 事件被误判成 user，代价只是「跟随被解除」，用户滚回底部即可恢复；
        // 反方向（该解除却继续钉底）代价是抽搐。宁可错解除。
        return { state: { ...next, following: false }, pin: false }
      }

      const movedTowardBottom = gap < previousGap - config.directionSlopPx
      if (movedTowardBottom) {
        if (now <= state.latchUntil) {
          next.latchUntil = now + config.latchMs
        }
        // Paseo：仅「向下滚且已贴底」才重跟随；不 pin（由 contentGrowth / forceFollow 钉）。
        if (!state.pointerHeld && isAtBottom(gap, config)) {
          next.following = true
        }
      }
      return { state: next, pin: false }
    }

    case 'pointerDown': {
      return { state: { ...state, pointerHeld: true }, pin: false }
    }

    case 'pointerRelease': {
      if (!state.pointerHeld) {
        return { state, pin: false }
      }
      const next = { ...state, pointerHeld: false }
      // 按住期间（拖选文本带出的自动滚动）不接跟随，松手时补判一次。
      // 不补的话有个卡死路径：拖上去 → 解除跟随 → 拖回底部（被 pointerHeld 挡住不接）→
      // 松手，此后不再有 scroll 事件，跟随就永远回不来了。
      // 只认真正贴底，不恢复原来 192px 的大区 —— 那个区会从很远处硬拽，是底部抽搐的主因之一。
      if (isAtBottom(event.gap, config)) {
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
      const next = { ...state, lastGap: event.gap }
      // 自愈：视口已经贴底却没在跟随，就接回来。
      // 这是唯一不依赖 source 判定的重跟随入口，用来兜住两条会卡死的路径：
      // 用户滚回底部的那个 scroll 恰好落在 resize 窗口里被记成 self；或者贴底后不再产生
      // 任何 scroll 事件（内容继续长只改 gap 不改 scrollTop）。按住指针期间不接。
      if (!state.following && !state.pointerHeld && isAtBottom(event.gap, config)) {
        next.following = true
        return { state: next, pin: true }
      }
      return { state: next, pin: state.following }
    }

    case 'forceFollow': {
      return {
        state: {
          ...state,
          following: true,
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
