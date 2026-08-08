import { useCallback, useEffect, useMemo, useState } from 'react'
import { createPortal } from 'react-dom'
import { ArrowUpRight, Check, Loader2, RefreshCw } from 'lucide-react'
import { Button } from '../components/Button'
import { useT, type I18n } from '../settings/i18n'
import { chatApi } from './api'
import type { ChatProject, ImportableCliSession } from './types'
import { useCloseAnimation } from './useCloseAnimation'

/// 各 CLI 的展示名。后端返回的是 `RuntimeAgentDef.id`。
const AGENT_LABELS: Record<string, string> = {
  claude: 'Claude Code',
  codex: 'Codex',
  grok: 'Grok',
  kimi: 'Kimi Code',
  opencode: 'OpenCode',
  cursor: 'Cursor',
}

/// kimi 的 `session/load` 能绑定但不重放历史，本地 wire 也没有稳定可读的 assistant 正文，
/// 所以导进来消息区是空的。这条必须在勾选前就说清楚，不能等导完让用户以为丢数据了。
const NO_HISTORY_AGENTS = new Set(['kimi'])

interface CliImportDialogProps {
  project: ChatProject
  onClose: () => void
  /** 导入成功后回调，交给外层刷新列表并跳转。 */
  onImported: (conversationIds: string[]) => void
  /** 点击一条「Kivio 里已经有了」的会话时，跳到那条已存在的对话。 */
  onOpenConversation: (conversationId: string) => void
}

function formatWhen(ms: number | null | undefined, t: I18n): string {
  if (!ms) return ''
  const diff = Date.now() - ms
  const day = 86_400_000
  if (diff < 3_600_000) return t.chatSkillCliJustNow
  if (diff < day) return t.chatSkillCliHoursAgo.replace('{n}', String(Math.floor(diff / 3_600_000)))
  if (diff < 30 * day) return t.chatSkillCliDaysAgo.replace('{n}', String(Math.floor(diff / day)))
  return new Date(ms).toLocaleDateString()
}

export function CliImportDialog({
  project,
  onClose,
  onImported,
  onOpenConversation,
}: CliImportDialogProps) {
  const [sessions, setSessions] = useState<ImportableCliSession[]>([])
  const [loading, setLoading] = useState(true)
  const [importing, setImporting] = useState(false)
  const [error, setError] = useState('')
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const { closing, startClose, onAnimationEnd } = useCloseAnimation(onClose)
  const t = useT()

  const load = useCallback(async () => {
    setLoading(true)
    setError('')
    try {
      setSessions(await chatApi.listImportableCliSessions(project.id))
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
      setSessions([])
    } finally {
      setLoading(false)
    }
  }, [project.id])

  useEffect(() => {
    void load()
  }, [load])

  useEffect(() => {
    const onKeyDown = (e: KeyboardEvent) => {
      if (e.key === 'Escape') startClose()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [startClose])

  const grouped = useMemo(() => {
    const map = new Map<string, ImportableCliSession[]>()
    for (const session of sessions) {
      const list = map.get(session.agentId) ?? []
      list.push(session)
      map.set(session.agentId, list)
    }
    return [...map.entries()].sort((a, b) => a[0].localeCompare(b[0]))
  }, [sessions])

  const keyOf = (s: ImportableCliSession) => `${s.agentId}::${s.sessionId}`

  const toggle = (session: ImportableCliSession) => {
    // 已经有对话绑着这条原生会话就不能再导（绑定是 1:1 的，导第二次会让两边快照都残缺）。
    // 但点击不该没反应——跳到那条已存在的对话去。
    if (session.boundConversationId) {
      onOpenConversation(session.boundConversationId)
      startClose()
      return
    }
    if (session.alreadyImported) return
    setSelected((prev) => {
      const next = new Set(prev)
      const key = keyOf(session)
      if (next.has(key)) next.delete(key)
      else next.add(key)
      return next
    })
  }

  const runImport = async () => {
    if (!selected.size || importing) return
    setImporting(true)
    setError('')
    try {
      const items = [...selected].map((key) => {
        const [agentId, sessionId] = key.split('::')
        return { agentId, sessionId }
      })
      const result = await chatApi.importCliSessions(project.id, items)
      const ids = result.imported.map((item) => item.conversationId)
      if (result.failures.length) {
        // 部分失败不隐藏成功的那些——批量导入单条失败不该让整批白做。
        setError(
          t.chatSkillCliImportFailures
            .replace('{n}', String(result.failures.length))
            .replace('{detail}', result.failures.map((f) => f.error).slice(0, 2).join('；')),
        )
        if (ids.length) onImported(ids)
        await load()
        setSelected(new Set())
        return
      }
      onImported(ids)
      startClose()
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setImporting(false)
    }
  }

  const rootPath = (project.root_path ?? project.rootPath ?? '').trim()

  return createPortal(
    <div
      className={`${closing ? 'chat-motion-fade-out' : 'chat-motion-fade'} fixed inset-0 z-[300] flex items-center justify-center bg-black/30 px-4 backdrop-blur-[1px]`}
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) startClose()
      }}
    >
      <div
        className={`${closing ? 'chat-motion-modal-out' : 'chat-motion-modal-in'} flex max-h-[80vh] w-full max-w-[560px] flex-col rounded-[10px] border border-neutral-200 bg-white shadow-xl dark:border-neutral-700 dark:bg-[#252527]`}
        role="dialog"
        aria-modal="true"
        aria-label={t.chatImportFromCli}
        onAnimationEnd={onAnimationEnd}
      >
        <div className="flex items-start justify-between gap-3 border-b border-neutral-200 px-4 py-3 dark:border-neutral-700">
          <div className="min-w-0">
            <h3 className="text-[14px] font-semibold text-neutral-900 dark:text-neutral-50">
              {t.chatImportFromCli}
            </h3>
            <p className="mt-0.5 truncate text-[11px] text-neutral-500 dark:text-neutral-400">
              {t.chatSkillCliImportScopeHint.replace('{path}', rootPath)}
            </p>
          </div>
          <Button variant="ghost" size="sm" onClick={() => void load()} disabled={loading}>
            <RefreshCw strokeWidth={1.75} className={loading ? 'animate-spin' : undefined} />
            {t.externalAgentsRescan}
          </Button>
        </div>

        <div className="min-h-[160px] flex-1 overflow-y-auto px-4 py-3">
          {loading ? (
            <div className="flex items-center gap-2 py-8 text-[12px] text-neutral-500 dark:text-neutral-400">
              <Loader2 strokeWidth={1.75} className="animate-spin" />
              {t.chatSkillCliScanningHint}
            </div>
          ) : !sessions.length ? (
            <p className="py-8 text-center text-[12px] text-neutral-500 dark:text-neutral-400">
              {t.chatSkillCliNoSessions}
            </p>
          ) : (
            grouped.map(([agentId, list]) => (
              <div key={agentId} className="mb-4 last:mb-0">
                <div className="mb-1.5 flex items-baseline gap-2">
                  <span className="text-[12px] font-medium text-neutral-700 dark:text-neutral-200">
                    {AGENT_LABELS[agentId] ?? agentId}
                  </span>
                  <span className="text-[11px] text-neutral-400">{t.chatSkillCliCount.replace('{n}', String(list.length))}</span>
                  {NO_HISTORY_AGENTS.has(agentId) && (
                    <span className="text-[11px] text-amber-600 dark:text-amber-500">
                      {t.chatSkillCliNoHistoryHint}
                    </span>
                  )}
                </div>
                <ul className="space-y-1">
                  {list.map((session) => {
                    const key = keyOf(session)
                    const checked = selected.has(key)
                    const bound = Boolean(session.boundConversationId)
                    // 「已导入」和「Kivio 里已经有了」是两回事：后者是 Kivio 自己跑出来的会话，
                    // 用户从没导过它，标成"已导入"是在撒谎。
                    const boundLabel = session.alreadyImported ? t.chatSkillCliImported : t.chatSkillCliBound
                    return (
                      <li key={key}>
                        <button
                          type="button"
                          onClick={() => toggle(session)}
                          title={bound ? t.chatSkillCliOpenBound : undefined}
                          className={`flex w-full items-start gap-2 rounded-[6px] px-2 py-1.5 text-left transition-colors ${
                            bound ? 'opacity-55' : ''
                          } hover:bg-neutral-100 dark:hover:bg-neutral-800`}
                        >
                          <span
                            className={`mt-0.5 flex h-[14px] w-[14px] shrink-0 items-center justify-center rounded-[3px] ${
                              bound
                                ? 'text-neutral-400'
                                : checked
                                  ? 'rounded-[3px] border border-blue-500 bg-blue-500 text-white'
                                  : 'border border-neutral-300 dark:border-neutral-600'
                            }`}
                          >
                            {bound ? (
                              <ArrowUpRight strokeWidth={2} className="h-[12px] w-[12px]" />
                            ) : (
                              checked && <Check strokeWidth={3} className="h-[10px] w-[10px]" />
                            )}
                          </span>
                          <span className="min-w-0 flex-1">
                            <span className="block truncate text-[12px] text-neutral-800 dark:text-neutral-100">
                              {session.title || t.chatSkillCliUntitled}
                            </span>
                            <span className="mt-0.5 block text-[11px] text-neutral-400">
                              {session.messageCount == null
                                ? t.chatSkillCliUnknownCount
                                : t.chatSkillCliCount.replace('{n}', String(session.messageCount))}
                              {formatWhen(session.updatedAt, t) && ` · ${formatWhen(session.updatedAt, t)}`}
                              {bound && ` · ${boundLabel}`}
                            </span>
                          </span>
                        </button>
                      </li>
                    )
                  })}
                </ul>
              </div>
            ))
          )}
        </div>

        {error && (
          <p className="border-t border-neutral-200 px-4 py-2 text-[11px] text-red-600 dark:border-neutral-700 dark:text-red-400">
            {error}
          </p>
        )}

        <div className="flex items-center justify-end gap-2 border-t border-neutral-200 px-4 py-3 dark:border-neutral-700">
          <Button variant="ghost" size="sm" onClick={startClose} disabled={importing}>
            {t.cancel}
          </Button>
          <Button size="sm" onClick={() => void runImport()} disabled={!selected.size || importing}>
            {importing && <Loader2 strokeWidth={1.75} className="animate-spin" />}
            {t.chatSkillCliImportAction.replace('{n}', selected.size ? String(selected.size) : '')}
          </Button>
        </div>
      </div>
    </div>,
    document.body,
  )
}
