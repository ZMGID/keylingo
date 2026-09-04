// 会话「助手/专家」选择器：列出已配置专家，点选即应用到当前会话
// （无会话则以该专家开新对话）；底部「管理 / 创建专家」跳 AssistantCenter 整页。
// 嵌在加号菜单的二级页里。
import { useCallback, useContext, useEffect, useRef, useState } from 'react'
import { Bot, Check, Settings2 } from 'lucide-react'
import { useT } from '../settings/i18n'
import { chatApi } from './api'
import { api } from '../api/tauri'
import { builtinAssistantGlyph } from './assistantIcons'
import { ComposerAddMenuCloseContext } from './composerAddMenuContext'
import type { ChatAssistant } from './types'

export function AssistantPicker({
  currentAssistant,
  onSelect,
  onOpenCenter,
}: {
  currentAssistant: { id: string; name: string } | null
  onSelect: (assistant: ChatAssistant | null) => void | Promise<void>
  onOpenCenter: () => void
}) {
  const t = useT()
  const closeAddMenu = useContext(ComposerAddMenuCloseContext)
  const [assistants, setAssistants] = useState<ChatAssistant[]>([])

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

  const pick = (assistant: ChatAssistant | null) => {
    closeAddMenu?.()
    void onSelect(assistant)
  }

  return (
    <>
      {currentAssistant && (
        <button
          type="button"
          onClick={() => pick(null)}
          className="kv-menu-item"
        >
          <span className="grid size-4 shrink-0 place-items-center">
            <Bot size={13} strokeWidth={1.75} />
          </span>
          {t.chatNoAssistant}
        </button>
      )}
      {assistants.length === 0 ? (
        <p className="px-2 py-2 text-[11px] text-neutral-500">{t.chatNoAssistantsYet}</p>
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
          closeAddMenu?.()
          onOpenCenter()
        }}
        className="kv-menu-item"
      >
        <span className="grid size-4 shrink-0 place-items-center">
          <Settings2 size={13} strokeWidth={1.75} />
        </span>
        {t.chatManageAssistants}
      </button>
    </>
  )
}
