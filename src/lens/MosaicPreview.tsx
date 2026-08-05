import { useEffect, useRef } from 'react'
import { MOSAIC_BLOCK_RATIO } from './annotation'
import type { Annotation } from './types'

/**
 * 已落定马赛克的实时预览层：从截图位图采样，把每个 mosaic 区域真实像素化后画到
 * 透明 canvas 上（其余区域透明）。与导出合成（annotation.ts drawMosaic）同一算法，
 * 保证所见即所得。坐标系 = capturedFrame 逻辑像素。
 */
export function MosaicPreview({
  imageSrc,
  annotations,
  width,
  height,
}: {
  imageSrc: string
  annotations: Annotation[]
  width: number
  height: number
}) {
  const canvasRef = useRef<HTMLCanvasElement>(null)

  useEffect(() => {
    const canvas = canvasRef.current
    if (!canvas) return
    const mosaics = annotations.filter(a => a.kind === 'mosaic')
    const dpr = window.devicePixelRatio || 1
    const bw = Math.max(1, Math.round(width * dpr))
    const bh = Math.max(1, Math.round(height * dpr))
    if (canvas.width !== bw) canvas.width = bw
    if (canvas.height !== bh) canvas.height = bh
    const ctx = canvas.getContext('2d')
    if (!ctx) return
    ctx.clearRect(0, 0, canvas.width, canvas.height)
    if (mosaics.length === 0 || !imageSrc) return

    let cancelled = false
    const img = new Image()
    img.onload = () => {
      if (cancelled) return
      // 逻辑像素 → 截图物理像素
      const scaleX = width > 0 ? img.naturalWidth / width : 1
      const scaleY = height > 0 ? img.naturalHeight / height : 1
      const blockSize = Math.max(8, img.naturalWidth * MOSAIC_BLOCK_RATIO)
      for (const m of mosaics) {
        const sx = Math.max(0, Math.round(Math.min(m.x1, m.x2) * scaleX))
        const sy = Math.max(0, Math.round(Math.min(m.y1, m.y2) * scaleY))
        const sw = Math.min(img.naturalWidth - sx, Math.round(Math.abs(m.x2 - m.x1) * scaleX))
        const sh = Math.min(img.naturalHeight - sy, Math.round(Math.abs(m.y2 - m.y1) * scaleY))
        if (sw < 2 || sh < 2) continue
        const cols = Math.max(1, Math.round(sw / blockSize))
        const rows = Math.max(1, Math.round(sh / blockSize))
        const tiny = document.createElement('canvas')
        tiny.width = cols
        tiny.height = rows
        const tinyCtx = tiny.getContext('2d')
        if (!tinyCtx) continue
        tinyCtx.imageSmoothingEnabled = true
        tinyCtx.drawImage(img, sx, sy, sw, sh, 0, 0, cols, rows)
        // 画布坐标（逻辑 × dpr）
        const dx = Math.min(m.x1, m.x2) * dpr
        const dy = Math.min(m.y1, m.y2) * dpr
        const dw = Math.abs(m.x2 - m.x1) * dpr
        const dh = Math.abs(m.y2 - m.y1) * dpr
        ctx.save()
        ctx.imageSmoothingEnabled = false
        ctx.drawImage(tiny, 0, 0, cols, rows, dx, dy, dw, dh)
        ctx.restore()
      }
    }
    img.src = imageSrc
    return () => {
      cancelled = true
    }
  }, [imageSrc, annotations, width, height])

  return (
    <canvas
      ref={canvasRef}
      className="absolute inset-0 pointer-events-none"
      style={{ width, height }}
    />
  )
}
