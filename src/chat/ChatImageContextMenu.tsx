import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { save } from '@tauri-apps/plugin-dialog'
import { Check, Clipboard, Download, Maximize2 } from 'lucide-react'
import { api } from '../api/tauri'
import { useCloseAnimation } from './useCloseAnimation'
import { useClampedMenuPosition } from './useClampedMenuPosition'
import { base64FromDataUrl, imageExtension } from './imageData'

export interface ChatImageMenuAnchor {
  left: number
  top: number
}

interface ChatImageContextMenuProps {
  anchor: ChatImageMenuAnchor
  /** 图片的 data URL（复制/另存都从它取字节）。 */
  src: string
  name?: string
  onOpenViewer?: () => void
  onClose: () => void
}

export function ChatImageContextMenu({
  anchor,
  src,
  name,
  onOpenViewer,
  onClose: onCloseProp,
}: ChatImageContextMenuProps) {
  const menuRef = useRef<HTMLDivElement>(null)
  const pos = useClampedMenuPosition(menuRef, anchor)
  const { closing, startClose, onAnimationEnd } = useCloseAnimation(onCloseProp)
  const onClose = startClose
  const [copied, setCopied] = useState(false)
  const base64 = base64FromDataUrl(src)

  useEffect(() => {
    const onPointerDown = (e: MouseEvent) => {
      if (menuRef.current?.contains(e.target as Node)) return
      onClose()
    }
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') onClose()
    }
    window.addEventListener('mousedown', onPointerDown)
    window.addEventListener('keydown', onKeyDown)
    return () => {
      window.removeEventListener('mousedown', onPointerDown)
      window.removeEventListener('keydown', onKeyDown)
    }
  }, [onClose])

  const handleCopy = async () => {
    if (!base64) return
    // 复用 Lens 标注早就有的剪贴板写图命令（解码 → arboard set_image），不另造一条。
    const result = await api.lensCopyImageToClipboard(base64)
    if (!result.success) {
      window.alert(`复制失败：${result.error ?? '未知错误'}`)
      return
    }
    setCopied(true)
    window.setTimeout(onClose, 600)
  }

  const handleSave = async () => {
    if (!base64) return
    const ext = imageExtension(src, name)
    const path = await save({
      defaultPath: name || `image.${ext}`,
      filters: [{ name: 'Image', extensions: [ext] }],
    })
    if (!path) return
    const result = await api.lensSaveAnnotatedPng(base64, path)
    if (!result.success) window.alert(`保存失败：${result.error ?? '未知错误'}`)
    onClose()
  }

  const itemClass =
    'flex w-full items-center gap-3 px-3.5 py-2 text-left text-[13px] text-neutral-800 transition-colors hover:bg-black/[0.04] disabled:cursor-not-allowed disabled:opacity-40 dark:text-neutral-100 dark:hover:bg-white/[0.06]'
  const iconClass = 'shrink-0 text-neutral-500'

  const menu = (
    <div
      ref={menuRef}
      className={`${closing ? 'chat-motion-popover-out' : 'chat-motion-popover chat-motion-menu-cascade'} fixed z-[200] min-w-[176px] rounded-xl border border-neutral-200/90 bg-white py-1.5 shadow-lg dark:border-neutral-700 dark:bg-[#2a2a2c]`}
      style={{ left: pos.left, top: pos.top }}
      role="menu"
      onAnimationEnd={onAnimationEnd}
    >
      <button
        type="button"
        role="menuitem"
        className={itemClass}
        disabled={!base64}
        onClick={() => void handleCopy()}
      >
        {copied ? (
          <Check size={16} strokeWidth={2} className="shrink-0 text-neutral-500 chat-motion-pop" />
        ) : (
          <Clipboard size={16} strokeWidth={1.75} className={iconClass} />
        )}
        {copied ? '已复制图片' : '复制图片'}
      </button>
      <button
        type="button"
        role="menuitem"
        className={itemClass}
        disabled={!base64}
        onClick={() => void handleSave()}
      >
        <Download size={16} strokeWidth={1.75} className={iconClass} />
        图片另存为…
      </button>
      {onOpenViewer ? (
        <button
          type="button"
          role="menuitem"
          className={itemClass}
          onClick={() => {
            onOpenViewer()
            onClose()
          }}
        >
          <Maximize2 size={16} strokeWidth={1.75} className={iconClass} />
          查看大图
        </button>
      ) : null}
    </div>
  )

  return createPortal(menu, document.body)
}
