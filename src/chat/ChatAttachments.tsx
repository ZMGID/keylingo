import { useEffect, useState } from 'react'
import { FileText, X } from 'lucide-react'
import { ChatImageContextMenu, type ChatImageMenuAnchor } from './ChatImageContextMenu'
import { loadAttachmentDataUrl, openAttachment, type DisplayAttachment } from './attachmentPreview'
import { openChatImageViewer } from './imageViewer'

type ChatAttachmentsProps = {
  attachments: DisplayAttachment[]
  conversationId?: string | null
  variant: 'user' | 'assistant' | 'composer'
  onRemove?: (id: string) => void
}

function ImagePreview({
  attachment,
  conversationId,
  variant,
  onPreview,
}: {
  attachment: DisplayAttachment
  conversationId?: string | null
  variant: ChatAttachmentsProps['variant']
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

  const isComposer = variant === 'composer'
  const loadingClass =
    isComposer
      ? 'kv-skeleton h-16 w-16 rounded-lg'
      : 'kv-skeleton min-h-[72px] min-w-[120px] rounded-xl'

  return (
    <div className={isComposer ? 'relative h-16 w-16 overflow-hidden rounded-lg bg-neutral-100 dark:bg-neutral-800' : 'relative inline-block max-w-full'}>
      {loading && <div className={loadingClass} aria-hidden="true" />}
      {!loading && src && (
        <button
          type="button"
          className={
            isComposer
              ? 'chat-motion-fade block h-full w-full cursor-zoom-in rounded-lg p-0'
              : 'chat-motion-fade block max-w-full cursor-zoom-in rounded-xl p-0 text-left'
          }
          onClick={() => onPreview?.(src, attachment.name)}
          // 与模型出图（ChatInlineImage）同一个菜单组件。stopPropagation 是必须的：
          // 滚动容器上挂着消息级右键菜单，不掐断就会被它盖住。
          onContextMenu={(event) => {
            event.preventDefault()
            event.stopPropagation()
            setMenuAnchor({ left: event.clientX, top: event.clientY })
          }}
          title="预览图片"
          aria-label="预览图片"
        >
          <img
            src={src}
            alt=""
            className={
              isComposer
                ? 'h-full w-full rounded-lg object-cover'
                : 'block max-h-72 max-w-[min(100%,420px)] rounded-xl object-contain'
            }
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
        <div className={`${loadingClass} px-4 text-center text-[12px] text-neutral-400`}>
          图片无法预览
        </div>
      )}
    </div>
  )
}

function FileAttachmentChip({
  attachment,
  conversationId,
  variant,
  onRemove,
  removing = false,
  onExited,
}: {
  attachment: DisplayAttachment
  conversationId?: string | null
  variant: ChatAttachmentsProps['variant']
  onRemove?: (id: string) => void
  removing?: boolean
  onExited?: (id: string) => void
}) {
  const chipClass =
    variant === 'composer'
      ? 'inline-flex max-w-[min(100%,13rem)] items-center gap-1 rounded-lg border border-neutral-200/90 bg-neutral-50 py-0.5 pl-1.5 pr-0.5 text-[11px] text-neutral-700 dark:border-neutral-700 dark:bg-neutral-800 dark:text-neutral-200'
      : variant === 'user'
        ? 'flex max-w-full items-center gap-2 rounded-lg bg-black/[0.05] px-2.5 py-2 text-sm text-neutral-700 dark:bg-white/[0.08] dark:text-neutral-200'
        : 'flex max-w-full items-center gap-2 rounded-lg border border-neutral-200/80 px-2.5 py-2 text-sm text-neutral-700 dark:border-neutral-700 dark:text-neutral-200'

  return (
    <div
      className={`${removing ? 'chat-motion-exit' : 'chat-motion-fade-up'} ${chipClass}`}
      onAnimationEnd={
        removing && onExited
          ? (event) => {
              if (event.target === event.currentTarget) onExited(attachment.id)
            }
          : undefined
      }
    >
      <button
        type="button"
        onClick={() => void openAttachment(attachment, conversationId)}
        className={`flex min-w-0 items-center text-left hover:opacity-80 ${variant === 'composer' ? 'gap-1' : 'flex-1 gap-2'}`}
        title={attachment.name}
      >
        <FileText
          size={variant === 'composer' ? 12 : 15}
          strokeWidth={1.8}
          className="shrink-0 text-neutral-500"
        />
        <span className="min-w-0 truncate">{attachment.name}</span>
      </button>
      {onRemove ? (
        <button
          type="button"
          onClick={() => onRemove(attachment.id)}
          className={
            variant === 'composer'
              ? 'flex h-4 w-4 shrink-0 items-center justify-center rounded-full text-neutral-400 hover:bg-black/[0.06] hover:text-neutral-600 dark:hover:bg-white/[0.08] dark:hover:text-neutral-200'
              : 'shrink-0 rounded-full px-1.5 py-0.5 text-[11px] text-neutral-400 hover:bg-black/[0.06] hover:text-neutral-700 dark:hover:bg-white/[0.08] dark:hover:text-neutral-100'
          }
          title="移除"
          aria-label="移除"
        >
          {variant === 'composer' ? <X size={11} strokeWidth={2.4} /> : '移除'}
        </button>
      ) : null}
    </div>
  )
}

export function ChatAttachments({
  attachments,
  conversationId,
  variant,
  onRemove,
}: ChatAttachmentsProps) {
  // 移除中的附件：先打退出动画，animationend 后再真正 onRemove（卸载节点）。
  const [removingIds, setRemovingIds] = useState<ReadonlySet<string>>(() => new Set())

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

  const images = attachments.filter((item) => item.type === 'image')
  const files = attachments.filter((item) => item.type !== 'image')

  return (
    <div className={variant === 'composer' ? 'space-y-1.5' : 'mt-2 space-y-2'}>
      {images.length > 0 && (
        <div className={variant === 'composer' ? 'flex flex-wrap gap-2' : 'flex flex-col gap-2'}>
          {images.map((attachment) => {
            const removing = removingIds.has(attachment.id)
            const baseMotion = removing ? 'chat-motion-exit' : 'chat-motion-fade-up'
            return (
              <div
                key={attachment.id}
                className={
                  variant === 'composer'
                    ? `${baseMotion} group relative h-16 w-16 shrink-0`
                    : `${baseMotion} relative`
                }
                onAnimationEnd={
                  removing
                    ? (event) => {
                        if (event.target === event.currentTarget) finishRemove(attachment.id)
                      }
                    : undefined
                }
              >
                <ImagePreview
                  attachment={attachment}
                  conversationId={conversationId}
                  variant={variant}
                  onPreview={(src, alt) => openChatImageViewer({ src, alt, name: attachment.name })}
                />
                {beginRemove ? (
                  <button
                    type="button"
                    onClick={() => beginRemove(attachment.id)}
                    className={
                      variant === 'composer'
                        ? 'absolute right-1 top-1 flex h-5 w-5 items-center justify-center rounded-full bg-neutral-950/90 text-white opacity-0 shadow-sm transition-opacity duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-out)] hover:bg-neutral-800 focus-visible:opacity-100 group-hover:opacity-100'
                        : 'absolute right-2 top-2 rounded-full bg-black/50 px-2 py-0.5 text-[11px] text-white backdrop-blur-sm hover:bg-black/65'
                    }
                    title="移除图片"
                    aria-label="移除图片"
                  >
                    {variant === 'composer' ? <X size={12} strokeWidth={2.4} /> : '移除'}
                  </button>
                ) : null}
              </div>
            )
          })}
        </div>
      )}
      {files.length > 0 && (
        <div className={variant === 'composer' ? 'flex flex-wrap gap-1.5' : 'flex flex-col gap-1.5'}>
          {files.map((attachment) => (
            <FileAttachmentChip
              key={attachment.id}
              attachment={attachment}
              conversationId={conversationId}
              variant={variant}
              onRemove={beginRemove}
              removing={removingIds.has(attachment.id)}
              onExited={finishRemove}
            />
          ))}
        </div>
      )}
    </div>
  )
}
