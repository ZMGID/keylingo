import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { MessageSquarePlus } from 'lucide-react'
import { insertIntoComposer } from './composerInsert'
import type { Lang } from '../settings/i18n'

const BTN_WIDTH = 116

/**
 * 划词浮动按钮：在消息区选中文字后，于选区上方浮出「添加到聊天」，
 * 点击把选中文字作为引用追加到输入框。仅对落在消息气泡（[data-message-id]）内的选区生效。
 */
export function AddSelectionToChat({ containerEl, lang }: { containerEl: HTMLElement | null; lang: Lang }) {
  const [state, setState] = useState<{ text: string; left: number; top: number } | null>(null)
  const stateRef = useRef(state)
  stateRef.current = state

  useEffect(() => {
    if (!containerEl) return

    const resolveFromSelection = () => {
      const sel = window.getSelection()
      const text = sel?.toString().trim() ?? ''
      if (!text || !sel || sel.rangeCount === 0 || sel.isCollapsed) {
        setState(null)
        return
      }
      const anchor = sel.anchorNode
      const el = anchor instanceof Element ? anchor : anchor?.parentElement ?? null
      const inMessage = el?.closest('[data-message-id]')
      if (!inMessage || !containerEl.contains(inMessage)) {
        setState(null)
        return
      }
      const rect = sel.getRangeAt(0).getBoundingClientRect()
      if (rect.width === 0 && rect.height === 0) {
        setState(null)
        return
      }
      setState({ text, left: rect.right, top: rect.top })
    }

    // mouseup 后 selection 才稳定，延一帧再读。
    const onMouseUp = () => setTimeout(resolveFromSelection, 0)
    const onSelectionChange = () => {
      const sel = window.getSelection()
      if (!sel || sel.isCollapsed) setState(null)
    }
    const hide = () => setState(null)

    document.addEventListener('mouseup', onMouseUp)
    document.addEventListener('selectionchange', onSelectionChange)
    // 滚动/切换会话时选区位置失效，直接隐藏。
    containerEl.addEventListener('scroll', hide, true)
    return () => {
      document.removeEventListener('mouseup', onMouseUp)
      document.removeEventListener('selectionchange', onSelectionChange)
      containerEl.removeEventListener('scroll', hide, true)
    }
  }, [containerEl])

  if (!state) return null

  const left = Math.min(Math.max(8, state.left - BTN_WIDTH), window.innerWidth - BTN_WIDTH - 8)
  const top = Math.max(8, state.top - 38)

  return createPortal(
    <button
      type="button"
      className="kv-add-to-chat"
      style={{ position: 'fixed', left, top, zIndex: 60 }}
      // 保住选区：mousedown 默认会清选区并夺焦，阻止它。
      onMouseDown={(e) => e.preventDefault()}
      onClick={() => {
        const text = stateRef.current?.text
        if (text) insertIntoComposer(text)
        window.getSelection()?.removeAllRanges()
        setState(null)
      }}
    >
      <MessageSquarePlus size={13} />
      {lang === 'en' ? 'Add to chat' : '添加到聊天'}
    </button>,
    document.body,
  )
}
