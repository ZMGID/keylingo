import { useMemo, useState } from 'react'
import { Bot, ChevronDown, Globe, MessageSquare, Pencil, Plus, RefreshCw, Terminal, Trash2, Wrench } from 'lucide-react'
import { HOOK_EVENTS, type HookDef, type HookEvent } from '../../api/tauri'
import { i18n, type I18n, type Lang } from '../i18n'
import { Toggle } from '../components'
import { Button, IconButton } from '../../components/Button'
import { HookModal } from '../HookModal'

type PhaseKey = 'agent' | 'turn' | 'message' | 'tool'

/** 事件流按对话顺序排列；生命周期导轨的唯一数据源（对齐 Rust `HookEvent`）。 */
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

type Strings = I18n

function phaseLabel(t: Strings, phase: PhaseKey): string {
  return phase === 'agent' ? t.hooksPhaseAgent
    : phase === 'turn' ? t.hooksPhaseTurn
      : phase === 'message' ? t.hooksPhaseMessage
        : t.hooksPhaseTool
}

function eventLabel(t: Strings, event: HookEvent): string {
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

function eventDescription(t: Strings, event: HookEvent): string {
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

/** 相邻同阶段的事件合成一组（AGENT 首尾各一组，与参考实现一致）。 */
function buildPhaseGroups(): { key: string; phase: PhaseKey; events: HookEvent[] }[] {
  const groups: { key: string; phase: PhaseKey; events: HookEvent[] }[] = []
  for (const event of HOOK_EVENTS) {
    const phase = EVENT_PHASES[event]
    const last = groups[groups.length - 1]
    if (last && last.phase === phase) last.events.push(event)
    else groups.push({ key: `${phase}-${groups.length}`, phase, events: [event] })
  }
  return groups
}

export function HooksTab({ lang, hooks, onChange }: {
  lang: Lang
  hooks: HookDef[]
  onChange: (hooks: HookDef[]) => void
}) {
  const t = i18n[lang]
  const [activeEvent, setActiveEvent] = useState<HookEvent>(HOOK_EVENTS[0])
  const [collapsed, setCollapsed] = useState<Set<string>>(new Set())
  const [modal, setModal] = useState<{ editing: HookDef | null } | null>(null)
  const [confirmDeleteId, setConfirmDeleteId] = useState<string | null>(null)

  const groups = useMemo(buildPhaseGroups, [])
  const countByEvent = useMemo(() => {
    const counts: Partial<Record<string, number>> = {}
    for (const hook of hooks) counts[hook.event] = (counts[hook.event] ?? 0) + 1
    return counts
  }, [hooks])
  const activeHooks = hooks.filter((hook) => hook.event === activeEvent)

  const togglePhase = (key: string) => {
    setCollapsed((prev) => {
      const next = new Set(prev)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }

  const saveHook = (hook: HookDef) => {
    const exists = hooks.some((item) => item.id === hook.id)
    onChange(exists ? hooks.map((item) => (item.id === hook.id ? hook : item)) : [...hooks, hook])
    setModal(null)
  }

  return (
    <div className="flex min-h-0 gap-3">
      <aside className="kv-panel w-[230px] shrink-0 space-y-1 p-2">
        <div className="px-2 py-1 text-[11px] font-semibold uppercase tracking-wide text-neutral-400 dark:text-neutral-500">
          {t.hooksLifecycle}
        </div>
        {groups.map((group) => {
          const Icon = PHASE_ICONS[group.phase]
          const phaseCount = group.events.reduce((sum, event) => sum + (countByEvent[event] ?? 0), 0)
          const isCollapsed = collapsed.has(group.key)
          return (
            <div key={group.key}>
              <button
                type="button"
                onClick={() => togglePhase(group.key)}
                aria-expanded={!isCollapsed}
                className="flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[11px] font-semibold uppercase tracking-wide text-neutral-500 transition-colors hover:bg-black/5 dark:text-neutral-400 dark:hover:bg-white/5"
                data-tauri-drag-region="false"
              >
                <Icon size={13} strokeWidth={1.9} className="shrink-0" />
                <span className="min-w-0 flex-1 truncate">{phaseLabel(t, group.phase)}</span>
                {phaseCount > 0 && (
                  <span className="rounded-full bg-black/5 px-1.5 text-[10px] font-semibold dark:bg-white/10">{phaseCount}</span>
                )}
                <ChevronDown size={13} className={`shrink-0 transition-transform ${isCollapsed ? '-rotate-90' : ''}`} />
              </button>
              {!isCollapsed && (
                <ul className="ml-2 border-l border-black/5 pl-1 dark:border-white/10">
                  {group.events.map((event) => {
                    const count = countByEvent[event] ?? 0
                    const selected = activeEvent === event
                    return (
                      <li key={event}>
                        <button
                          type="button"
                          onClick={() => setActiveEvent(event)}
                          aria-current={selected}
                          className={`flex w-full items-center gap-2 rounded-md px-2 py-1.5 text-left text-[12px] transition-colors ${
                            selected
                              ? 'bg-black/[0.06] font-medium text-neutral-900 dark:bg-white/10 dark:text-neutral-100'
                              : 'text-neutral-600 hover:bg-black/[0.04] dark:text-neutral-400 dark:hover:bg-white/5'
                          }`}
                          data-tauri-drag-region="false"
                        >
                          <span
                            aria-hidden
                            className={`h-1.5 w-1.5 shrink-0 rounded-full ${
                              selected || count > 0
                                ? 'bg-[var(--accent)]'
                                : 'border border-neutral-300 dark:border-neutral-600'
                            }`}
                          />
                          <span className="min-w-0 flex-1 truncate">{eventLabel(t, event)}</span>
                          {count > 0 && (
                            <span className="rounded-full bg-black/5 px-1.5 text-[10px] font-semibold dark:bg-white/10">{count}</span>
                          )}
                        </button>
                      </li>
                    )
                  })}
                </ul>
              )}
            </div>
          )
        })}
      </aside>

      <section className="min-w-0 flex-1 space-y-3">
        <div className="flex items-start gap-3">
          <div className="min-w-0 flex-1">
            <div className="text-[13px] font-semibold">{eventLabel(t, activeEvent)}</div>
            <p className="kv-row-desc">{eventDescription(t, activeEvent)}</p>
          </div>
          <Button size="sm" className="shrink-0" onClick={() => setModal({ editing: null })} data-tauri-drag-region="false">
            <Plus size={11} />
            {t.hooksAdd}
          </Button>
        </div>

        {activeHooks.length === 0 ? (
          <div className="kv-panel px-4 py-8 text-center">
            <div className="text-[13px] font-medium">{t.hooksEmptyTitle}</div>
            <p className="kv-row-desc mx-auto mt-1 max-w-sm">{t.hooksEmptyDesc}</p>
            <div className="mt-3 flex justify-center">
              <Button size="sm" onClick={() => setModal({ editing: null })} data-tauri-drag-region="false">
                <Plus size={11} />
                {t.hooksAdd}
              </Button>
            </div>
          </div>
        ) : (
          <div className="space-y-2">
            {activeHooks.map((hook) => (
              <div key={hook.id} className={`kv-panel flex items-start gap-3 p-3 ${hook.enabled ? '' : 'opacity-60'}`}>
                <span className="mt-0.5 shrink-0 text-neutral-400 dark:text-neutral-500">
                  {hook.type === 'http' ? <Globe size={15} /> : <Terminal size={15} />}
                </span>
                <div className="min-w-0 flex-1">
                  <div className="truncate text-[13px] font-medium">{hook.name}</div>
                  <p className="kv-row-desc truncate">{hook.description || t.hooksNoDescription}</p>
                  <p className="kv-row-desc mt-0.5 truncate font-mono text-[11px] opacity-70">
                    {hook.type === 'http' ? `${hook.method} ${hook.url}` : hook.script.split('\n')[0]}
                  </p>
                </div>
                <div className="flex shrink-0 items-center gap-1.5">
                  <Toggle
                    checked={hook.enabled}
                    ariaLabel={hook.name}
                    onChange={(enabled) => onChange(hooks.map((item) => (
                      item.id === hook.id ? { ...item, enabled } : item
                    )))}
                  />
                  <IconButton size="xs" label={t.hooksEdit} onClick={() => setModal({ editing: hook })}>
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
            ))}
          </div>
        )}
      </section>

      {modal && (
        <HookModal
          lang={lang}
          event={activeEvent}
          eventLabel={eventLabel(t, (modal.editing?.event as HookEvent) ?? activeEvent)}
          initial={modal.editing}
          onSave={saveHook}
          onClose={() => setModal(null)}
        />
      )}

      {confirmDeleteId && (
        <div className="kv-modal-backdrop" data-tauri-drag-region="false">
          <div className="kv-modal space-y-3">
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
        </div>
      )}
    </div>
  )
}
