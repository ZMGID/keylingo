import { useEffect, useState } from 'react'
import { Check, Copy, Grid3x3, Loader2, MoveUpRight, Save, Square, Undo2 } from 'lucide-react'
import type { AnnotationKind } from './types'

export type AnnotateToolbarLabels = {
  arrow: string
  rect: string
  mosaic: string
  undo: string
  copy: string
  copied: string
  save: string
  saving: string
}

/**
 * 截图标注工具栏：截完图后出现在选区下方（放不下则上方）。
 * 工具三选一（箭头/矩形/马赛克）+ 撤销 + 复制/保存两个出口。
 * 入场动画：从选区方向轻微位移 + scale + fade（cubic-bezier 同 Lens 全局曲线）。
 */
export function AnnotateToolbar({
  x,
  y,
  placeAbove,
  tool,
  onToolChange,
  canUndo,
  onUndo,
  onCopy,
  onSave,
  copied,
  saving,
  labels,
}: {
  x: number
  y: number
  placeAbove: boolean
  tool: AnnotationKind
  onToolChange: (t: AnnotationKind) => void
  canUndo: boolean
  onUndo: () => void
  onCopy: () => void
  onSave: () => void
  copied: boolean
  saving: boolean
  labels: AnnotateToolbarLabels
}) {
  // 入场：mount 后下一帧翻转，让 transform/opacity 过渡播放
  const [entered, setEntered] = useState(false)
  useEffect(() => {
    const raf = requestAnimationFrame(() => setEntered(true))
    return () => cancelAnimationFrame(raf)
  }, [])

  const tools: Array<{ kind: AnnotationKind; icon: typeof MoveUpRight; label: string }> = [
    { kind: 'arrow', icon: MoveUpRight, label: labels.arrow },
    { kind: 'rect', icon: Square, label: labels.rect },
    { kind: 'mosaic', icon: Grid3x3, label: labels.mosaic },
  ]

  return (
    <div
      className="absolute z-[45]"
      style={{
        left: x,
        top: y,
        transform: entered
          ? 'translateY(0) scale(1)'
          : `translateY(${placeAbove ? 8 : -8}px) scale(0.92)`,
        opacity: entered ? 1 : 0,
        transition: 'transform 260ms cubic-bezier(0.22, 1, 0.36, 1), opacity 200ms ease-out',
        willChange: 'transform, opacity',
      }}
      onMouseDown={(e) => e.stopPropagation()}
      onMouseMove={(e) => e.stopPropagation()}
      onMouseUp={(e) => e.stopPropagation()}
      onClick={(e) => e.stopPropagation()}
    >
      <div className="flex items-center gap-1 px-1.5 py-1.5 rounded-[14px] bg-white dark:bg-neutral-900 border border-black/[0.07] dark:border-white/[0.08] lens-floating-surface">
        {tools.map(({ kind, icon: Icon, label }) => {
          const active = tool === kind
          return (
            <button
              key={kind}
              type="button"
              title={label}
              onClick={() => onToolChange(kind)}
              className={`w-9 h-9 rounded-[10px] flex items-center justify-center transition-all duration-150 ${
                active
                  ? 'bg-neutral-900 text-white dark:bg-white dark:text-neutral-900 scale-100 shadow-sm'
                  : 'text-neutral-600 dark:text-neutral-300 hover:bg-black/[0.05] dark:hover:bg-white/[0.06] active:scale-90'
              }`}
            >
              <Icon size={16} strokeWidth={1.75} />
            </button>
          )
        })}

        <div className="w-px h-5 mx-0.5 bg-black/[0.08] dark:bg-white/[0.1]" />

        <button
          type="button"
          title={labels.undo}
          onClick={onUndo}
          disabled={!canUndo}
          className={`w-9 h-9 rounded-[10px] flex items-center justify-center transition-all duration-150 ${
            canUndo
              ? 'text-neutral-600 dark:text-neutral-300 hover:bg-black/[0.05] dark:hover:bg-white/[0.06] active:scale-90'
              : 'text-neutral-300 dark:text-neutral-600 cursor-not-allowed'
          }`}
        >
          <Undo2 size={16} strokeWidth={1.75} />
        </button>

        <div className="w-px h-5 mx-0.5 bg-black/[0.08] dark:bg-white/[0.1]" />

        <button
          type="button"
          title={labels.save}
          onClick={onSave}
          disabled={saving}
          className="h-9 px-3 rounded-[10px] flex items-center gap-1.5 text-[13px] font-medium text-neutral-700 dark:text-neutral-200 hover:bg-black/[0.05] dark:hover:bg-white/[0.06] active:scale-95 transition-all duration-150 disabled:opacity-50"
        >
          {saving ? <Loader2 size={15} strokeWidth={1.75} className="animate-spin" /> : <Save size={15} strokeWidth={1.75} />}
          <span>{saving ? labels.saving : labels.save}</span>
        </button>

        <button
          type="button"
          title={labels.copy}
          onClick={onCopy}
          className={`h-9 px-3.5 rounded-[10px] flex items-center gap-1.5 text-[13px] font-medium transition-all duration-150 active:scale-95 ${
            copied
              ? 'bg-green-500 text-white'
              : 'bg-neutral-900 text-white hover:bg-neutral-700 dark:bg-white dark:text-neutral-900 dark:hover:bg-neutral-200'
          }`}
        >
          {copied ? <Check size={15} strokeWidth={2} /> : <Copy size={15} strokeWidth={1.75} />}
          <span>{copied ? labels.copied : labels.copy}</span>
        </button>
      </div>
    </div>
  )
}
