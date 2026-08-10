import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react'


import {
  createFollowState,
  DEFAULT_FOLLOW_CONFIG,
  type FollowConfig,
  type FollowEvent,
  type FollowState,
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
  // 瞬时跳到底部并强制跟随。给用户可见的操作（回到底部按钮）用；与 stickToBottom 同路径，
  // 不走平滑动画——长会话 virtualizer 重测补偿会中途打断 rAF 动画，导致要点多次。
  jumpToBottom: () => void
  // 主动脱离跟随（导航跳转到上方消息时用）。
  releaseFollow: () => void
  // 程序化定位（消息导航）唯一的 scrollTop 写入口，不改变 follow 意图。
  scrollToOffset: (offset: number, options?: { adjustments?: number; behavior?: ScrollBehavior }) => void
  isFollowing: () => boolean
  markLayoutCompensation: () => void
}

// 跟随解除和按钮显示故意分开：底部附近的微小上滚不应立刻弹出按钮。
// 用视口高度算「明显离开底部」的距离，再设上下限，避免固定 px 在不同窗口里过早/过晚出现。
const JUMP_BUTTON_SHOW_RATIO = 0.35
const JUMP_BUTTON_SHOW_MIN_PX = 240
const JUMP_BUTTON_SHOW_MAX_PX = 480

export type UseScrollFollowArgs = {
  viewport: HTMLElement | null
  content?: Element | null
  listenerRoot?: HTMLElement | null
  enabled?: boolean
  trackKeys?: boolean
  config?: Partial<FollowConfig>
  /**
   * 流式内容指纹。ResizeObserver 是主路径；外置 live row / jsdom stub 等 RO
   * 不投递时，跟随时若 scrollHeight 真变了，靠这个信号补 contentGrowth。
   * 高度没变则 no-op，避免和 RO 双通道每 token 互写。
   */
  growthSignal?: string | number | null
}

export function useScrollFollow(args: UseScrollFollowArgs): {
  handle: ScrollFollowHandle
  following: boolean
  showJumpButton: boolean
} {
  const {
    viewport,
    content = null,
    listenerRoot = null,
    enabled = true,
    trackKeys = false,
    growthSignal = null,
  } = args


  const stateRef = useRef<FollowState>(createFollowState())
  const boundViewportRef = useRef<HTMLElement | null>(null)
  const configRef = useRef<FollowConfig>(DEFAULT_FOLLOW_CONFIG)
  configRef.current = { ...DEFAULT_FOLLOW_CONFIG, ...args.config }
  const [following, setFollowing] = useState(true)
  const [showJumpButton, setShowJumpButton] = useState(false)
  const layoutCompensationTicketRef = useRef(false)
  const layoutCompensationClearRafRef = useRef<number | null>(null)

  // 机制一（对齐 use-stick-to-bottom 的 ignoreScrollToTop）：记下**我们自己写完之后读回来的**
  // scrollTop。下一个 scroll 事件里 el.scrollTop 与之相等 → 这一下是自己弄出来的，不是用户滚的。
  // 必须读回：pin 写的是 scrollHeight，浏览器会 clamp 到 scrollHeight - clientHeight，
  // 拿写入值去比永远比不中。
  const ignoreScrollTopRef = useRef<number | null>(null)
  // Keep only geometry evidence for virtualizer/browser compensation. There is no timer-based
  // "resize window": user intent is recorded by wheel/touch/key events, while a scroll that
  // arrives with a larger scrollHeight is treated as layout compensation.
  const lastScrollHeightRef = useRef<number | null>(null)
  const lastScrollTopRef = useRef<number | null>(null)
  const geometrySampledRef = useRef(false)

  // 唯一的 scrollTop 写入口：写完立刻读回并登记，别处一律不许直接赋值。
  const applyScrollTop = useCallback((el: HTMLElement, value: number) => {
    el.scrollTop = value
    ignoreScrollTopRef.current = el.scrollTop
  }, [])

  const jumpButtonThreshold = useCallback(() => {
    const viewport = boundViewportRef.current
    const viewportHeight = viewport?.clientHeight ?? 0
    return Math.min(
      JUMP_BUTTON_SHOW_MAX_PX,
      Math.max(JUMP_BUTTON_SHOW_MIN_PX, viewportHeight * JUMP_BUTTON_SHOW_RATIO),
    )
  }, [])

  const markLayoutCompensation = useCallback(() => {
    layoutCompensationTicketRef.current = true
    if (layoutCompensationClearRafRef.current !== null) {
      cancelAnimationFrame(layoutCompensationClearRafRef.current)
    }
    layoutCompensationClearRafRef.current = requestAnimationFrame(() => {
      // Scroll events caused by a measurement/anchor correction are delivered
      // in the scroll steps before the following paint. Keep the ticket through
      // one extra frame so a ResizeObserver → virtualizer → scroll sequence is
      // one explicit transaction even when the first rAF also performs pinning.
      layoutCompensationClearRafRef.current = requestAnimationFrame(() => {
        layoutCompensationClearRafRef.current = null
        layoutCompensationTicketRef.current = false
      })
    })
  }, [])

  const pinToBottom = useCallback(() => {
    // 对齐 LiveAgent：瞬时单次写入。双帧 rAF 会在 paint 后再钉一次，
    // 流式中表现就是生成内容整段「往下闪」。virtualizer 估算→实测的第二下
    // 高度变化由 ResizeObserver contentGrowth 再钉，节奏已 ≤1/frame。
    const el = boundViewportRef.current
    if (el) applyScrollTop(el, el.scrollHeight)
  }, [applyScrollTop])

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
      // `gap` 是当前事件采样值；没有 gap 的控制事件（forceFollow/release）沿用
      // 状态里的最近一次采样。只在跨过显示阈值时更新，避免每个 scroll 帧触发重渲。
      const gap = 'gap' in event ? event.gap : step.state.lastGap
      const shouldShowJumpButton = !step.state.following && gap > jumpButtonThreshold()
      setShowJumpButton((visible) => visible === shouldShowJumpButton ? visible : shouldShowJumpButton)
    },
    [jumpButtonThreshold, pinToBottom],
  )

  const stickToBottom = useCallback(() => {
    dispatch({ type: 'forceFollow' })
  }, [dispatch])

  const releaseFollow = useCallback(() => {
    dispatch({ type: 'release' })
  }, [dispatch])

  const scrollToOffset = useCallback((offset: number, options?: { adjustments?: number; behavior?: ScrollBehavior }) => {
    const viewport = boundViewportRef.current
    if (!viewport) return
    const nextOffset = Math.max(0, offset + (options?.adjustments ?? 0))
    // TanStack's scrollToFn and message navigation stay synchronous so every
    // resulting scroll event matches this authority's one-shot programmatic token.
    applyScrollTop(viewport, nextOffset)
  }, [applyScrollTop])

  // Instant jump: force-follow + pin. No rAF animation — long chats remeasure
  // rows mid-flight and used to cancel the smooth path before stickToBottom ran.
  const jumpToBottom = useCallback(() => {
    stickToBottom()
  }, [stickToBottom])

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
    setShowJumpButton(false)
    pinToBottom()
    lastScrollHeightRef.current = viewport.scrollHeight
    lastScrollTopRef.current = viewport.scrollTop
    geometrySampledRef.current = false

    const getGap = () =>
      Math.max(0, viewport.scrollHeight - viewport.scrollTop - viewport.clientHeight)
    const hasOverflow = () =>
      viewport.scrollHeight - viewport.clientHeight > SCROLLABLE_OVERFLOW_MIN_PX
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
      // token **读一次就作废**：一次写入只授权一个 scroll 事件。不作废的话会卡死 ——
      // 「底部」这个数值是稳定的（我们 pin 写 scrollHeight，浏览器 clamp 成 max；
      // 用户把滚动条拖到最底，浏览器写进去的也是同一个 max，逐位相等），于是用户
      // 拖回底部那一下会被永远判成 self，三条重跟随的路全部落空。
      const token = ignoreScrollTopRef.current
      ignoreScrollTopRef.current = null
      const scrollTop = viewport.scrollTop
      const gap = getGap()
      const previousScrollHeight = lastScrollHeightRef.current
      const previousScrollTop = lastScrollTopRef.current
      const contentGrewBeforeScroll =
        geometrySampledRef.current &&
        previousScrollHeight !== null &&
        viewport.scrollHeight > previousScrollHeight &&
        (previousScrollTop === null || scrollTop >= previousScrollTop - 1) &&
        stateRef.current.following
      lastScrollHeightRef.current = viewport.scrollHeight
      lastScrollTopRef.current = scrollTop
      geometrySampledRef.current = true
      const layoutCompensation = layoutCompensationTicketRef.current
      layoutCompensationTicketRef.current = false
      const userReturnedDuringLayout = layoutCompensation
        && stateRef.current.userDetached
        && gap <= configRef.current.attachThresholdPx
        && previousScrollTop !== null
        && scrollTop > previousScrollTop + 1
      const selfInduced = scrollTop === token || contentGrewBeforeScroll || (layoutCompensation && !userReturnedDuringLayout)
      dispatch({
        type: 'scroll',
        gap,
        now: Date.now(),
        source: selfInduced ? 'self' : 'user',
      })
    }

    const handleWheel = (event: WheelEvent) => {
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
            markLayoutCompensation()
            dispatch({ type: 'contentGrowth', gap: getGap() })
            lastScrollHeightRef.current = viewport.scrollHeight
            lastScrollTopRef.current = viewport.scrollTop
            geometrySampledRef.current = true
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
      if (layoutCompensationClearRafRef.current !== null) {
        cancelAnimationFrame(layoutCompensationClearRafRef.current)
        layoutCompensationClearRafRef.current = null
      }
      layoutCompensationTicketRef.current = false
      ignoreScrollTopRef.current = null
      boundViewportRef.current = null
      lastScrollHeightRef.current = null
      lastScrollTopRef.current = null
      geometrySampledRef.current = false
    }
  }, [content, dispatch, enabled, listenerRoot, markLayoutCompensation, pinToBottom, trackKeys, viewport])

  // RO 主路径之外的补钉：仅当跟随中且 scrollHeight 真的变了。
  useLayoutEffect(() => {
    if (!enabled || !viewport || growthSignal == null) return
    if (!stateRef.current.following) return
    const nextHeight = viewport.scrollHeight
    const prevHeight = lastScrollHeightRef.current
    if (prevHeight != null && nextHeight === prevHeight) return
    markLayoutCompensation()
    const gap = Math.max(0, nextHeight - viewport.scrollTop - viewport.clientHeight)
    dispatch({ type: 'contentGrowth', gap })
    lastScrollHeightRef.current = nextHeight
    lastScrollTopRef.current = viewport.scrollTop
  }, [dispatch, enabled, growthSignal, markLayoutCompensation, viewport])


  const handle = useMemo<ScrollFollowHandle>(
    () => ({
      stickToBottom,
      jumpToBottom,
      releaseFollow,
      scrollToOffset,
      isFollowing: () => stateRef.current.following,
      markLayoutCompensation,
    }),
    [jumpToBottom, markLayoutCompensation, releaseFollow, scrollToOffset, stickToBottom],
  )

  return { handle, following, showJumpButton }
}
