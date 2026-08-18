import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { Bot, Check, Layers, X } from 'lucide-react'
import { Button, IconButton } from '../components/Button'
import { useT } from '../settings/i18n'
import { builtinAssistantGlyph } from './assistantIcons'
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
  const t = useT()
  const [name, setName] = useState(set?.name ?? '')
  const [systemPrompt, setSystemPrompt] = useState(
    set?.system_prompt ?? set?.systemPrompt ?? '',
  )
  const [defaultAssistantId, setDefaultAssistantId] = useState(
    set?.default_assistant_id ?? set?.defaultAssistantId ?? '',
  )
  const [color, setColor] = useState<string | null>(set?.color ?? null)
  const [assistantOpen, setAssistantOpen] = useState(false)
  const inputRef = useRef<HTMLInputElement>(null)
  const assistantRef = useRef<HTMLDivElement>(null)
  const title = set ? t.chatEditSet : t.chatNewSet
  const { closing, startClose, onAnimationEnd } = useCloseAnimation(onClose)
  const selectableAssistants = assistants.filter((a) => !a.archived)
  const selectedAssistant = selectableAssistants.find((a) => a.id === defaultAssistantId) ?? null
  const accent = color ?? '#8E8E93'

  useEffect(() => {
    inputRef.current?.focus()
    inputRef.current?.select()
  }, [])

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') {
        if (assistantOpen) {
          setAssistantOpen(false)
          return
        }
        startClose()
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [assistantOpen, startClose])

  useEffect(() => {
    if (!assistantOpen) return
    const onDown = (e: MouseEvent) => {
      if (assistantRef.current?.contains(e.target as Node)) return
      setAssistantOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    return () => document.removeEventListener('mousedown', onDown)
  }, [assistantOpen])

  const submit = () => {
    const nextName = name.trim()
    if (!nextName || saving) return
    onSave(nextName, systemPrompt.trim(), defaultAssistantId.trim() || null, color)
  }

  return createPortal(
    <div
      className={`${closing ? 'chat-motion-fade-out' : 'chat-motion-fade'} fixed inset-0 z-[300] flex items-center justify-center bg-black/35 px-4 backdrop-blur-[2px]`}
      data-tauri-drag-region="false"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) startClose()
      }}
    >
      <form
        className={`${closing ? 'chat-motion-modal-out' : 'chat-motion-modal-in'} w-full max-w-[520px] overflow-hidden rounded-2xl bg-[#f6f5f2] text-neutral-900 shadow-[0_24px_64px_-20px_rgba(0,0,0,0.35)] dark:bg-[#1c1c1e] dark:text-neutral-100`}
        role="dialog"
        aria-modal="true"
        aria-label={title}
        onAnimationEnd={onAnimationEnd}
        onSubmit={(e) => {
          e.preventDefault()
          submit()
        }}
      >
        <div className="flex items-center justify-between px-5 pt-4">
          <span className="text-[11px] font-medium tracking-[0.14em] text-neutral-400 uppercase dark:text-neutral-500">
            {title}
          </span>
          <IconButton size="xs" label={t.cancel} onClick={startClose}>
            <X strokeWidth={1.75} />
          </IconButton>
        </div>

        <div className="px-5 pt-5 pb-4">
          <div className="flex items-start gap-3.5">
            <div
              className={`grid size-12 shrink-0 place-items-center rounded-2xl transition-colors duration-200 ${
                color ? '' : 'bg-black/[0.05] text-neutral-400 dark:bg-white/[0.07] dark:text-neutral-500'
              }`}
              style={color ? { backgroundColor: `${color}24`, color: accent } : undefined}
            >
              <Layers size={22} strokeWidth={1.6} />
            </div>
            <div className="min-w-0 flex-1 pt-0.5">
              <input
                ref={inputRef}
                type="text"
                value={name}
                maxLength={80}
                onChange={(e) => setName(e.target.value)}
                className="w-full bg-transparent text-[22px] font-semibold leading-tight tracking-tight text-neutral-900 outline-none placeholder:font-medium placeholder:text-neutral-300 dark:text-neutral-50 dark:placeholder:text-neutral-600"
                placeholder={t.chatSetNamePlaceholder}
                aria-label={t.chatSetName}
              />
              <div className="mt-3 flex flex-wrap items-center gap-1.5">
                <button
                  type="button"
                  onClick={() => setColor(null)}
                  className={`grid size-[18px] place-items-center rounded-full border transition-transform ${
                    color === null
                      ? 'scale-110 border-neutral-800 dark:border-neutral-200'
                      : 'border-neutral-300 hover:scale-110 dark:border-neutral-600'
                  }`}
                  aria-label={t.chatSetNoColor}
                  title={t.chatSetNoColor}
                >
                  <span className="block h-px w-2.5 rotate-45 bg-neutral-400" />
                </button>
                {setColors.map((c) => (
                  <button
                    key={c}
                    type="button"
                    onClick={() => setColor(c)}
                    className={`size-[18px] rounded-full transition-transform ${
                      color === c ? 'scale-110 ring-2 ring-neutral-900 ring-offset-2 ring-offset-[#f6f5f2] dark:ring-white dark:ring-offset-[#1c1c1e]' : 'hover:scale-110'
                    }`}
                    style={{ backgroundColor: c }}
                    aria-label={t.chatAssistantSelectColorNamed.replace('{name}', c)}
                  />
                ))}
              </div>
            </div>
          </div>
        </div>

        <div className="mx-5 overflow-hidden rounded-xl bg-white dark:bg-[#2a2a2c]">
          <div className="flex items-center justify-between px-3.5 pt-3">
            <span className="text-[11px] font-medium text-neutral-400 dark:text-neutral-500">
              {t.chatSystemPrompt}
            </span>
            <span className="text-[10.5px] text-neutral-300 dark:text-neutral-600">
              {t.chatSetSystemPromptHint}
            </span>
          </div>
          <textarea
            value={systemPrompt}
            onChange={(e) => setSystemPrompt(e.target.value)}
            rows={6}
            className="min-h-[148px] w-full resize-none bg-transparent px-3.5 pt-2 pb-3.5 text-[13.5px] leading-relaxed text-neutral-800 outline-none placeholder:text-neutral-300 dark:text-neutral-200 dark:placeholder:text-neutral-600"
            placeholder={t.chatSetSystemPromptPlaceholder}
            aria-label={t.chatSystemPrompt}
          />
        </div>

        {error && <p className="px-5 pt-3 text-[12px] text-red-600 dark:text-red-400">{error}</p>}

        <div className="flex items-center justify-between gap-3 px-5 pt-4 pb-4">
          <div ref={assistantRef} className="relative min-w-0">
            <button
              type="button"
              onClick={() => setAssistantOpen((open) => !open)}
              className="flex max-w-[220px] items-center gap-1.5 rounded-full bg-white px-2.5 py-1 text-[12px] text-neutral-600 transition-colors hover:bg-white/80 dark:bg-[#2a2a2c] dark:text-neutral-300 dark:hover:bg-[#323234]"
              aria-haspopup="listbox"
              aria-expanded={assistantOpen}
              title={t.chatSetDefaultAssistantHint}
            >
              <span className="grid size-4 shrink-0 place-items-center text-neutral-400 dark:text-neutral-500">
                {selectedAssistant
                  ? builtinAssistantGlyph(selectedAssistant.id, 13) ?? <Bot size={13} strokeWidth={1.75} />
                  : <Bot size={13} strokeWidth={1.75} />}
              </span>
              <span className="min-w-0 truncate">
                {selectedAssistant ? selectedAssistant.name : t.chatSetDefaultAssistantNone}
              </span>
            </button>
            {assistantOpen && (
              <div
                className="chat-motion-popover absolute bottom-full left-0 z-50 mb-1.5 w-[240px] overflow-y-auto kv-menu"
                style={{ ['--chat-popover-origin' as string]: 'bottom left', maxHeight: 260 }}
                role="listbox"
              >
                <button
                  type="button"
                  role="option"
                  aria-selected={!selectedAssistant}
                  onClick={() => {
                    setDefaultAssistantId('')
                    setAssistantOpen(false)
                  }}
                  className="kv-menu-item"
                >
                  <Bot size={13} strokeWidth={1.75} />
                  {t.chatSetDefaultAssistantNone}
                  {!selectedAssistant && <Check size={12} strokeWidth={2.5} className="ml-auto" />}
                </button>
                {selectableAssistants.map((assistant) => {
                  const active = assistant.id === defaultAssistantId
                  return (
                    <button
                      key={assistant.id}
                      type="button"
                      role="option"
                      aria-selected={active}
                      onClick={() => {
                        setDefaultAssistantId(assistant.id)
                        setAssistantOpen(false)
                      }}
                      className="kv-menu-item"
                    >
                      <span className="grid size-4 shrink-0 place-items-center">
                        {builtinAssistantGlyph(assistant.id, 13) ?? <Bot size={13} strokeWidth={1.75} />}
                      </span>
                      <span className="min-w-0 flex-1 truncate">{assistant.name}</span>
                      {active && <Check size={12} strokeWidth={2.5} />}
                    </button>
                  )
                })}
              </div>
            )}
          </div>

          <div className="flex shrink-0 items-center gap-2">
            <Button size="sm" onClick={startClose}>
              {t.cancel}
            </Button>
            <Button
              type="submit"
              variant="primary"
              size="sm"
              disabled={!name.trim() || saving}
            >
              {saving ? t.saving : set ? t.save : t.chatSetCreate}
            </Button>
          </div>
        </div>
      </form>
    </div>,
    document.body,
  )
}
