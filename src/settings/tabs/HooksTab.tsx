import { useMemo, useState } from 'react'
import { createPortal } from 'react-dom'
import { Bot, Globe, MessageSquare, Pencil, Plus, RefreshCw, Terminal, Trash2, Wrench, Zap } from 'lucide-react'
import { HOOK_EVENTS, type HookDef, type HookEvent } from '../../api/tauri'
import { i18n, type I18n, type Lang } from '../i18n'
import { SettingsGroup, Toggle } from '../components'
import { Button, IconButton } from '../../components/Button'
import { HookModal } from '../HookModal'

type PhaseKey = 'agent' | 'turn' | 'message' | 'tool'

/** 事件 → 所属阶段。仅用于给列表行和新建菜单打分组标签。 */
const EVENT_PHASES: Record<HookEvent, PhaseKey> = {
  agent_start: 'agent',
  turn_start: 'turn',
  message_start: 'message',
  message_end: 'message',
  tool_execution_start: 'tool',
  tool_execution_end: 'tool',
  turn_end: 'turn',
  agent_end: 'agent',
}

const PHASE_ICONS = {
  agent: Bot,
  turn: RefreshCw,
  message: MessageSquare,
  tool: Wrench,
} as const

function eventLabel(t: I18n, event: HookEvent): string {
  switch (event) {
    case 'agent_start': return t.hookEventAgentStart
    case 'agent_end': return t.hookEventAgentEnd
    case 'turn_start': return t.hookEventTurnStart
    case 'turn_end': return t.hookEventTurnEnd
    case 'message_start': return t.hookEventMessageStart
    case 'message_end': return t.hookEventMessageEnd
    case 'tool_execution_start': return t.hookEventToolStart
    case 'tool_execution_end': return t.hookEventToolEnd
  }
}

function eventDescription(t: I18n, event: HookEvent): string {
  switch (event) {
    case 'agent_start': return t.hookEventDescAgentStart
    case 'agent_end': return t.hookEventDescAgentEnd
    case 'turn_start': return t.hookEventDescTurnStart
    case 'turn_end': return t.hookEventDescTurnEnd
    case 'message_start': return t.hookEventDescMessageStart
    case 'message_end': return t.hookEventDescMessageEnd
    case 'tool_execution_start': return t.hookEventDescToolStart
    case 'tool_execution_end': return t.hookEventDescToolEnd
  }
}

/** 单个 Hook 的摘要行。有说明用说明；否则命令取首行、HTTP 取「方法 URL」。 */
function hookSummary(hook: HookDef): string {
  const description = hook.description.trim()
  if (description) return description
  if (hook.type === 'http') return `${hook.method} ${hook.url}`
  return hook.script.split('\n').find((line) => line.trim()) ?? ''
}

function hookSummaryIsMono(hook: HookDef): boolean {
  return !hook.description.trim()
}

export function HooksTab({ lang, hooks, onChange }: {
  lang: Lang
  hooks: HookDef[]
  onChange: (hooks: HookDef[]) => void
}) {
  const t = i18n[lang]
  const [modal, setModal] = useState<{ editing: HookDef | null; event: HookEvent } | null>(null)
  const [picking, setPicking] = useState(false)
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null)

  // 按生命周期顺序排，而不是按创建顺序——读列表时「什么时候触发」比「什么时候加的」重要。
  const ordered = useMemo(() => {
    const rank = new Map(HOOK_EVENTS.map((event, index) => [event as string, index]))
    return [...hooks].sort((a, b) => (rank.get(a.event) ?? 99) - (rank.get(b.event) ?? 99))
  }, [hooks])

  const saveHook = (hook: HookDef) => {
    const exists = hooks.some((item) => item.id === hook.id)
    onChange(exists ? hooks.map((item) => (item.id === hook.id ? hook : item)) : [...hooks, hook])
    setModal(null)
  }

  const enabledCount = hooks.filter((hook) => hook.enabled).length

  return (
    <>
      <SettingsGroup title={t.hooksSectionTitle}>
        <div className="mb-3 flex items-start justify-between gap-3">
          <p className="kv-row-desc max-w-[560px]">{t.hooksSectionHint}</p>
          <Button size="sm" className="shrink-0" onClick={() => setPicking(true)} data-tauri-drag-region="false">
            <Plus size={11} />
            {t.hooksAdd}
          </Button>
        </div>

        {ordered.length === 0 ? (
          <div className="flex flex-col items-center gap-2 py-8 text-center">
            <Zap size={18} className="text-neutral-300 dark:text-neutral-600" strokeWidth={1.8} />
            <div className="text-[12.5px] text-neutral-500 dark:text-neutral-400">{t.hooksEmptyTitle}</div>
            <p className="kv-row-desc max-w-[420px]">{t.hooksEmptyDesc}</p>
          </div>
        ) : (
          ordered.map((hook) => {
            const event = hook.event as HookEvent
            const Icon = PHASE_ICONS[EVENT_PHASES[event] ?? 'agent'] ?? Bot
            return (
              <div key={hook.id} className="kv-row">
                <span className="shrink-0 text-neutral-400 dark:text-neutral-500">
                  {hook.type === 'http' ? <Globe size={15} strokeWidth={1.8} /> : <Terminal size={15} strokeWidth={1.8} />}
                </span>
                <div className="kv-row-text">
                  <span className="kv-row-label flex items-center gap-1.5">
                    {hook.name}
                    <span className="inline-flex items-center gap-1 rounded bg-black/[0.05] px-1.5 py-px text-[10.5px] font-normal text-neutral-500 dark:bg-white/10 dark:text-neutral-400">
                      <Icon size={10} strokeWidth={2} />
                      {eventLabel(t, event)}
                    </span>
                  </span>
                  <p className={`kv-row-desc truncate${hookSummaryIsMono(hook) ? ' font-mono' : ''}`}>{hookSummary(hook)}</p>
                </div>
                <div className="kv-row-control">
                  <Toggle
                    checked={hook.enabled}
                    ariaLabel={hook.name}
                    onChange={(enabled) => onChange(hooks.map((item) => (
                      item.id === hook.id ? { ...item, enabled } : item
                    )))}
                  />
                  <IconButton size="xs" label={t.hooksEdit} onClick={() => setModal({ editing: hook, event })}>
                    <Pencil size={13} />
                  </IconButton>
                  <IconButton
                    size="xs"
                    variant="danger"
                    label={t.hooksDeleteConfirm}
                    onClick={() => setConfirmDeleteId(hook.id)}
                  >
                    <Trash2 size={13} />
                  </IconButton>
                </div>
              </div>
            )
          })
        )}

        {hooks.length > 0 && (
          <p className="kv-row-desc mt-2">
            {t.hooksCountSummary.replace('{total}', String(hooks.length)).replace('{enabled}', String(enabledCount))}
          </p>
        )}
      </SettingsGroup>

      {/* 事件选择：portal 全屏遮罩，避免只盖住内容区留下侧栏/顶栏亮着。 */}
      {picking && createPortal(
        <div
          className="kv-modal-backdrop kv-modal-backdrop--portal"
          data-tauri-drag-region="false"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) setPicking(false)
          }}
        >
          <div
            className="kv kv-modal max-w-lg space-y-3"
            role="dialog"
            aria-modal="true"
            aria-label={t.hooksPickEvent}
          >
            <div>
              <h3 className="text-[14px] font-semibold">{t.hooksPickEvent}</h3>
              <p className="kv-row-desc">{t.hooksPickEventHint}</p>
            </div>
            <div className="custom-scrollbar max-h-[52vh] overflow-y-auto">
              {HOOK_EVENTS.map((event) => {
                const Icon = PHASE_ICONS[EVENT_PHASES[event]]
                const count = hooks.filter((hook) => hook.event === event).length
                return (
                  <button
                    key={event}
                    type="button"
                    onClick={() => {
                      setPicking(false)
                      setModal({ editing: null, event })
                    }}
                    className="flex w-full items-center gap-2.5 rounded-md px-2 py-2 text-left transition-colors hover:bg-black/[0.04] dark:hover:bg-white/5"
                    data-tauri-drag-region="false"
                  >
                    <span className="shrink-0 text-neutral-400 dark:text-neutral-500">
                      <Icon size={14} strokeWidth={1.8} />
                    </span>
                    <span className="min-w-0 flex-1">
                      <span className="flex items-center gap-1.5 text-[13px]">
                        {eventLabel(t, event)}
                        {count > 0 && (
                          <span className="rounded bg-black/[0.05] px-1.5 text-[10.5px] text-neutral-500 dark:bg-white/10 dark:text-neutral-400">
                            {count}
                          </span>
                        )}
                      </span>
                      <span className="kv-row-desc block">{eventDescription(t, event)}</span>
                    </span>
                  </button>
                )
              })}
            </div>
            <div className="flex justify-end pt-1">
              <Button onClick={() => setPicking(false)} data-tauri-drag-region="false">{t.cancel}</Button>
            </div>
          </div>
        </div>,
        document.body,
      )}

      {modal && (
        <HookModal
          lang={lang}
          event={modal.event}
          eventLabel={eventLabel(t, (modal.editing?.event as HookEvent) ?? modal.event)}
          eventOptions={HOOK_EVENTS.map((item) => ({ value: item, label: eventLabel(t, item) }))}
          initial={modal.editing}
          onSave={saveHook}
          onClose={() => setModal(null)}
        />
      )}

      {confirmDeleteId && createPortal(
        <div
          className="kv-modal-backdrop kv-modal-backdrop--portal"
          data-tauri-drag-region="false"
          onMouseDown={(e) => {
            if (e.target === e.currentTarget) setConfirmDeleteId(null)
          }}
        >
          <div className="kv kv-modal space-y-3">
            <h3 className="text-[14px] font-semibold">{t.hooksDeleteConfirm}</h3>
            <div className="flex justify-end gap-2 pt-1">
              <Button onClick={() => setConfirmDeleteId(null)} data-tauri-drag-region="false">{t.cancel}</Button>
              <Button
                variant="danger"
                onClick={() => {
                  onChange(hooks.filter((item) => item.id !== confirmDeleteId))
                  setConfirmDeleteId(null)
                }}
                data-tauri-drag-region="false"
              >
                {t.hooksDeleteConfirm}
              </Button>
            </div>
          </div>
        </div>,
        document.body,
      )}
    </>
  )
}
