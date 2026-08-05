import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Check, Copy, X } from 'lucide-react'
import { Button, IconButton } from '../components/Button'
import { useT } from '../settings/i18n'

type PastedTextEditorModalProps = {
  name: string
  initialContent: string
  /** 只读查看模式（已发送消息中的虚拟文本附件）：隐藏确定保存，textarea 不可编辑。 */
  readOnly?: boolean
  onSave: (content: string) => void
  onClose: () => void
}

/**
 * 虚拟文本附件（粘贴长文本自动生成的 txt）的编辑/查看弹窗。
 * 编辑模式：确定保存 → 以编辑后内容重建内存 File/Blob（提交时随消息传给后端）；
 * 取消 / 右上角 × / 点击遮罩 → 放弃本次编辑，原始附件数据不变。
 * 查看模式（readOnly）：仅展示内容与复制全部。
 * 通过 portal 挂到 body，使用 .kv-modal-backdrop--portal 的显式 fallback 背景保证对比度。
 */
export function PastedTextEditorModal({
  name,
  initialContent,
  readOnly = false,
  onSave,
  onClose,
}: PastedTextEditorModalProps) {
  const t = useT()
  const [draft, setDraft] = useState(initialContent)
  const [copied, setCopied] = useState(false)
  const textareaRef = useRef<HTMLTextAreaElement>(null)

  useEffect(() => {
    textareaRef.current?.focus()
    textareaRef.current?.setSelectionRange(0, 0)
  }, [])

  // Esc 关闭：放弃编辑
  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [onClose])

  const copyAll = async () => {
    try {
      await navigator.clipboard.writeText(draft)
      setCopied(true)
      window.setTimeout(() => setCopied(false), 1200)
    } catch (err) {
      console.error('Failed to copy pasted text:', err)
    }
  }

  const save = () => {
    onSave(draft)
    onClose()
  }

  return createPortal(
    <div
      className="kv-modal-backdrop kv-modal-backdrop--portal"
      data-tauri-drag-region="false"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose()
      }}
    >
      <div
        className="kv-modal kv-pasted-text-editor"
        data-tauri-drag-region="false"
        onMouseDown={(e) => e.stopPropagation()}
        role="dialog"
        aria-modal="true"
        aria-label={name}
      >
        <div className="flex items-center justify-between gap-3">
          <div className="min-w-0">
            <h3 className="truncate text-[15px] font-semibold">{name}</h3>
            <p className="mt-1 text-[11px] text-neutral-500 dark:text-neutral-400">
              {readOnly ? t.chatPastedTextViewHint : t.chatPastedTextEditHint}
            </p>
          </div>
          <IconButton size="xs" onClick={onClose} label={t.chatPastedTextCancel}>
            <X size={14} />
          </IconButton>
        </div>

        <textarea
          ref={textareaRef}
          value={draft}
          onChange={(e) => setDraft(e.target.value)}
          spellCheck={false}
          readOnly={readOnly}
          aria-readonly={readOnly}
          className="kv-pasted-text-editor-textarea custom-scrollbar"
        />

        <div className="flex items-center justify-between gap-2">
          <Button variant="ghost" size="sm" onClick={() => void copyAll()}>
            {copied ? <Check size={14} /> : <Copy size={14} />}
            {copied ? t.chatPastedTextCopied : t.chatPastedTextCopyAll}
          </Button>
          {readOnly ? (
            <Button variant="ghost" size="sm" onClick={onClose}>
              {t.chatPastedTextCancel}
            </Button>
          ) : (
            <div className="flex items-center gap-2">
              <Button variant="ghost" size="sm" onClick={onClose}>
                {t.chatPastedTextCancel}
              </Button>
              <Button variant="primary" size="sm" onClick={save}>
                {t.chatPastedTextSave}
              </Button>
            </div>
          )}
        </div>
      </div>
    </div>,
    document.body,
  )
}