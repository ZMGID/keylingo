import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Select } from '../settings/components'
import type { ChatAssistant, ChatSet } from './types'
import { useCloseAnimation } from './useCloseAnimation'

// ponytail: same palette as AssistantCenter so 集/助手 colors match; duplicated 6-element array, not worth sharing
const setColors = ['#6A8FBD', '#2f6ff0', '#4F9D7A', '#8A6FBD', '#B7791F', '#5E8C6A']

interface SetDialogProps {
  set?: ChatSet | null
  assistants: ChatAssistant[]
  saving?: boolean
  error?: string
  onSave: (
    name: string,
    systemPrompt: string,
    defaultAssistantId: string | null,
    color: string | null,
  ) => void
  onClose: () => void
}

export function SetDialog({
  set,
  assistants,
  saving = false,
  error = '',
  onSave,
  onClose,
}: SetDialogProps) {
  const [name, setName] = useState(set?.name ?? '')
  const [systemPrompt, setSystemPrompt] = useState(
    set?.system_prompt ?? set?.systemPrompt ?? '',
  )
  const [defaultAssistantId, setDefaultAssistantId] = useState(
    set?.default_assistant_id ?? set?.defaultAssistantId ?? '',
  )
  const [color, setColor] = useState<string | null>(set?.color ?? null)
  const inputRef = useRef<HTMLInputElement>(null)
  const title = set ? '编辑集' : '新建集'
  const { closing, startClose, onAnimationEnd } = useCloseAnimation(onClose)

  useEffect(() => {
    inputRef.current?.focus()
    inputRef.current?.select()
  }, [])

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') startClose()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [startClose])

  const submit = () => {
    const nextName = name.trim()
    if (!nextName || saving) return
    onSave(nextName, systemPrompt.trim(), defaultAssistantId.trim() || null, color)
  }

  const selectableAssistants = assistants.filter((a) => !a.archived)

  return createPortal(
    <div
      className={`${closing ? 'chat-motion-fade-out' : 'chat-motion-fade'} fixed inset-0 z-[300] flex items-center justify-center bg-black/30 px-4 backdrop-blur-[1px]`}
      data-tauri-drag-region="false"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) startClose()
      }}
    >
      <form
        className={`${closing ? 'chat-motion-modal-out' : 'chat-motion-modal-in'} w-full max-w-[380px] rounded-[10px] border border-neutral-200 bg-white p-4 shadow-xl dark:border-neutral-700 dark:bg-[#252527]`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onAnimationEnd={onAnimationEnd}
        onSubmit={(e) => {
          e.preventDefault()
          submit()
        }}
      >
        <h3 className="text-[14px] font-semibold text-neutral-900 dark:text-neutral-50">{title}</h3>

        <label className="mt-3 block text-[12px] font-medium text-neutral-500 dark:text-neutral-400">
          集名称
        </label>
        <input
          ref={inputRef}
          type="text"
          value={name}
          maxLength={80}
          onChange={(e) => setName(e.target.value)}
          className="mt-1.5 w-full rounded-lg border border-neutral-200 bg-white px-3 py-2 text-[13px] text-neutral-900 outline-none transition-colors placeholder:text-neutral-400 focus:border-neutral-400 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
          placeholder="例如：写作助手集"
        />

        <label className="mt-3 block text-[12px] font-medium text-neutral-500 dark:text-neutral-400">
          系统提示词
          <span className="ml-1 font-normal text-neutral-400">（集内对话实时套用）</span>
        </label>
        <textarea
          value={systemPrompt}
          onChange={(e) => setSystemPrompt(e.target.value)}
          rows={4}
          className="mt-1.5 max-h-48 w-full resize-y rounded-lg border border-neutral-200 bg-white px-3 py-2 text-[13px] leading-relaxed text-neutral-900 outline-none transition-colors placeholder:text-neutral-400 focus:border-neutral-400 dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
          placeholder="给这个集里的对话统一的角色 / 指令…"
        />

        <label className="mt-3 block text-[12px] font-medium text-neutral-500 dark:text-neutral-400">
          默认助手
          <span className="ml-1 font-normal text-neutral-400">（在集下新建对话时使用）</span>
        </label>
        <Select
          value={defaultAssistantId}
          onChange={setDefaultAssistantId}
          options={[
            { value: '', label: '不指定（用全局默认）' },
            ...selectableAssistants.map((assistant) => ({
              value: assistant.id,
              label: assistant.name,
            })),
          ]}
          className="mt-1.5"
        />

        <label className="mt-3 block text-[12px] font-medium text-neutral-500 dark:text-neutral-400">
          颜色
          <span className="ml-1 font-normal text-neutral-400">（侧栏集图标着色）</span>
        </label>
        <div className="mt-1.5 flex flex-wrap items-center gap-1.5">
          <button
            type="button"
            onClick={() => setColor(null)}
            className={`grid size-6 place-items-center rounded-full border text-[10px] text-neutral-400 ${
              color === null
                ? 'border-neutral-900 ring-2 ring-neutral-300 dark:border-neutral-100 dark:ring-neutral-600'
                : 'border-neutral-300 dark:border-neutral-600'
            }`}
            aria-label="无颜色"
            title="无颜色"
          >
            ✕
          </button>
          {setColors.map((c) => (
            <button
              key={c}
              type="button"
              onClick={() => setColor(c)}
              className={`size-6 rounded-full border ${
                color === c
                  ? 'border-neutral-900 ring-2 ring-neutral-300 dark:border-neutral-100 dark:ring-neutral-600'
                  : 'border-transparent'
              }`}
              style={{ backgroundColor: c }}
              aria-label={`选择颜色 ${c}`}
            />
          ))}
        </div>

        {error && <p className="mt-2 text-[12px] text-red-600 dark:text-red-400">{error}</p>}
        <div className="mt-4 flex justify-end gap-2">
          <button
            type="button"
            onClick={startClose}
            className="rounded-lg px-3 py-1.5 text-[13px] text-neutral-600 transition-colors hover:bg-black/[0.04] dark:text-neutral-300 dark:hover:bg-white/[0.06]"
          >
            取消
          </button>
          <button
            type="submit"
            disabled={!name.trim() || saving}
            className="rounded-lg bg-neutral-900 px-3 py-1.5 text-[13px] font-medium text-white transition-colors hover:bg-neutral-800 disabled:cursor-default disabled:opacity-50 dark:bg-neutral-100 dark:text-neutral-950 dark:hover:bg-white"
          >
            {saving ? '保存中…' : set ? '保存' : '创建'}
          </button>
        </div>
      </form>
    </div>,
    document.body,
  )
}
