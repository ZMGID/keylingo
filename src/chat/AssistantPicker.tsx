// 会话「助手/专家」选择器：底栏图标 + 弹层。列出已配置专家，点选即应用到当前会话
// （无会话则以该专家开新对话）；底部「管理 / 创建专家」跳 AssistantCenter 整页。
// 弹层贴按钮、紧凑宽度，不再铺满输入框。
import { useCallback, useEffect, useRef, useState } from 'react'
import { Bot, Check, Settings2 } from 'lucide-react'
import { chatApi } from './api'
import { api } from '../api/tauri'
import { builtinAssistantGlyph } from './assistantIcons'
import { IconButton } from '../components/Button'
import { usePopoverMaxHeight } from './usePopoverMaxHeight'
import type { ChatAssistant } from './types'

export function AssistantPicker({
  currentAssistant,
  onSelect,
  onOpenCenter,
  disabled,
  layout = 'footer',
}: {
  currentAssistant: { id: string; name: string } | null
  onSelect: (assistant: ChatAssistant | null) => void | Promise<void>
  onOpenCenter: () => void
  disabled?: boolean
  layout?: 'footer' | 'inline'
}) {
  const [open, setOpen] = useState(false)
  const [assistants, setAssistants] = useState<ChatAssistant[]>([])
  const ref = useRef<HTMLDivElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)

  const load = useCallback(async () => {
    try {
      // 对话栏只列「常用」专家（内置默认不在常用，需在专家中心的套件广场添加）。
      const all = await chatApi.getAssistants()
      setAssistants(all.filter((a) => (a.installed ?? true) !== false))
    } catch {
      /* ignore */
    }
  }, [])

  const loadRef = useRef(load)
  loadRef.current = load

  useEffect(() => {
    void loadRef.current()
    let cancelled = false
    let unlisten: (() => void) | undefined
    void api.onChatAssistantsChanged(() => void loadRef.current()).then((fn) => {
      if (cancelled) fn()
      else unlisten = fn
    })
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [])

  useEffect(() => {
    if (open) void loadRef.current()
  }, [open])

  useEffect(() => {
    if (!open) return
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node
      if (ref.current?.contains(t)) return
      setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [open])

  const placement = layout === 'inline' ? 'top-full mt-1.5' : 'bottom-full mb-1.5'
  const origin = layout === 'inline' ? 'top left' : 'bottom left'
  const maxH = usePopoverMaxHeight(open, popoverRef, layout === 'inline' ? 'down' : 'up', 320)

  const pick = (assistant: ChatAssistant | null) => {
    setOpen(false)
    void onSelect(assistant)
  }

  return (
    <div className="relative shrink-0" ref={ref}>
      <IconButton
        size="sm"
        shape="circle"
        disabled={disabled}
        onClick={() => setOpen((v) => !v)}
        className={`focus-visible:ring-2 focus-visible:ring-neutral-300/60 dark:focus-visible:ring-neutral-600 ${
          open
            ? 'bg-neutral-200 text-neutral-700 dark:bg-neutral-700 dark:text-neutral-100'
            : currentAssistant
              ? 'text-indigo-500! hover:bg-neutral-100 dark:text-indigo-300! dark:hover:bg-neutral-800'
              : 'text-neutral-500 hover:bg-neutral-100 dark:text-neutral-400 dark:hover:bg-neutral-800'
        }`}
        aria-expanded={open}
        aria-haspopup="menu"
        label={currentAssistant ? `专家 · ${currentAssistant.name}` : '选择或创建专家'}
        title={currentAssistant ? `专家 · ${currentAssistant.name}` : '选择或创建专家'}
      >
        {currentAssistant
          ? builtinAssistantGlyph(currentAssistant.id, 18) ?? <Bot size={18} strokeWidth={1.75} />
          : <Bot size={18} strokeWidth={1.75} />}
      </IconButton>
      {open && (
        <div
          ref={popoverRef}
          className={`chat-motion-popover chat-popover-scroll absolute left-0 z-50 w-[min(240px,calc(100vw-24px))] overflow-y-auto kv-menu ${placement}`}
          style={{ ['--chat-popover-origin' as string]: origin, maxHeight: maxH }}
          data-tauri-drag-region="false"
          role="menu"
        >
          {currentAssistant && (
            <button
              type="button"
              onClick={() => pick(null)}
              className="kv-menu-item"
            >
              <span className="grid size-4 shrink-0 place-items-center">
                <Bot size={13} strokeWidth={1.75} />
              </span>
              不使用专家
            </button>
          )}
          {assistants.length === 0 ? (
            <p className="px-2 py-2 text-[11px] text-neutral-500">还没有专家，点下方创建。</p>
          ) : (
            assistants.map((assistant) => {
              const active = assistant.id === currentAssistant?.id
              return (
                <button
                  key={assistant.id}
                  type="button"
                  onClick={() => pick(assistant)}
                  className={`kv-menu-row transition-colors ${
                    active
                      ? 'bg-neutral-100 font-medium text-neutral-900 dark:bg-neutral-800 dark:text-neutral-100'
                      : 'text-neutral-700 hover:bg-neutral-100 dark:text-neutral-200 dark:hover:bg-neutral-800'
                  }`}
                >
                  <span className="grid size-4 shrink-0 place-items-center text-indigo-500 dark:text-indigo-300">
                    {builtinAssistantGlyph(assistant.id, 14) ?? <Bot size={13} strokeWidth={1.75} />}
                  </span>
                  <span className="min-w-0 flex-1 truncate">{assistant.name}</span>
                  {active && <Check size={12} strokeWidth={2.5} className="shrink-0 text-indigo-500 dark:text-indigo-300" />}
                </button>
              )
            })
          )}
          <div className="my-1 border-t border-neutral-200/80 dark:border-neutral-800" />
          <button
            type="button"
            onClick={() => {
              setOpen(false)
              onOpenCenter()
            }}
            className="kv-menu-item"
          >
            <span className="grid size-4 shrink-0 place-items-center">
              <Settings2 size={13} strokeWidth={1.75} />
            </span>
            管理 / 创建专家
          </button>
        </div>
      )}
    </div>
  )
}
