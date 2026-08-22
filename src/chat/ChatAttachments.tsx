import { useEffect, useState } from 'react'
import {
  File,
  FileArchive,
  FileCode,
  FileJson,
  FileMusic,
  FilePlay,
  FileSpreadsheet,
  FileText,
  ImageOff,
  Presentation,
  X,
} from 'lucide-react'
import { ChatImageContextMenu, type ChatImageMenuAnchor } from './ChatImageContextMenu'
import { loadAttachmentDataUrl, openAttachment, type DisplayAttachment } from './attachmentPreview'
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

type FileKindVisual = {
  Icon: typeof File
  label: string
  iconClass: string
  wellClass: string
}

const CODE_EXTS = new Set([
  'ts', 'tsx', 'mts', 'cts', 'js', 'jsx', 'mjs', 'cjs',
  'py', 'rs', 'go', 'java', 'kt', 'kts', 'c', 'h', 'cc', 'cpp', 'cxx', 'hpp',
  'rb', 'php', 'swift', 'html', 'htm', 'css', 'scss', 'less', 'vue', 'svelte',
  'sh', 'bash', 'zsh', 'ps1', 'bat', 'sql',
])

function extensionOf(name: string): string {
  const slash = Math.max(name.lastIndexOf('/'), name.lastIndexOf('\\'))
  const base = slash >= 0 ? name.slice(slash + 1) : name
  const dot = base.lastIndexOf('.')
  if (dot <= 0 || dot === base.length - 1) return ''
  return base.slice(dot + 1).toLowerCase()
}

function fileKindVisual(name: string): FileKindVisual {
  const ext = extensionOf(name)
  const label = ext ? ext.toUpperCase() : 'FILE'
  if (ext === 'pdf') {
    return { Icon: FileText, label, iconClass: 'text-red-500 dark:text-red-400', wellClass: 'bg-red-500/10 dark:bg-red-400/15' }
  }
  if (ext === 'doc' || ext === 'docx') {
    return { Icon: FileText, label, iconClass: 'text-blue-600 dark:text-blue-400', wellClass: 'bg-blue-500/10 dark:bg-blue-400/15' }
  }
  if (ext === 'xls' || ext === 'xlsx' || ext === 'xlsm' || ext === 'csv' || ext === 'tsv') {
    return { Icon: FileSpreadsheet, label, iconClass: 'text-emerald-600 dark:text-emerald-400', wellClass: 'bg-emerald-500/10 dark:bg-emerald-400/15' }
  }
  if (ext === 'ppt' || ext === 'pptx') {
    return { Icon: Presentation, label, iconClass: 'text-orange-500 dark:text-orange-400', wellClass: 'bg-orange-500/10 dark:bg-orange-400/15' }
  }
  if (ext === 'zip' || ext === 'tar' || ext === 'gz' || ext === '7z' || ext === 'rar') {
    return { Icon: FileArchive, label, iconClass: 'text-amber-600 dark:text-amber-400', wellClass: 'bg-amber-500/10 dark:bg-amber-400/15' }
  }
  if (ext === 'json' || ext === 'jsonc' || ext === 'yaml' || ext === 'yml' || ext === 'toml') {
    return { Icon: FileJson, label, iconClass: 'text-amber-500 dark:text-amber-400', wellClass: 'bg-amber-500/10 dark:bg-amber-400/15' }
  }
  if (ext === 'mp3' || ext === 'wav' || ext === 'm4a' || ext === 'flac' || ext === 'ogg' || ext === 'aac') {
    return { Icon: FileMusic, label, iconClass: 'text-violet-500 dark:text-violet-400', wellClass: 'bg-violet-500/10 dark:bg-violet-400/15' }
  }
  if (ext === 'mp4' || ext === 'mov' || ext === 'webm' || ext === 'mkv' || ext === 'avi') {
    return { Icon: FilePlay, label, iconClass: 'text-fuchsia-500 dark:text-fuchsia-400', wellClass: 'bg-fuchsia-500/10 dark:bg-fuchsia-400/15' }
  }
  if (CODE_EXTS.has(ext)) {
    return { Icon: FileCode, label, iconClass: 'text-sky-500 dark:text-sky-400', wellClass: 'bg-sky-500/10 dark:bg-sky-400/15' }
  }
  if (ext === 'md' || ext === 'markdown' || ext === 'txt' || ext === 'log') {
    return { Icon: FileText, label, iconClass: 'text-neutral-500 dark:text-neutral-300', wellClass: 'bg-neutral-500/10 dark:bg-white/10' }
  }
  return { Icon: File, label, iconClass: 'text-neutral-500 dark:text-neutral-300', wellClass: 'bg-neutral-500/10 dark:bg-white/10' }
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
  const visual = fileKindVisual(attachment.name)
  const Icon = visual.Icon

  return (
    <button
      type="button"
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
      className="flex h-16 w-[9.5rem] items-center gap-2 rounded-lg border border-neutral-200/90 bg-neutral-50 px-1.5 text-left hover:bg-neutral-100 dark:border-neutral-700 dark:bg-neutral-800 dark:hover:bg-neutral-700/80"
      title={attachment.name}
    >
      <span className={`flex h-11 w-11 shrink-0 items-center justify-center rounded-md ${visual.wellClass}`}>
        <Icon size={18} strokeWidth={1.8} className={visual.iconClass} />
      </span>
      <span className="min-w-0 flex-1">
        <span className="block truncate text-[12px] font-medium leading-tight text-neutral-800 dark:text-neutral-100">
          {attachment.name}
        </span>
        <span className="mt-0.5 block truncate text-[10px] font-medium uppercase tracking-wide text-neutral-400 dark:text-neutral-500">
          {visual.label}
        </span>
      </span>
    </button>
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
