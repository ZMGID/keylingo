import { useCallback, useEffect, useMemo, useRef, useState } from 'react'

import {
  createFollowState,
  DEFAULT_FOLLOW_CONFIG,
  type FollowConfig,
  type FollowEvent,
  type FollowState,
  isDominantVerticalWheel,
  POINTER_DRAG_SLOP_PX,
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
    el.scrollTop = el.scrollHeight
    // 第二帧必须重新问一次「现在还在跟随吗」：流式中 contentGrowth 几乎每帧都钉，
    // 用户在这一帧里滚轮上滚会先解除跟随、再被这个待执行的 rAF 拽回底部 —— 滚动被抢走。
    // 同时合并同帧内的多次钉底请求。
    if (pinRafRef.current !== null) return
    pinRafRef.current = requestAnimationFrame(() => {
      pinRafRef.current = null
      const viewport = boundViewportRef.current
      if (viewport && stateRef.current.following) viewport.scrollTop = viewport.scrollHeight
    })
  }, [cancelJumpAnimation])

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
      viewportEl.scrollTop = startTop + (target - startTop) * eased
      if (t >= 1) {
        jumpRafRef.current = null
        stickToBottom()
        return
      }
      jumpRafRef.current = requestAnimationFrame(tick)
    }
    jumpRafRef.current = requestAnimationFrame(tick)
  }, [cancelJumpAnimation, stickToBottom])

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
      dispatch({ type: 'scroll', gap: getGap(), now: Date.now() })
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

    let pointerDownX = 0
    let pointerDownY = 0
    const handlePointerDown = (event: PointerEvent) => {
      if (event.pointerType === 'mouse' && event.button === 2) {
        return
      }
      pointerDownX = event.clientX
      pointerDownY = event.clientY
      dispatch({ type: 'pointerDown' })
      if (event.target instanceof Element && event.target.closest('[data-scroll-area-scrollbar]')) {
        cancelJumpAnimation()
        dispatch({ type: 'pointerDragStart' })
      }
    }
    const handlePointerRelease = () => {
      dispatch({ type: 'pointerRelease', gap: getGap() })
    }
    const handlePointerMove = (event: PointerEvent) => {
      const state = stateRef.current
      if (!state.pointerHeld) {
        return
      }
      if (event.buttons === 0) {
        handlePointerRelease()
        return
      }
      if (!state.pointerDragging) {
        const dx = event.clientX - pointerDownX
        const dy = event.clientY - pointerDownY
        if (dx * dx + dy * dy >= POINTER_DRAG_SLOP_PX * POINTER_DRAG_SLOP_PX) {
          cancelJumpAnimation()
          dispatch({ type: 'pointerDragStart' })
        }
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
      cancelJumpAnimation()
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
