import type { ExplainMessage } from '../api/tauri'

export type Stage = 'select' | 'ready' | 'answering' | 'translating' | 'translated'
export type Mode = 'chat' | 'translate' | 'translateText' | 'replace' | 'screenshot'
export type Point = { x: number; y: number }
export type BarRect = { x: number; y: number; width: number }
export type CapturedFrame = { x: number; y: number; width: number; height: number; label: string }
export type TranslateCardDrag = { pointerId: number; startX: number; startY: number; startRect: BarRect }
export type AnnotationKind = 'arrow' | 'rect' | 'mosaic'
/** 标注元素：全部是两点拖拽（起点/终点），同一坐标系（capturedFrame 逻辑像素） */
export type Annotation = {
  kind: AnnotationKind
  x1: number
  y1: number
  x2: number
  y2: number
}
/** 旧名兼容：Lens chat 模式的箭头标注沿用 */
export type Arrow = Annotation

export type HistoryItem = {
  id: string
  imagePreview: string
  appLabel: string
  messages: ExplainMessage[]
  capturedFrame: CapturedFrame | null
  timestamp: number
}

export type Metrics = {
  READY_W: number
  SELECT_W: number
  ANSWER_H: number
  SELECT_BOTTOM_OFFSET: number
}
