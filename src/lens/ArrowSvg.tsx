import { ARROW_COLOR, ARROW_HEAD_ANGLE_DEG } from './annotation'
import type { Annotation } from './types'

/** 箭头/矩形的 SVG 实时预览。马赛克由独立的 canvas 预览层渲染（需要位图采样），此组件画半透明占位。 */
export function AnnotationSvg({ annotation }: { annotation: Annotation }) {
  const { kind, x1, y1, x2, y2 } = annotation

  if (kind === 'rect' || kind === 'mosaic') {
    const x = Math.min(x1, x2)
    const y = Math.min(y1, y2)
    const w = Math.abs(x2 - x1)
    const h = Math.abs(y2 - y1)
    if (w < 1 || h < 1) return null
    if (kind === 'mosaic') {
      // 拖拽中的马赛克草稿：毛玻璃感占位（真正像素化由 MosaicCanvas 层在落定后渲染）
      return (
        <rect
          x={x}
          y={y}
          width={w}
          height={h}
          rx={3}
          fill="rgba(120,120,128,0.35)"
          stroke="rgba(255,255,255,0.8)"
          strokeWidth={1.5}
          strokeDasharray="6 4"
        />
      )
    }
    return (
      <rect
        x={x}
        y={y}
        width={w}
        height={h}
        rx={6}
        fill="none"
        stroke={ARROW_COLOR}
        strokeWidth={4}
        strokeLinejoin="round"
      />
    )
  }

  const dx = x2 - x1
  const dy = y2 - y1
  const len = Math.hypot(dx, dy)
  if (len < 1) return null

  // SVG 在逻辑像素坐标系下渲染 → 线宽用屏幕粗细,合成时再按 PNG 物理像素重算
  const lineWidth = 4
  const headSize = lineWidth * 4
  const angle = Math.atan2(dy, dx)
  const headAngle = (ARROW_HEAD_ANGLE_DEG * Math.PI) / 180

  const shaftEndX = x2 - Math.cos(angle) * (headSize * 0.6)
  const shaftEndY = y2 - Math.sin(angle) * (headSize * 0.6)
  const wing1X = x2 - Math.cos(angle - headAngle) * headSize
  const wing1Y = y2 - Math.sin(angle - headAngle) * headSize
  const wing2X = x2 - Math.cos(angle + headAngle) * headSize
  const wing2Y = y2 - Math.sin(angle + headAngle) * headSize

  return (
    <g>
      <line
        x1={x1}
        y1={y1}
        x2={shaftEndX}
        y2={shaftEndY}
        stroke={ARROW_COLOR}
        strokeWidth={lineWidth}
        strokeLinecap="round"
      />
      <polygon
        points={`${x2},${y2} ${wing1X},${wing1Y} ${wing2X},${wing2Y}`}
        fill={ARROW_COLOR}
      />
    </g>
  )
}

/** 旧名兼容：Lens chat 模式沿用 */
export function ArrowSvg({ arrow }: { arrow: Annotation }) {
  return <AnnotationSvg annotation={arrow} />
}
