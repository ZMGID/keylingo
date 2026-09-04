import { useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { ClipboardPaste, Copy, Scissors, TextSelect } from 'lucide-react'
import { useT } from './i18n'

export type TextEditAnchor = {
  left: number
  top: number
}

export function TextEditContextMenu({
  anchor,
  hasSelection,
  onCut,
  onCopy,
  onPaste,
  onSelectAll,
  onClose,
}: {
  anchor: TextEditAnchor
  hasSelection: boolean
  onCut: () => void
  onCopy: () => void
  onPaste: () => void
  onSelectAll: () => void
  onClose: () => void
}) {
  const t = useT()
  const menuRef = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState(anchor)

  useLayoutEffect(() => {
    const el = menuRef.current
    if (!el) return
    const rect = el.getBoundingClientRect()
    const margin = 8
    setPos({
      left: Math.max(margin, Math.min(anchor.left, window.innerWidth - rect.width - margin)),
      top: Math.max(margin, Math.min(anchor.top, window.innerHeight - rect.height - margin)),
    })
  }, [anchor.left, anchor.top])

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

  const run = (action: () => void) => {
    onClose()
    action()
  }

  return createPortal(
    <div
      ref={menuRef}
      className="kv-menu chat-motion-popover fixed z-[1000] min-w-[168px]"
      style={{ left: pos.left, top: pos.top }}
      role="menu"
      data-tauri-drag-region="false"
    >
      <button
        type="button"
        role="menuitem"
        className="kv-menu-item"
        disabled={!hasSelection}
        onClick={() => run(onCut)}
      >
        <Scissors strokeWidth={1.75} />
        {t.editCut}
      </button>
      <button
        type="button"
        role="menuitem"
        className="kv-menu-item"
        disabled={!hasSelection}
        onClick={() => run(onCopy)}
      >
        <Copy strokeWidth={1.75} />
        {t.editCopy}
      </button>
      <button
        type="button"
        role="menuitem"
        className="kv-menu-item"
        onClick={() => run(onPaste)}
      >
        <ClipboardPaste strokeWidth={1.75} />
        {t.editPaste}
      </button>
      <button
        type="button"
        role="menuitem"
        className="kv-menu-item"
        onClick={() => run(onSelectAll)}
      >
        <TextSelect strokeWidth={1.75} />
        {t.editSelectAll}
      </button>
    </div>,
    document.body,
  )
}
