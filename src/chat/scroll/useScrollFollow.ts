import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import {
  createFollowState,
  DEFAULT_FOLLOW_CONFIG,
  type FollowConfig,
  type FollowEvent,
  type FollowState,
  isDominantVerticalWheel,
  reduceFollowEvent,
} from './scrollFollowCore'

// 低于此高度元素无法有效滚动；其上的 wheel/touch 不应改变跟随状态。
const SCROLLABLE_OVERFLOW_MIN_PX = 4

function isEditableEventTarget(target: EventTarget | null) {
  if (!(target instanceof HTMLElement)) return false
  return (
    target.isContentEditable ||
    target instanceof HTMLInputElement ||
    target instanceof HTMLTextAreaElement ||
    target instanceof HTMLSelectElement
  )
}

function isHistoryScrollKey(event: KeyboardEvent) {
  if (isEditableEventTarget(event.target)) return false
  return (
    event.key === 'ArrowUp' ||
    event.key === 'PageUp' ||
    event.key === 'Home' ||
    (event.key === ' ' && event.shiftKey)
  )
}

function isFollowScrollKey(event: KeyboardEvent) {
  if (isEditableEventTarget(event.target)) return false
  return (
    event.key === 'ArrowDown' ||
    event.key === 'PageDown' ||
    event.key === 'End' ||
    (event.key === ' ' && !event.shiftKey)
  )
}

export type ScrollFollowHandle = {
  // 强制跟随并立即钉底（元素未绑定时于视口到位后钉）。
  stickToBottom: () => void
  // 动画滑到底部后强制跟随。给用户可见的操作（回到底部按钮）用；程序钉底走 stickToBottom 瞬时。
  jumpToBottom: () => void
  // 主动脱离跟随（导航跳转到上方消息时用）。
  releaseFollow: () => void
  isFollowing: () => boolean
}

const JUMP_BASE_DURATION_MS = 260
const JUMP_MAX_DURATION_MS = 600
const JUMP_DISTANCE_DURATION_DIVISOR = 8

function prefersReducedMotion() {
  return (
    typeof window !== 'undefined' &&
    typeof window.matchMedia === 'function' &&
    window.matchMedia('(prefers-reduced-motion: reduce)').matches
  )
}

export type UseScrollFollowArgs = {
  viewport: HTMLElement | null
  content?: Element | null
  listenerRoot?: HTMLElement | null
  enabled?: boolean
  trackKeys?: boolean
  config?: Partial<FollowConfig>
}

export function useScrollFollow(args: UseScrollFollowArgs): {
  handle: ScrollFollowHandle
  following: boolean
} {
  const { viewport, content = null, listenerRoot = null, enabled = true, trackKeys = false } = args

  const stateRef = useRef<FollowState>(createFollowState())
  const boundViewportRef = useRef<HTMLElement | null>(null)
  const configRef = useRef<FollowConfig>(DEFAULT_FOLLOW_CONFIG)
  configRef.current = { ...DEFAULT_FOLLOW_CONFIG, ...args.config }
  const [following, setFollowing] = useState(true)
  const jumpRafRef = useRef<number | null>(null)
  const pinRafRef = useRef<number | null>(null)

  // 机制一（对齐 use-stick-to-bottom 的 ignoreScrollToTop）：记下**我们自己写完之后读回来的**
  // scrollTop。下一个 scroll 事件里 el.scrollTop 与之相等 → 这一下是自己弄出来的，不是用户滚的。
  // 必须读回：pin 写的是 scrollHeight，浏览器会 clamp 到 scrollHeight - clientHeight，
  // 拿写入值去比永远比不中。
  const ignoreScrollTopRef = useRef<number | null>(null)
  // 机制二（对齐 use-stick-to-bottom 的 resizeDifference）：内容尺寸变化引起的滚动，其 scroll
  // 事件**晚于** ResizeObserver 回调到达，且 scrollTop 未必等于我们写的值（浏览器滚动锚定、
  // virtua 的 shift 纠正都会插一手）。所以 RO 一响就开一个窗口，跨过一帧 + 一个宏任务才关，
  // 窗口内的 scroll 一律按 self 记账，否则会被误判成用户滚动而解除跟随。
  const resizeWindowRef = useRef(false)
  const resizeWindowRafRef = useRef<number | null>(null)
  const resizeWindowTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  // 唯一的 scrollTop 写入口：写完立刻读回并登记，别处一律不许直接赋值。
  const applyScrollTop = useCallback((el: HTMLElement, value: number) => {
    el.scrollTop = value
    ignoreScrollTopRef.current = el.scrollTop
  }, [])

  const cancelJumpAnimation = useCallback(() => {
    if (jumpRafRef.current !== null) {
      cancelAnimationFrame(jumpRafRef.current)
      jumpRafRef.current = null
    }
  }, [])

  const pinToBottom = useCallback(() => {
    cancelJumpAnimation()
    const el = boundViewportRef.current
    if (!el) return
    // 双帧钉底：virtua 估算→实测常在下一帧才把 scrollHeight 写准；
    // 只钉一次会先钉在偏低高度，下一帧再被纠正 → 底部弹一下。
    applyScrollTop(el, el.scrollHeight)
    // 第二帧必须重新问一次「现在还在跟随吗」：流式中 contentGrowth 几乎每帧都钉，
    // 用户在这一帧里滚轮上滚会先解除跟随、再被这个待执行的 rAF 拽回底部 —— 滚动被抢走。
    // 同时合并同帧内的多次钉底请求。
    if (pinRafRef.current !== null) return
    pinRafRef.current = requestAnimationFrame(() => {
      pinRafRef.current = null
      const viewport = boundViewportRef.current
      if (viewport && stateRef.current.following) applyScrollTop(viewport, viewport.scrollHeight)
    })
  }, [applyScrollTop, cancelJumpAnimation])

  const dispatch = useCallback(
    (event: FollowEvent) => {
      const wasFollowing = stateRef.current.following
      const step = reduceFollowEvent(stateRef.current, event, configRef.current)
      stateRef.current = step.state
      if (step.pin) {
        pinToBottom()
      }
      if (step.state.following !== wasFollowing) {
        setFollowing(step.state.following)
      }
    },
    [pinToBottom],
  )

  const stickToBottom = useCallback(() => {
    dispatch({ type: 'forceFollow' })
  }, [dispatch])

  const releaseFollow = useCallback(() => {
    dispatch({ type: 'release' })
  }, [dispatch])

  const jumpToBottom = useCallback(() => {
    const el = boundViewportRef.current
    const distance = el ? Math.max(0, el.scrollHeight - el.clientHeight - el.scrollTop) : 0
    if (!el || distance < 2 || prefersReducedMotion()) {
      stickToBottom()
      return
    }
    cancelJumpAnimation()
    const startTop = el.scrollTop
    const duration = Math.min(
      JUMP_MAX_DURATION_MS,
      JUMP_BASE_DURATION_MS + distance / JUMP_DISTANCE_DURATION_DIVISOR,
    )
    let startTs: number | null = null
    const tick = (ts: number) => {
      const viewportEl = boundViewportRef.current
      if (!viewportEl) {
        jumpRafRef.current = null
        return
      }
      if (startTs === null) {
        startTs = ts
      }
      const t = Math.min(1, (ts - startTs) / duration)
      const eased = 1 - (1 - t) ** 3
      const target = viewportEl.scrollHeight - viewportEl.clientHeight
      applyScrollTop(viewportEl, startTop + (target - startTop) * eased)
      if (t >= 1) {
        jumpRafRef.current = null
        stickToBottom()
        return
      }
      jumpRafRef.current = requestAnimationFrame(tick)
    }
    jumpRafRef.current = requestAnimationFrame(tick)
  }, [applyScrollTop, cancelJumpAnimation, stickToBottom])

  useEffect(() => {
    if (!enabled || !viewport) {
      return
    }
    const root = listenerRoot ?? viewport
    const growthTarget = content ?? viewport.firstElementChild

    // 新绑定总是跟随：新挂载、视口重建、重新启用都从钉底开始，元素到位前 dispatch 的 forceFollow 也由此兑现。
    boundViewportRef.current = viewport
    stateRef.current = createFollowState()
    setFollowing(true)
    pinToBottom()

    const getGap = () =>
      Math.max(0, viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight)
    const hasOverflow = () =>
      viewport.scrollHeight - viewport.clientHeight > SCROLLABLE_OVERFLOW_MIN_PX
    const pendingScrollTimers = new Set<ReturnType<typeof setTimeout>>()

    const nestedCanConsumeWheelUp = (target: EventTarget | null) => {
      let node = target instanceof Element ? target : null
      while (node && node !== viewport && node !== root) {
        if (
          node instanceof HTMLElement &&
          node.scrollTop > 0 &&
          node.scrollHeight - node.clientHeight > SCROLLABLE_OVERFLOW_MIN_PX
        ) {
          return true
        }
        node = node.parentElement
      }
      return false
    }

    const handleScroll = () => {
      // 来源判定见 ignoreScrollTopRef / resizeWindowRef 的注释。
      // token **读一次就作废**：一次写入只授权一个 scroll 事件。不作废的话会卡死 ——
      // 「底部」这个数值是稳定的（我们 pin 写 scrollHeight，浏览器 clamp 成 max；
      // 用户把滚动条拖到最底，浏览器写进去的也是同一个 max，逐位相等），于是用户
      // 拖回底部那一下会被永远判成 self，三条重跟随的路全部落空。
      const token = ignoreScrollTopRef.current
      ignoreScrollTopRef.current = null
      const scrollTop = viewport.scrollTop
      const gap = getGap()
      // scroll 可能先于 ResizeObserver delivery 到达。延后一拍再判来源，让 RO 有机会打开
      // resizeWindow；否则 virtua 的高度补偿会被当成用户滚动，直接解除流式跟随。
      const timer = setTimeout(() => {
        pendingScrollTimers.delete(timer)
        const selfInduced = resizeWindowRef.current || scrollTop === token
        dispatch({
          type: 'scroll',
          gap,
          now: Date.now(),
          source: selfInduced ? 'self' : 'user',
        })
      }, 1)
      pendingScrollTimers.add(timer)
    }

    const handleWheel = (event: WheelEvent) => {
      if (isDominantVerticalWheel(event.deltaX, event.deltaY)) {
        cancelJumpAnimation()
      }
      dispatch({
        type: 'wheel',
        deltaX: event.deltaX,
        deltaY: event.deltaY,
        gap: getGap(),
        hasOverflow: hasOverflow(),
        nestedCanConsume: event.deltaY < 0 && nestedCanConsumeWheelUp(event.target),
        now: Date.now(),
      })
    }

    let touchY: number | null = null
    const handleTouchStart = (event: TouchEvent) => {
      touchY = event.touches[0]?.clientY ?? null
    }
    const handleTouchMove = (event: TouchEvent) => {
      cancelJumpAnimation()
      const nextY = event.touches[0]?.clientY ?? null
      const previousY = touchY
      touchY = nextY
      dispatch({
        type: 'touchMove',
        fingerMovedDown: previousY === null || nextY === null ? null : nextY > previousY + 1,
        gap: getGap(),
        hasOverflow: hasOverflow(),
        now: Date.now(),
      })
    }

    // pointerHeld 只用于「按住不放时别自动重新跟随」（按住拖选文本会带出滚动）。
    // 原来这里还有一条按 `[data-scroll-area-scrollbar]` 识别拖滚动条的分支 —— 那个属性
    // 整个 src/ 里没有任何组件渲染（是从自定义滚动条组件那边搬过来时留下的），聊天列表用的是
    // .custom-scrollbar 原生滚动条，永远命中不了。原生滚动条既不派发 DOM 指针事件也没有 wheel，
    // 现在由 scroll 事件的 source 判定统一接管，这条分支连同 pointerDragging 一起删掉了。
    const handlePointerDown = (event: PointerEvent) => {
      if (event.pointerType === 'mouse' && event.button === 2) {
        return
      }
      dispatch({ type: 'pointerDown' })
    }
    const handlePointerRelease = () => {
      dispatch({ type: 'pointerRelease', gap: getGap() })
    }
    const handlePointerMove = (event: PointerEvent) => {
      if (!stateRef.current.pointerHeld) {
        return
      }
      if (event.buttons === 0) {
        handlePointerRelease()
      }
    }

    const handleKeyDown = (event: KeyboardEvent) => {
      if (isHistoryScrollKey(event)) {
        cancelJumpAnimation()
        dispatch({ type: 'historyKey', hasOverflow: hasOverflow(), now: Date.now() })
      } else if (isFollowScrollKey(event)) {
        dispatch({ type: 'followKey', now: Date.now() })
      }
    }

    const handleVisibilityChange = () => {
      if (document.visibilityState === 'visible' && stateRef.current.following) {
        pinToBottom()
      }
    }

    viewport.addEventListener('scroll', handleScroll, { passive: true })
    root.addEventListener('wheel', handleWheel, { passive: true })
    root.addEventListener('touchstart', handleTouchStart, { passive: true })
    root.addEventListener('touchmove', handleTouchMove, { passive: true })
    root.addEventListener('pointerdown', handlePointerDown, { passive: true })
    window.addEventListener('pointerup', handlePointerRelease, { passive: true })
    window.addEventListener('pointercancel', handlePointerRelease, { passive: true })
    window.addEventListener('pointermove', handlePointerMove, { passive: true })
    window.addEventListener('blur', handlePointerRelease)
    if (trackKeys) {
      window.addEventListener('keydown', handleKeyDown, { capture: true })
    }
    document.addEventListener('visibilitychange', handleVisibilityChange)

    // 布局后、绘制前，在每次内容/视口尺寸变化时触发 —— 这就是流式钉底驱动。
    const resizeObserver =
      typeof ResizeObserver === 'undefined'
        ? null
        : new ResizeObserver(() => {
            // 开一个「这段时间的滚动都算 self」的窗口。尺寸变化引起的 scroll 事件晚于本回调
            // 到达，且 scrollTop 未必等于我们写进去的值（浏览器滚动锚定、virtua 的 shift 纠正
            // 都会插一手 —— virtua 那次写入根本不经过 applyScrollTop，全靠这个窗口兜住），
            // 所以窗口要跨过一帧 + 一个宏任务才关。
            //
            // **每次 resize 都必须把待执行的关闭动作取消重排**（= 尾部 debounce）。只判
            // rAF 是否在飞不行：连续增长（正是流式）时，第二次 resize 到来的那一刻上一轮的
            // 关闭定时器可能已经排出去了，它会在本轮的 scroll 事件之前把窗口关掉，
            // 结果一串 resize 里只有第一个受保护，后面每个都被判成 user → 流式中途莫名解除跟随。
            resizeWindowRef.current = true
            if (resizeWindowRafRef.current !== null) {
              cancelAnimationFrame(resizeWindowRafRef.current)
            }
            if (resizeWindowTimerRef.current !== null) {
              clearTimeout(resizeWindowTimerRef.current)
              resizeWindowTimerRef.current = null
            }
            resizeWindowRafRef.current = requestAnimationFrame(() => {
              resizeWindowRafRef.current = null
              resizeWindowTimerRef.current = setTimeout(() => {
                resizeWindowTimerRef.current = null
                resizeWindowRef.current = false
              }, 0)
            })
            dispatch({ type: 'contentGrowth', gap: getGap() })
          })
    resizeObserver?.observe(viewport)
    if (growthTarget instanceof Element) {
      resizeObserver?.observe(growthTarget)
    }

    return () => {
      viewport.removeEventListener('scroll', handleScroll)
      root.removeEventListener('wheel', handleWheel)
      root.removeEventListener('touchstart', handleTouchStart)
      root.removeEventListener('touchmove', handleTouchMove)
      root.removeEventListener('pointerdown', handlePointerDown)
      window.removeEventListener('pointerup', handlePointerRelease)
      window.removeEventListener('pointercancel', handlePointerRelease)
      window.removeEventListener('pointermove', handlePointerMove)
      window.removeEventListener('blur', handlePointerRelease)
      if (trackKeys) {
        window.removeEventListener('keydown', handleKeyDown, { capture: true })
      }
      document.removeEventListener('visibilitychange', handleVisibilityChange)
      resizeObserver?.disconnect()
      for (const timer of pendingScrollTimers) clearTimeout(timer)
      pendingScrollTimers.clear()
      cancelJumpAnimation()
      if (pinRafRef.current !== null) {
        cancelAnimationFrame(pinRafRef.current)
        pinRafRef.current = null
      }
      if (resizeWindowRafRef.current !== null) {
        cancelAnimationFrame(resizeWindowRafRef.current)
        resizeWindowRafRef.current = null
      }
      if (resizeWindowTimerRef.current !== null) {
        clearTimeout(resizeWindowTimerRef.current)
        resizeWindowTimerRef.current = null
      }
      resizeWindowRef.current = false
      ignoreScrollTopRef.current = null
      boundViewportRef.current = null
    }
  }, [cancelJumpAnimation, content, dispatch, enabled, listenerRoot, pinToBottom, trackKeys, viewport])

  const handle = useMemo<ScrollFollowHandle>(
    () => ({
      stickToBottom,
      jumpToBottom,
      releaseFollow,
      isFollowing: () => stateRef.current.following,
    }),
    [jumpToBottom, releaseFollow, stickToBottom],
  )

  return { handle, following }
}
