import { useEffect, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react'
import type { LensReplaceGroup, LensReplaceRenderSlot } from '../api/tauri'
import { copyToClipboard } from '../utils/clipboard'
import { DRAG_THRESHOLD } from './layout'
import { layoutReplaceTextFlow, replaceTextVerticalOffset, selectedGroupsText, type ReplaceTextFlowSlotLayout } from './replaceTextLayout'
import type { CapturedFrame } from './types'

type ReplaceTranslateOverlayProps = {
  frame: CapturedFrame
  cleanedImage: string
  groups: LensReplaceGroup[]
  slots: LensReplaceRenderSlot[]
  phase: 'ocr' | 'processing' | 'done' | 'error' | ''
  statusLabel: string
  // 局部降级详情（如修复回退、个别区域回退原文）：非错误，挂在状态胶囊 title 上。
  statusTitle?: string
  escHint: string
  interactHint: string
  showOriginalLabel: string
  showTranslatedLabel: string
  copiedLabel: string
}

type SelectionRect = { x1: number; y1: number; x2: number; y2: number }

function normalizedRect(selection: SelectionRect) {
  return {
    x: Math.min(selection.x1, selection.x2),
    y: Math.min(selection.y1, selection.y2),
    width: Math.abs(selection.x2 - selection.x1),
    height: Math.abs(selection.y2 - selection.y1),
  }
}

function textX(bounds: LensReplaceRenderSlot['bounds'], align: LensReplaceRenderSlot['align'], padding: number) {
  return align === 'center'
    ? bounds.x + bounds.width / 2
    : align === 'right'
      ? bounds.x + bounds.width - padding
      : bounds.x + padding
}

function anchoredTextX(slot: LensReplaceRenderSlot, padding: number) {
  // Left-aligned text (exact lines AND table cells) starts at the measured ink
  // anchor so translations don't slide toward the region/cell border.
  return slot.align === 'left' ? slot.anchor.x : textX(slot.bounds, slot.align, padding)
}

function anchoredTextY(
  slot: LensReplaceRenderSlot,
  innerHeight: number,
  contentHeight: number,
  padding: number,
) {
  if (slot.flow === 'exact_line' || slot.verticalAlign === 'top') return slot.anchor.y
  return slot.bounds.y + padding + replaceTextVerticalOffset(slot.kind, innerHeight, contentHeight)
}

function drawNormalSlotText(
  ctx: CanvasRenderingContext2D,
  slot: LensReplaceRenderSlot,
  layout: ReplaceTextFlowSlotLayout,
  fontPx: number,
  lineHeight: number,
  padding: number,
) {
  const { bounds } = slot
  const innerHeight = Math.max(1, bounds.height - padding * 2)
  ctx.font = `${fontPx}px system-ui, "Segoe UI", sans-serif`
  ctx.fillStyle = slot.sourceColor
  ctx.textBaseline = 'top'
  ctx.textAlign = slot.align
  const x = anchoredTextX(slot, padding)
  let y = anchoredTextY(slot, innerHeight, layout.contentHeight, padding)
  for (const line of layout.lines) {
    ctx.fillText(line, x, y)
    y += lineHeight
  }
}

function drawSafelyScaledSlotText(
  ctx: CanvasRenderingContext2D,
  slot: LensReplaceRenderSlot,
  layout: ReplaceTextFlowSlotLayout,
  fontPx: number,
  lineHeight: number,
  safeScale: number,
  padding: number,
) {
  const offscreen = document.createElement('canvas')
  offscreen.width = Math.max(1, Math.ceil(slot.bounds.width / safeScale))
  offscreen.height = Math.max(1, Math.ceil(slot.bounds.height / safeScale))
  const offscreenCtx = offscreen.getContext('2d')
  if (!offscreenCtx) return
  const virtualPadding = padding / safeScale
  const innerHeight = Math.max(1, offscreen.height - virtualPadding * 2)
  offscreenCtx.font = `${fontPx}px system-ui, "Segoe UI", sans-serif`
  offscreenCtx.fillStyle = slot.sourceColor
  offscreenCtx.textBaseline = 'top'
  offscreenCtx.textAlign = slot.align
  const anchorX = (slot.anchor.x - slot.bounds.x) / safeScale
  const anchorY = (slot.anchor.y - slot.bounds.y) / safeScale
  const x = slot.align === 'left'
    ? anchorX
    : slot.align === 'center'
    ? offscreen.width / 2
    : offscreen.width - virtualPadding
  let y = slot.flow === 'exact_line' || slot.verticalAlign === 'top'
    ? anchorY
    : virtualPadding + replaceTextVerticalOffset(slot.kind, innerHeight, layout.contentHeight)
  for (const line of layout.lines) {
    offscreenCtx.fillText(line, x, y)
    y += lineHeight
  }
  ctx.drawImage(offscreen, slot.bounds.x, slot.bounds.y, slot.bounds.width, slot.bounds.height)
}

export function ReplaceTranslateOverlay({
  frame,
  cleanedImage,
  groups,
  slots,
  phase,
  statusLabel,
  statusTitle,
  escHint,
  interactHint,
  showOriginalLabel,
  showTranslatedLabel,
  copiedLabel,
}: ReplaceTranslateOverlayProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null)
  // 已解码图的自然像素尺寸，onload 时写入。slots bounds 是自然像素，框选换算依赖它；
  // 解码完成前 canvas.width 还是 HTML 默认 300×150，直接用会把选框映射到错误区域。
  const naturalSizeRef = useRef<{ w: number; h: number } | null>(null)
  const [showOriginal, setShowOriginal] = useState(false)
  const [selection, setSelection] = useState<SelectionRect | null>(null)
  const [copied, setCopied] = useState(false)
  const dragRef = useRef<{ pointerId: number; startX: number; startY: number; boxLeft: number; boxTop: number; dragging: boolean } | null>(null)
  const copiedTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)

  // 新一轮结果到达时复位交互态，避免上一张图的原文模式/选框残留。
  useEffect(() => {
    setShowOriginal(false)
    setSelection(null)
    setCopied(false)
  }, [cleanedImage])

  useEffect(() => () => {
    if (copiedTimerRef.current) clearTimeout(copiedTimerRef.current)
  }, [])

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas || !cleanedImage || groups.length === 0 || slots.length === 0 || phase !== 'done') return
    const context = canvas.getContext('2d')
    if (!context) return
    let cancelled = false
    naturalSizeRef.current = null
    const image = new Image()
    image.onerror = () => {
      if (!cancelled) naturalSizeRef.current = null
    }
    image.onload = () => {
      if (cancelled) return
      canvas.width = Math.max(1, image.naturalWidth)
      canvas.height = Math.max(1, image.naturalHeight)
      naturalSizeRef.current = { w: canvas.width, h: canvas.height }
      context.clearRect(0, 0, canvas.width, canvas.height)
      context.drawImage(image, 0, 0)
      const slotsByGroup = new Map<string, LensReplaceRenderSlot[]>()
      for (const slot of slots) {
        const groupSlots = slotsByGroup.get(slot.groupId) ?? []
        groupSlots.push(slot)
        slotsByGroup.set(slot.groupId, groupSlots)
      }
      for (const group of groups) {
        const groupSlots = (slotsByGroup.get(group.id) ?? [])
          .sort((left, right) => left.anchor.y - right.anchor.y || left.anchor.x - right.anchor.x)
        if (groupSlots.length === 0) continue
        const text = group.translated.trim() || group.sourceText
        if (!text) continue
        const sourceFontPx = Math.max(...groupSlots.map(slot => slot.sourceFontPx))
        const padding = Math.max(2, Math.min(6, sourceFontPx * 0.2))
        const layout = layoutReplaceTextFlow(
          text,
          groupSlots.map(slot => ({
            width: Math.max(1, slot.bounds.width - padding * 2),
            height: Math.max(1, slot.bounds.height - padding * 2),
          })),
          sourceFontPx,
          (value, fontPx) => {
            context.font = `${fontPx}px system-ui, "Segoe UI", sans-serif`
            return context.measureText(value).width
          },
        )
        groupSlots.forEach((slot, index) => {
          const slotLayout = layout.slots[index]
          if (!slotLayout || slotLayout.lines.length === 0) return
          context.save()
          context.beginPath()
          context.rect(slot.bounds.x, slot.bounds.y, slot.bounds.width, slot.bounds.height)
          context.clip()
          // ponytail: scene_patch currently degrades to the plain system-font path
          // (content stays complete). Rotation threading + a gated photo-redraw
          // model are deferred to scene-rendering; do not add them speculatively.
          if (layout.safeScale < 1) {
            drawSafelyScaledSlotText(context, slot, slotLayout, layout.fontPx, layout.lineHeight, layout.safeScale, padding)
          } else {
            drawNormalSlotText(context, slot, slotLayout, layout.fontPx, layout.lineHeight, padding)
          }
          context.restore()
        })
      }
    }
    image.src = cleanedImage
    return () => {
      cancelled = true
    }
  }, [cleanedImage, groups, phase, slots])

  const showOverlay = phase === 'done' && cleanedImage && groups.length > 0 && slots.length > 0

  // frame 内 CSS 坐标 → canvas 自然像素坐标（slots bounds 是自然像素）。
  // 解码尺寸就绪才换算；未就绪返回 null → 调用方跳过复制，绝不用错误比例算选区。
  const toCanvasRect = (rect: { x: number; y: number; width: number; height: number }) => {
    const nat = naturalSizeRef.current
    if (!nat) return null
    const scaleX = nat.w / Math.max(1, frame.width)
    const scaleY = nat.h / Math.max(1, frame.height)
    return { x: rect.x * scaleX, y: rect.y * scaleY, width: rect.width * scaleX, height: rect.height * scaleY }
  }

  const flashCopied = () => {
    setCopied(true)
    if (copiedTimerRef.current) clearTimeout(copiedTimerRef.current)
    copiedTimerRef.current = setTimeout(() => setCopied(false), 1200)
  }

  const handlePointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (event.button !== 0) return
    // 覆盖层绝对定位且拖拽期间不动，box 只需在按下时量一次，move/up 复用避免每帧强制重排。
    const box = event.currentTarget.getBoundingClientRect()
    dragRef.current = {
      pointerId: event.pointerId,
      startX: event.clientX - box.left,
      startY: event.clientY - box.top,
      boxLeft: box.left,
      boxTop: box.top,
      dragging: false,
    }
    event.currentTarget.setPointerCapture?.(event.pointerId)
  }

  const handlePointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current
    if (!drag || drag.pointerId !== event.pointerId) return
    const x = event.clientX - drag.boxLeft
    const y = event.clientY - drag.boxTop
    if (!drag.dragging && Math.hypot(x - drag.startX, y - drag.startY) < DRAG_THRESHOLD) return
    drag.dragging = true
    setSelection({ x1: drag.startX, y1: drag.startY, x2: x, y2: y })
  }

  const handlePointerUp = (event: ReactPointerEvent<HTMLDivElement>) => {
    const drag = dragRef.current
    if (!drag || drag.pointerId !== event.pointerId) return
    dragRef.current = null
    if (!drag.dragging) {
      // 单击（无位移）：切换原文/译文对比。
      setShowOriginal(v => !v)
      return
    }
    const rect = normalizedRect({
      x1: drag.startX,
      y1: drag.startY,
      x2: event.clientX - drag.boxLeft,
      y2: event.clientY - drag.boxTop,
    })
    setSelection(null)
    const canvasRect = toCanvasRect(rect)
    if (!canvasRect) return
    const text = selectedGroupsText(groups, slots, canvasRect, showOriginal)
    if (!text) return
    void copyToClipboard(text).then(ok => {
      if (ok) flashCopied()
    })
  }

  const handlePointerCancel = () => {
    dragRef.current = null
    setSelection(null)
  }

  const selectionBox = selection ? normalizedRect(selection) : null
  return (
    <>
      <div className="absolute top-[calc(env(safe-area-inset-top,0px)+36px)] left-0 right-0 z-30 flex flex-col items-center pointer-events-none">
        <div className="flex items-center gap-2 pointer-events-none">
          <div
            className="px-3 py-1.5 rounded-full text-[12px] font-medium bg-black/70 text-white shadow-lg backdrop-blur-sm"
            title={statusTitle}
          >
            {statusLabel}
            {phase !== 'done' && phase !== 'error' && (
              <span className="ml-2 inline-block w-1.5 h-1.5 rounded-full bg-white animate-pulse align-middle" />
            )}
          </div>
          {showOverlay && (
            <button
              type="button"
              className="pointer-events-auto px-3 py-1.5 rounded-full text-[12px] font-medium bg-black/70 text-white shadow-lg backdrop-blur-sm hover:bg-black/80"
              onClick={() => setShowOriginal(v => !v)}
            >
              {showOriginal ? showTranslatedLabel : showOriginalLabel}
            </button>
          )}
          {/* 复制反馈用独立瞬态胶囊，不遮盖真实状态（避免与 statusTitle 警告提示错配）。 */}
          {copied && (
            <div className="px-3 py-1.5 rounded-full text-[12px] font-medium bg-[#2f6ff0] text-white shadow-lg backdrop-blur-sm">
              {copiedLabel}
            </div>
          )}
        </div>
        <p className="text-center text-[11px] text-white/80 mt-1 drop-shadow">
          {showOverlay ? interactHint : escHint}
        </p>
      </div>
      {showOverlay && (
        <div
          className="absolute z-20 rounded-md overflow-hidden pointer-events-auto cursor-crosshair select-none"
          style={{ left: frame.x, top: frame.y, width: frame.width, height: frame.height }}
          onPointerDown={handlePointerDown}
          onPointerMove={handlePointerMove}
          onPointerUp={handlePointerUp}
          onPointerCancel={handlePointerCancel}
        >
          <canvas
            ref={canvasRef}
            className="block"
            style={{ width: frame.width, height: frame.height, visibility: showOriginal ? 'hidden' : 'visible' }}
          />
          {selectionBox && (
            <div
              className="absolute border border-[#2f6ff0] bg-[#2f6ff0]/15 pointer-events-none"
              style={{
                left: selectionBox.x,
                top: selectionBox.y,
                width: selectionBox.width,
                height: selectionBox.height,
              }}
            />
          )}
        </div>
      )}
    </>
  )
}
