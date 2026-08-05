import type { Annotation } from './types'

export const ARROW_COLOR = '#ff3b30'
export const ARROW_HEAD_ANGLE_DEG = 30
export const ARROW_MIN_DRAG_PX = 8
/** 马赛克块大小：相对图片宽度的比例（1/64），最小 8 物理像素 */
export const MOSAIC_BLOCK_RATIO = 1 / 64

function drawArrow(
  ctx: CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D,
  x1: number,
  y1: number,
  x2: number,
  y2: number,
  lineWidth: number,
) {
  const dx = x2 - x1
  const dy = y2 - y1
  const len = Math.hypot(dx, dy)
  if (len < 1) return

  const headSize = lineWidth * 4
  const angle = Math.atan2(dy, dx)
  const headAngle = (ARROW_HEAD_ANGLE_DEG * Math.PI) / 180

  // 箭杆终点回退一格,避免三角覆盖时尾端有缺口
  const shaftEndX = x2 - Math.cos(angle) * (headSize * 0.6)
  const shaftEndY = y2 - Math.sin(angle) * (headSize * 0.6)

  ctx.save()
  ctx.strokeStyle = ARROW_COLOR
  ctx.fillStyle = ARROW_COLOR
  ctx.lineWidth = lineWidth
  ctx.lineCap = 'round'
  ctx.lineJoin = 'round'

  ctx.beginPath()
  ctx.moveTo(x1, y1)
  ctx.lineTo(shaftEndX, shaftEndY)
  ctx.stroke()

  // 三角箭头
  const wing1X = x2 - Math.cos(angle - headAngle) * headSize
  const wing1Y = y2 - Math.sin(angle - headAngle) * headSize
  const wing2X = x2 - Math.cos(angle + headAngle) * headSize
  const wing2Y = y2 - Math.sin(angle + headAngle) * headSize
  ctx.beginPath()
  ctx.moveTo(x2, y2)
  ctx.lineTo(wing1X, wing1Y)
  ctx.lineTo(wing2X, wing2Y)
  ctx.closePath()
  ctx.fill()

  ctx.restore()
}

function drawRect(
  ctx: CanvasRenderingContext2D | OffscreenCanvasRenderingContext2D,
  x1: number,
  y1: number,
  x2: number,
  y2: number,
  lineWidth: number,
) {
  const x = Math.min(x1, x2)
  const y = Math.min(y1, y2)
  const w = Math.abs(x2 - x1)
  const h = Math.abs(y2 - y1)
  if (w < 1 || h < 1) return
  ctx.save()
  ctx.strokeStyle = ARROW_COLOR
  ctx.lineWidth = lineWidth
  ctx.lineJoin = 'round'
  // 圆角矩形：半径与线宽同量级，屏幕预览（SVG rx）与导出观感一致
  const r = Math.min(lineWidth * 1.5, w / 2, h / 2)
  ctx.beginPath()
  ctx.roundRect(x, y, w, h, r)
  ctx.stroke()
  ctx.restore()
}

/**
 * 马赛克：把区域缩小到块数网格再放大回原尺寸（放大关平滑 = 最近邻），经典像素化。
 * 从 source canvas 采样到临时小 canvas，再画回原区域。
 */
function drawMosaic(
  ctx: OffscreenCanvasRenderingContext2D,
  source: OffscreenCanvas,
  x1: number,
  y1: number,
  x2: number,
  y2: number,
  blockSize: number,
) {
  const x = Math.max(0, Math.round(Math.min(x1, x2)))
  const y = Math.max(0, Math.round(Math.min(y1, y2)))
  const w = Math.min(source.width - x, Math.round(Math.abs(x2 - x1)))
  const h = Math.min(source.height - y, Math.round(Math.abs(y2 - y1)))
  if (w < 2 || h < 2) return

  const cols = Math.max(1, Math.round(w / blockSize))
  const rows = Math.max(1, Math.round(h / blockSize))
  const tiny = new OffscreenCanvas(cols, rows)
  const tinyCtx = tiny.getContext('2d')
  if (!tinyCtx) return
  // 缩小采样（平滑开启 = 块内均值近似）
  tinyCtx.imageSmoothingEnabled = true
  tinyCtx.drawImage(source, x, y, w, h, 0, 0, cols, rows)
  // 放大回原区域（平滑关闭 = 硬边块）
  ctx.save()
  ctx.imageSmoothingEnabled = false
  ctx.drawImage(tiny, 0, 0, cols, rows, x, y, w, h)
  ctx.restore()
}

export async function composeAnnotatedImage(
  imageDataUrl: string,
  annotations: Annotation[],
  frameWidth: number,
  frameHeight: number,
): Promise<string> {
  const img = await new Promise<HTMLImageElement>((resolve, reject) => {
    const el = new Image()
    el.onload = () => resolve(el)
    el.onerror = () => reject(new Error('failed to load image for compose'))
    el.src = imageDataUrl
  })

  const canvas = new OffscreenCanvas(img.naturalWidth, img.naturalHeight)
  const ctx = canvas.getContext('2d')
  if (!ctx) throw new Error('OffscreenCanvas 2d context unavailable')

  ctx.drawImage(img, 0, 0)

  // 逻辑像素 → 物理像素的等比缩放
  // capturedFrame.width 是逻辑像素;PNG 是物理像素 → naturalWidth 大于等于 width
  const scaleX = frameWidth > 0 ? img.naturalWidth / frameWidth : 1
  const scaleY = frameHeight > 0 ? img.naturalHeight / frameHeight : 1
  const lineWidth = Math.max(3, img.naturalWidth / 400)
  const mosaicBlock = Math.max(8, img.naturalWidth * MOSAIC_BLOCK_RATIO)

  // 马赛克先画（遮蔽内容在底层），箭头/矩形叠在其上
  for (const a of annotations) {
    if (a.kind !== 'mosaic') continue
    drawMosaic(ctx, canvas, a.x1 * scaleX, a.y1 * scaleY, a.x2 * scaleX, a.y2 * scaleY, mosaicBlock)
  }
  for (const a of annotations) {
    if (a.kind === 'arrow') {
      drawArrow(ctx, a.x1 * scaleX, a.y1 * scaleY, a.x2 * scaleX, a.y2 * scaleY, lineWidth)
    } else if (a.kind === 'rect') {
      drawRect(ctx, a.x1 * scaleX, a.y1 * scaleY, a.x2 * scaleX, a.y2 * scaleY, lineWidth)
    }
  }

  const blob = await canvas.convertToBlob({ type: 'image/png' })
  const buf = await blob.arrayBuffer()
  let binary = ''
  const bytes = new Uint8Array(buf)
  const chunkSize = 0x8000
  for (let i = 0; i < bytes.length; i += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunkSize))
  }
  return btoa(binary)
}
