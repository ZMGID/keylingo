// 会话「来源」选择器：把三类信息源整合进一个弹层——
// 知识库（会话级挂载 id）、连接器（MCP 服务器全局 enabled）、网络搜索（nativeTools.webSearch 全局）。
// 仿 Notion「信息源」：每项一个开关，底部「管理来源」跳设置。
// 弹层贴按钮、紧凑宽度，不再铺满输入框。
import { useCallback, useEffect, useRef, useState } from 'react'
import { SlidersHorizontal, Library, Globe, SearchCheck } from 'lucide-react'
import { kbListLibraries, onKbIndex, type KnowledgeLibrary } from './knowledgeBase'
import { IconButton } from '../components/Button'
import { usePopoverMaxHeight } from './usePopoverMaxHeight'
import type { ChatMcpServer } from '../api/tauri'
import type { WebSearchMode } from './types'

const WEB_SEARCH_OPTIONS: { value: WebSearchMode; label: string; hint?: string }[] = [
  { value: 'off', label: '关闭' },
  { value: 'builtin', label: '内置', hint: '使用模型自带的联网搜索' },
  { value: 'third_party', label: '第三方', hint: '使用已配置的搜索服务（Tavily / Exa 等）' },
]

function Switch({ checked }: { checked: boolean }) {
  return (
    <span
      className={`relative inline-flex h-[18px] w-[30px] shrink-0 items-center rounded-full transition-colors duration-[var(--kv-dur-fast)] ${
        checked ? 'bg-emerald-500' : 'bg-neutral-300 dark:bg-neutral-600'
      }`}
    >
      <span
        className={`absolute left-0.5 size-[14px] rounded-full bg-white shadow-sm transition-transform duration-[var(--kv-dur-fast)] ease-[var(--kv-ease-spring)] ${
          checked ? 'translate-x-3' : ''
        }`}
      />
    </span>
  )
}

function SourceRow({
  icon,
  label,
  meta,
  checked,
  onClick,
}: {
  icon: React.ReactNode
  label: string
  meta?: string
  checked: boolean
  onClick: () => void
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      className="kv-menu-item"
    >
      <span className="grid size-4 shrink-0 place-items-center text-neutral-500 dark:text-neutral-400">{icon}</span>
      <span className="min-w-0 flex-1 truncate">{label}</span>
      {meta && <span className="shrink-0 text-[10.5px] text-neutral-400">{meta}</span>}
      <Switch checked={checked} />
    </button>
  )
}

export function SourcesButton({
  knowledgeBaseIds,
  onChangeKnowledgeBaseIds,
  forceKnowledgeSearch = false,
  onToggleForceKnowledgeSearch,
  webSearchMode,
  onSetWebSearchMode,
  builtinWebSearchSupported = false,
  onOpenSettings,
  disabled,
  layout = 'footer',
}: {
  knowledgeBaseIds: string[]
  onChangeKnowledgeBaseIds: (ids: string[]) => void | Promise<void>
  forceKnowledgeSearch?: boolean
  onToggleForceKnowledgeSearch?: () => void | Promise<void>
  mcpServers: ChatMcpServer[]
  onToggleMcpServer: (serverId: string) => void | Promise<void>
  webSearchMode: WebSearchMode
  onSetWebSearchMode: (mode: WebSearchMode) => void | Promise<void>
  /** 当前模型是否支持内置搜索（否则「内置」置灰）。 */
  builtinWebSearchSupported?: boolean
  onOpenSettings?: () => void
  disabled?: boolean
  layout?: 'footer' | 'inline'
}) {
  const [open, setOpen] = useState(false)
  const [libraries, setLibraries] = useState<KnowledgeLibrary[]>([])
  const ref = useRef<HTMLDivElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)

  const loadLibs = useCallback(async () => {
    try {
      const libs = await kbListLibraries()
      setLibraries(libs)
      // 清理已删除库留下的陈旧挂载 id。
      const valid = knowledgeBaseIds.filter((id) => libs.some((l) => l.id === id))
      if (valid.length !== knowledgeBaseIds.length) void onChangeKnowledgeBaseIds(valid)
    } catch {
      /* ignore */
    }
  }, [knowledgeBaseIds, onChangeKnowledgeBaseIds])

  const loadLibsRef = useRef(loadLibs)
  loadLibsRef.current = loadLibs

  useEffect(() => {
    void loadLibsRef.current()
    let cancelled = false
    let unlisten: (() => void) | undefined
    void onKbIndex(() => void loadLibsRef.current()).then((fn) => {
      if (cancelled) fn()
      else unlisten = fn
    })
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [])

  useEffect(() => {
    if (open) void loadLibsRef.current()
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

  const mountedKbCount = knowledgeBaseIds.length
  const anyActive = mountedKbCount > 0 || webSearchMode !== 'off'

  const toggleKb = (id: string) => {
    void onChangeKnowledgeBaseIds(
      knowledgeBaseIds.includes(id)
        ? knowledgeBaseIds.filter((x) => x !== id)
        : [...knowledgeBaseIds, id],
    )
  }

  const placement = layout === 'inline' ? 'top-full mt-1.5' : 'bottom-full mb-1.5'
  const origin = layout === 'inline' ? 'top left' : 'bottom left'
  const maxH = usePopoverMaxHeight(open, popoverRef, layout === 'inline' ? 'down' : 'up', 360)

  return (
    <div className="relative" ref={ref}>
      <IconButton
        size="sm"
        shape="circle"
        disabled={disabled}
        onClick={() => setOpen((v) => !v)}
        className={`relative focus-visible:ring-2 focus-visible:ring-neutral-300/60 dark:focus-visible:ring-neutral-600 ${
          open
            ? 'bg-neutral-200 text-neutral-700 dark:bg-neutral-700 dark:text-neutral-100'
            : anyActive
              ? 'text-emerald-600 hover:bg-neutral-100 dark:text-emerald-400 dark:hover:bg-neutral-800'
              : 'text-neutral-500 hover:bg-neutral-100 dark:text-neutral-400 dark:hover:bg-neutral-800'
        }`}
        aria-expanded={open}
        aria-haspopup="menu"
        label="信息来源 · 知识库 / 连接器 / 网络搜索"
      >
        <SlidersHorizontal size={16} strokeWidth={1.75} />
      </IconButton>
      {open && (
        <div
          ref={popoverRef}
          className={`chat-motion-popover chat-popover-scroll absolute left-0 z-50 w-[min(280px,calc(100vw-24px))] overflow-y-auto kv-menu ${placement}`}
          style={{ ['--chat-popover-origin' as string]: origin, maxHeight: maxH }}
          data-tauri-drag-region="false"
          role="menu"
        >
          <div className="flex items-center gap-2 px-2 py-1 text-[12px] text-neutral-700 dark:text-neutral-200">
            <span className="grid size-4 shrink-0 place-items-center text-neutral-500 dark:text-neutral-400">
              <Globe size={13} strokeWidth={1.75} />
            </span>
            <span className="min-w-0 flex-1 truncate">网络搜索</span>
            <div className="flex shrink-0 gap-1">
              {WEB_SEARCH_OPTIONS.map((opt) => {
                const active = webSearchMode === opt.value
                const dim = opt.value === 'builtin' && !builtinWebSearchSupported
                return (
                  <button
                    key={opt.value}
                    type="button"
                    disabled={dim}
                    title={dim ? '当前模型不支持内置搜索' : opt.hint}
                    onClick={() => void onSetWebSearchMode(opt.value)}
                    className={`rounded-md px-2 py-0.5 text-[11.5px] transition-colors ${
                      active
                        ? 'bg-emerald-500/15 font-medium text-emerald-700 dark:text-emerald-300'
                        : dim
                          ? 'cursor-not-allowed text-neutral-300 dark:text-neutral-600'
                          : 'text-neutral-600 hover:bg-neutral-100 dark:text-neutral-300 dark:hover:bg-neutral-800'
                    }`}
                  >
                    {opt.label}
                  </button>
                )
              })}
            </div>
          </div>

          {libraries.length > 0 && (
            <>
              <div className="px-2 pt-1 pb-0.5 text-[10px] font-semibold uppercase tracking-wide text-neutral-400">
                知识库
              </div>
              {libraries.map((lib) => (
                <SourceRow
                  key={lib.id}
                  icon={<Library size={13} strokeWidth={1.75} />}
                  label={lib.name}
                  meta={String(lib.docCount)}
                  checked={knowledgeBaseIds.includes(lib.id)}
                  onClick={() => toggleKb(lib.id)}
                />
              ))}
              {onToggleForceKnowledgeSearch && (
                <SourceRow
                  icon={<SearchCheck size={13} strokeWidth={1.75} />}
                  label="强制检索"
                  meta={mountedKbCount === 0 ? '需先挂载' : undefined}
                  checked={forceKnowledgeSearch}
                  onClick={() => void onToggleForceKnowledgeSearch()}
                />
              )}
            </>
          )}

          {onOpenSettings && (
            <>
              <div className="my-1 border-t border-neutral-200/80 dark:border-neutral-800" />
              <button
                type="button"
                onClick={() => {
                  setOpen(false)
                  onOpenSettings()
                }}
                className="kv-menu-item"
              >
                <span className="grid size-4 shrink-0 place-items-center">
                  <SlidersHorizontal size={13} strokeWidth={1.75} />
                </span>
                管理来源
              </button>
            </>
          )}
        </div>
      )}
    </div>
  )
}
