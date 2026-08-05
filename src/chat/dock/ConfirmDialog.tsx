// Dock 内共享的确认对话框（删除文件 / 丢弃变更等破坏性操作二次确认）。
// 样式沿用 Chat.tsx pendingToolConfirm 的固定浮层模式。
import { TriangleAlert } from 'lucide-react'
import { i18n, type Lang } from '../../settings/i18n'

type ConfirmDialogProps = {
  lang: Lang
  title: string
  message: string
  confirmLabel: string
  busy?: boolean
  onConfirm: () => void
  onCancel: () => void
}

export function ConfirmDialog({
  lang,
  title,
  message,
  confirmLabel,
  busy = false,
  onConfirm,
  onCancel,
}: ConfirmDialogProps) {
  const t = i18n[lang]
  return (
    <div className="fixed inset-0 z-[300] flex items-center justify-center bg-black/20 px-4">
      <div className="w-full max-w-sm rounded-lg border border-neutral-200 bg-white p-4 shadow-xl dark:border-neutral-700 dark:bg-neutral-900">
        <div className="mb-3 flex items-start gap-2">
          <TriangleAlert size={17} className="mt-0.5 shrink-0 text-[#C56646] dark:text-[#E39A78]" />
          <div className="min-w-0 flex-1">
            <div className="text-[14px] font-semibold text-neutral-900 dark:text-neutral-100">{title}</div>
            <div className="mt-1 break-all text-[12px] text-neutral-500 dark:text-neutral-400">{message}</div>
          </div>
        </div>
        <div className="flex justify-end gap-2">
          <button
            type="button"
            className="rounded-md px-3 py-1.5 text-[12px] font-medium text-neutral-600 hover:bg-neutral-100 dark:text-neutral-300 dark:hover:bg-neutral-800"
            onClick={onCancel}
            disabled={busy}
          >
            {t.cancel}
          </button>
          <button
            type="button"
            className="rounded-md bg-red-600 px-3 py-1.5 text-[12px] font-medium text-white hover:bg-red-500 disabled:opacity-50"
            onClick={onConfirm}
            disabled={busy}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  )
}
