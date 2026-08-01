import { CheckCircle2, ListTodo, X } from 'lucide-react'
import { useMemo, useState } from 'react'
import type { AgentTodoItem, AgentTodoState } from './types'

interface AgentTodoIndicatorProps {
  todoState?: AgentTodoState | null
  /** status = 输入框上方的状态条：整行铺开显示完整任务，弹层向上开。 */
  placement?: 'titlebar' | 'status'
}

const EMPTY_TODO_ITEMS: AgentTodoItem[] = []

function statusLabel(status: AgentTodoItem['status']): string {
  switch (status) {
    case 'completed':
      return 'Done'
    case 'in_progress':
      return 'Now'
    case 'cancelled':
      return 'Skip'
    default:
      return 'Next'
  }
}

function dotClass(status: AgentTodoItem['status']): string {
  switch (status) {
    case 'completed':
      return 'bg-emerald-500'
    case 'in_progress':
      return 'bg-amber-500 shadow-[0_0_0_3px_rgba(245,158,11,0.16)]'
    case 'cancelled':
      return 'bg-neutral-300 ring-1 ring-inset ring-neutral-400 dark:bg-neutral-700'
    default:
      return 'bg-neutral-300 dark:bg-neutral-600'
  }
}

function textClass(status: AgentTodoItem['status']): string {
  switch (status) {
    case 'completed':
      return 'text-neutral-400 line-through decoration-neutral-300 dark:text-neutral-500 dark:decoration-neutral-600'
    case 'in_progress':
      return 'font-medium text-neutral-900 dark:text-neutral-100'
    case 'cancelled':
      return 'text-neutral-400 line-through decoration-neutral-300 dark:text-neutral-500 dark:decoration-neutral-600'
    default:
      return 'text-neutral-600 dark:text-neutral-300'
  }
}

function formatUpdatedAt(todoState?: AgentTodoState | null): string {
  const seconds = todoState?.updated_at ?? todoState?.updatedAt
  if (!seconds) return ''
  return new Date(seconds * 1000).toLocaleTimeString(undefined, {
    hour: '2-digit',
    minute: '2-digit',
  })
}

export function AgentTodoIndicator({ todoState, placement = 'titlebar' }: AgentTodoIndicatorProps) {
  const [open, setOpen] = useState(false)
  const inStatusBar = placement === 'status'
  const items = todoState?.items ?? EMPTY_TODO_ITEMS
  const completedCount = items.filter((item) => item.status === 'completed').length
  const currentItem = useMemo(
    () =>
      items.find((item) => item.status === 'in_progress') ??
      items.find((item) => item.status === 'pending') ??
      items[items.length - 1],
    [items],
  )
  const updatedAt = formatUpdatedAt(todoState)
  // 全部解决 = 没有 pending / in_progress（cancelled 视为已解决，不算未完成）。
  const allDone = items.length > 0 && !items.some((item) => item.status === 'pending' || item.status === 'in_progress')

  if (items.length === 0) return null

  return (
    <div className="relative min-w-0 shrink-0" data-tauri-drag-region="false">
      <button
        type="button"
        className={
          inStatusBar
            ? 'flex h-[22px] shrink-0 items-center gap-1.5 rounded-[5px] px-1.5 text-[12px] font-medium text-neutral-700 transition-colors duration-[var(--kv-dur-instant)] hover:bg-black/[0.05] dark:text-neutral-200 dark:hover:bg-white/[0.07]'
            : `flex h-8 min-w-0 max-w-[18rem] shrink items-center gap-1.5 rounded-lg px-2 text-[12px] font-medium transition duration-[var(--kv-dur-instant)] active:scale-[0.97] ${
                allDone
                  ? 'text-neutral-500 hover:bg-black/[0.05] hover:text-neutral-800 dark:text-neutral-400 dark:hover:bg-white/[0.07] dark:hover:text-neutral-100'
                  : 'text-neutral-700 hover:bg-black/[0.05] hover:text-neutral-950 dark:text-neutral-200 dark:hover:bg-white/[0.07] dark:hover:text-neutral-50'
              }`
        }
        aria-label="Agent todo"
        title={inStatusBar && !allDone && currentItem ? currentItem.content : undefined}
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        {allDone ? (
          <CheckCircle2 size={14} strokeWidth={2} className="shrink-0 text-emerald-600 dark:text-emerald-400" />
        ) : (
          <ListTodo size={14} strokeWidth={2} className="shrink-0 text-neutral-500 dark:text-neutral-400" />
        )}
        <span className="shrink-0 tabular-nums">
          {allDone ? 'Done' : 'Todo'} {completedCount}/{items.length}
        </span>
        {/* ponytail: 状态条只放计数，当前任务文本点开弹层看 —— 铺满整行读起来是噪音。 */}
        {!inStatusBar && !allDone && currentItem && (
          <span
            className="chat-titlebar-todo-current min-w-0 truncate text-neutral-400 dark:text-neutral-500"
          >
            · {currentItem.content}
          </span>
        )}
      </button>

      {open && (
        <div
          className={`chat-motion-popover absolute z-40 w-[21rem] max-w-[calc(100vw-2rem)] rounded-xl border border-neutral-200/90 bg-white p-3 shadow-xl dark:border-neutral-700 dark:bg-neutral-900 ${
            inStatusBar ? 'bottom-8 left-0' : 'right-0 top-9'
          }`}
        >
          <div className="mb-3 flex items-start justify-between gap-3">
            <div className="min-w-0">
              <div className="flex min-w-0 items-center gap-2">
                <span className="text-[13px] font-semibold leading-none text-neutral-900 dark:text-neutral-50">
                  Agent todo
                </span>
                <span className="shrink-0 rounded-full bg-neutral-100 px-1.5 py-[2px] text-[10px] font-medium leading-none text-neutral-500 dark:bg-neutral-800 dark:text-neutral-400">
                  {completedCount}/{items.length}
                </span>
              </div>
              {updatedAt && (
                <div className="mt-1 text-[10.5px] leading-none text-neutral-400 dark:text-neutral-500">
                  Updated {updatedAt}
                </div>
              )}
            </div>
            <button
              type="button"
              className="-mr-1 -mt-1 rounded-md p-1 text-neutral-400 hover:bg-neutral-100 hover:text-neutral-700 dark:hover:bg-neutral-800 dark:hover:text-neutral-200"
              aria-label="Close todo panel"
              onClick={() => setOpen(false)}
            >
              <X size={14} />
            </button>
          </div>

          <div className="chat-popover-scroll max-h-72 space-y-2 overflow-auto pr-1">
            {items.map((item) => (
              <div key={item.id} className="chat-motion-fade grid grid-cols-[14px_1fr] gap-2 text-[12px] leading-relaxed">
                <span className={`mt-[7px] size-2.5 rounded-full transition-colors duration-[var(--kv-dur-normal)] ${dotClass(item.status)}`} />
                <div className="min-w-0">
                  <div className="mb-0.5 text-[10px] font-medium uppercase leading-none tracking-normal text-neutral-400 dark:text-neutral-500">
                    {statusLabel(item.status)}
                  </div>
                  <div className={textClass(item.status)}>
                    {item.content}
                  </div>
                  {item.description && (
                    <div className="mt-0.5 text-[11px] leading-snug text-neutral-400 dark:text-neutral-500">
                      {item.description}
                    </div>
                  )}
                  {item.blocked_by && item.blocked_by.length > 0 && (
                    <div className="mt-0.5 text-[10.5px] leading-none text-neutral-400 dark:text-neutral-500">
                      blocked by: {item.blocked_by.join(', ')}
                    </div>
                  )}
                </div>
              </div>
            ))}
          </div>
        </div>
      )}
    </div>
  )
}
