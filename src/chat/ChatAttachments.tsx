import { useEffect, useState } from 'react'
import { ImageOff, X } from 'lucide-react'
import { ChatImageContextMenu, type ChatImageMenuAnchor } from './ChatImageContextMenu'
import { loadAttachmentDataUrl, openAttachment, type DisplayAttachment } from './attachmentPreview'
import { FileChip } from './fileChip'
import { openChatImageViewer } from './imageViewer'
import { PastedTextEditorModal } from './PastedTextEditorModal'
import { useT } from '../settings/i18n'

type ChatAttachmentsProps = {
  attachments: DisplayAttachment[]
  conversationId?: string | null
  variant: 'user' | 'assistant' | 'composer'
  onRemove?: (id: string) => void
  /** 内存文本附件（粘贴长文本虚拟 txt）点击时回调，用于打开编辑弹窗。 */
  onEditAttachment?: (attachment: DisplayAttachment) => void
}

/**
 * 输入框 / 已发送消息共用同一套 64px 高卡片：图是方缩略图，文件是类型色块 + 文件名。
 * 发送后不再把图撑回原尺寸，点开才进查看器。
 */
function ImagePreview({
  attachment,
  conversationId,
  previewLabel,
  failedLabel,
  onPreview,
}: {
  attachment: DisplayAttachment
  conversationId?: string | null
  previewLabel: string
  failedLabel: string
  onPreview?: (src: string, alt: string) => void
}) {
  const [src, setSrc] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [failed, setFailed] = useState(false)
  const [menuAnchor, setMenuAnchor] = useState<ChatImageMenuAnchor | null>(null)

  useEffect(() => {
    let cancelled = false
    setLoading(true)
    setFailed(false)
    setSrc(null)

    void loadAttachmentDataUrl(attachment, conversationId).then((dataUrl) => {
      if (cancelled) return
      if (dataUrl) {
        setSrc(dataUrl)
      } else {
        setFailed(true)
      }
      setLoading(false)
    })

    return () => {
      cancelled = true
    }
  }, [attachment, conversationId])

  return (
    <div className="relative h-16 w-16 overflow-hidden rounded-lg bg-neutral-100 dark:bg-neutral-800">
      {loading && <div className="kv-skeleton h-16 w-16 rounded-lg" aria-hidden="true" />}
      {!loading && src && (
        <button
          type="button"
          className="block h-full w-full cursor-zoom-in rounded-lg p-0"
          onClick={() => onPreview?.(src, attachment.name)}
          // 与模型出图（ChatInlineImage）同一个菜单组件。stopPropagation 是必须的：
          // 滚动容器上挂着消息级右键菜单，不掐断就会被它盖住。
          onContextMenu={(event) => {
            event.preventDefault()
            event.stopPropagation()
            setMenuAnchor({ left: event.clientX, top: event.clientY })
          }}
          title={attachment.name}
          aria-label={previewLabel}
        >
          <img
            src={src}
            alt=""
            className="h-full w-full rounded-lg object-cover"
            loading="lazy"
          />
        </button>
      )}
      {menuAnchor && src ? (
        <ChatImageContextMenu
          anchor={menuAnchor}
          src={src}
          name={attachment.name}
          onOpenViewer={() => onPreview?.(src, attachment.name)}
          onClose={() => setMenuAnchor(null)}
        />
      ) : null}
      {!loading && failed && (
        <div
          className="flex h-16 w-16 items-center justify-center text-neutral-400"
          title={failedLabel}
        >
          <ImageOff size={16} strokeWidth={1.8} />
          <span className="sr-only">{failedLabel}</span>
        </div>
      )}
    </div>
  )
}

function FileAttachmentCard({
  attachment,
  conversationId,
  onEdit,
  onViewText,
}: {
  attachment: DisplayAttachment
  conversationId?: string | null
  onEdit?: (attachment: DisplayAttachment) => void
  /** 已发送消息中的虚拟文本附件（memory://）：打开只读查看弹窗。 */
  onViewText?: (name: string, content: string) => void
}) {
  return (
    <FileChip
      name={attachment.name}
      onClick={() => {
        if (typeof attachment.content === 'string' && onEdit) {
          onEdit(attachment)
          return
        }
        if (attachment.path.startsWith('memory://')) {
          if (typeof attachment.content === 'string' && onViewText) {
            onViewText(attachment.name, attachment.content)
          }
          return
        }
        void openAttachment(attachment, conversationId)
      }}
    />
  )
}

function RemoveButton({
  label,
  onClick,
}: {
  label: string
  onClick: () => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="absolute right-1 top-1 z-10 flex h-5 w-5 items-center justify-center rounded-full bg-neutral-950/90 text-white opacity-0 shadow-sm transition-opacity duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-out)] hover:bg-neutral-800 focus-visible:opacity-100 group-hover:opacity-100"
      title={label}
      aria-label={label}
    >
      <X size={12} strokeWidth={2.4} />
    </button>
  )
}

export function ChatAttachments({
  attachments,
  conversationId,
  variant,
  onRemove,
  onEditAttachment,
}: ChatAttachmentsProps) {
  const t = useT()
  // 移除中的附件：先打退出动画，animationend 后再真正 onRemove（卸载节点）。
  const [removingIds, setRemovingIds] = useState<ReadonlySet<string>>(() => new Set())
  // 已发送消息中虚拟文本附件的只读查看弹窗。
  const [viewingText, setViewingText] = useState<{ name: string; content: string } | null>(null)

  const beginRemove = onRemove
    ? (id: string) => setRemovingIds((prev) => new Set(prev).add(id))
    : undefined
  const finishRemove = (id: string) => {
    setRemovingIds((prev) => {
      if (!prev.has(id)) return prev
      const next = new Set(prev)
      next.delete(id)
      return next
    })
    onRemove?.(id)
  }

  if (attachments.length === 0) return null

  const wrapClass =
    variant === 'composer'
      ? 'flex flex-wrap gap-2'
      : variant === 'user'
        ? 'flex min-w-0 max-w-full flex-wrap justify-end gap-2'
        : 'mt-2 flex min-w-0 max-w-full flex-wrap gap-2'

  return (
    <div className={wrapClass}>
      {attachments.map((attachment) => {
        const removing = removingIds.has(attachment.id)
        const motion = removing
          ? 'chat-motion-exit'
          : variant === 'composer'
            ? 'chat-motion-fade-up'
            : ''
        return (
          <div
            key={attachment.id}
            className={`${motion} group relative shrink-0`}
            onAnimationEnd={
              removing
                ? (event) => {
                    if (event.target === event.currentTarget) finishRemove(attachment.id)
                  }
                : undefined
            }
          >
            {attachment.type === 'image' ? (
              <ImagePreview
                attachment={attachment}
                conversationId={conversationId}
                previewLabel={t.chatPreviewImage}
                failedLabel={t.chatImagePreviewFailed}
                onPreview={(src, alt) => openChatImageViewer({ src, alt, name: attachment.name })}
              />
            ) : (
              <FileAttachmentCard
                attachment={attachment}
                conversationId={conversationId}
                onEdit={onEditAttachment}
                onViewText={(name, content) => setViewingText({ name, content })}
              />
            )}
            {beginRemove ? (
              <RemoveButton
                label={t.chatRemoveAttachment}
                onClick={() => beginRemove(attachment.id)}
              />
            ) : null}
          </div>
        )
      })}

      {viewingText && (
        <PastedTextEditorModal
          name={viewingText.name}
          initialContent={viewingText.content}
          readOnly
          onSave={() => {}}
          onClose={() => setViewingText(null)}
        />
      )}
    </div>
  )
}
