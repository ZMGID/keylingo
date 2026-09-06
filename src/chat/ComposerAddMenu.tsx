import { useCallback, useEffect, useRef, useState, type ReactNode } from 'react'
import { open } from '@tauri-apps/plugin-dialog'
import { ChevronLeft, ChevronRight, FolderPlus, Folders, Paperclip, Plus, SlidersHorizontal, X } from 'lucide-react'
import { IconButton } from '../components/Button'
import { ComposerAddMenuCloseContext } from './composerAddMenuContext'
import { usePopoverMaxHeight } from './usePopoverMaxHeight'
import { useT } from '../settings/i18n'
import type { AdditionalDirectory } from './types'
import { MAX_ADDITIONAL_DIRECTORIES } from './types'

const PROMPT_ONLY_AGENTS = new Set(['pi', 'dsh'])

function normalizeDirPath(path: string): string {
  return path.replace(/[\\/]+$/, '').replace(/\\/g, '/').toLowerCase()
}

function displayName(entry: AdditionalDirectory): string {
  const named = entry.name?.trim()
  if (named) return named
  const normalized = entry.path.replace(/\\/g, '/')
  return normalized.split('/').filter(Boolean).pop() ?? entry.path
}

type AddMenuView = 'root' | 'sources'

export function ComposerAddMenu({
  onAddAttachment,
  directories = [],
  onChangeAdditionalDirectories,
  primaryRootPath,
  externalAgentId,
  disabled,
  layout = 'footer',
  onBeforeOpen,
  sourcesPanel,
  sourcesActive,
}: {
  onAddAttachment: () => void | Promise<void>
  directories?: AdditionalDirectory[]
  onChangeAdditionalDirectories?: (directories: AdditionalDirectory[]) => void | Promise<void>
  primaryRootPath?: string | null
  externalAgentId?: string | null
  disabled?: boolean
  layout?: 'footer' | 'inline'
  onBeforeOpen?: () => void
  sourcesPanel?: ReactNode
  sourcesActive?: boolean
}) {
  const t = useT()
  const [openMenu, setOpenMenu] = useState(false)
  const [view, setView] = useState<AddMenuView>('root')
  const [error, setError] = useState('')
  const ref = useRef<HTMLDivElement>(null)
  const popoverRef = useRef<HTMLDivElement>(null)
  const viewRef = useRef<AddMenuView>('root')
  viewRef.current = view
  const canAttachDirs = Boolean(onChangeAdditionalDirectories)
  const atLimit = directories.length >= MAX_ADDITIONAL_DIRECTORIES
  const promptOnly = Boolean(externalAgentId && PROMPT_ONLY_AGENTS.has(externalAgentId))
  const primaryNorm = primaryRootPath ? normalizeDirPath(primaryRootPath) : ''

  const closeMenu = useCallback(() => {
    setOpenMenu(false)
    setError('')
    setView('root')
  }, [])

  const toggleMenu = () => {
    if (disabled) return
    setOpenMenu((wasOpen) => {
      if (wasOpen) {
        setError('')
        setView('root')
        return false
      }
      onBeforeOpen?.()
      return true
    })
  }

  useEffect(() => {
    if (!openMenu) return
    const onDown = (e: MouseEvent) => {
      if (ref.current?.contains(e.target as Node)) return
      closeMenu()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Escape') return
      if (viewRef.current !== 'root') {
        setView('root')
        return
      }
      closeMenu()
    }
    document.addEventListener('mousedown', onDown)
    window.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('mousedown', onDown)
      window.removeEventListener('keydown', onKey)
    }
  }, [closeMenu, openMenu])

  const attachedNorm = new Set(directories.map((entry) => normalizeDirPath(entry.path)))

  const commit = (next: AdditionalDirectory[]) => {
    setError('')
    void onChangeAdditionalDirectories?.(next)
  }

  const addEntry = (path: string, name?: string) => {
    const trimmed = path.trim()
    if (!trimmed) return
    if (primaryNorm && normalizeDirPath(trimmed) === primaryNorm) return
    if (attachedNorm.has(normalizeDirPath(trimmed))) return
    if (atLimit) {
      setError(t.chatAdditionalDirectoryLimit.replace('{n}', String(MAX_ADDITIONAL_DIRECTORIES)))
      return
    }
    commit([...directories, { path: trimmed, name: name?.trim() || undefined }])
  }

  const addFromFolder = async () => {
    if (disabled || atLimit || !canAttachDirs) return
    closeMenu()
    try {
      const picked = await open({
        directory: true,
        multiple: false,
        title: t.chatAddAdditionalDirectory,
      })
      const rootPath = Array.isArray(picked) ? picked[0] : picked
      if (!rootPath) return
      addEntry(rootPath)
    } catch (err) {
      setError(typeof err === 'string' ? err : err instanceof Error ? err.message : t.chatAdditionalDirectoryFailed)
      setOpenMenu(true)
    }
  }

  const pickAttachment = () => {
    closeMenu()
    void onAddAttachment()
  }

  const placement = layout === 'inline' ? 'top-full mt-1.5' : 'bottom-full mb-1.5'
  const origin = layout === 'inline' ? 'top left' : 'bottom left'
  const maxH = usePopoverMaxHeight(openMenu, popoverRef, layout === 'inline' ? 'down' : 'up', 360)
  const active = directories.length > 0 || Boolean(sourcesActive)

  return (
    <ComposerAddMenuCloseContext.Provider value={closeMenu}>
      <div ref={ref} className="relative shrink-0">
        <IconButton
          size="sm"
          shape="circle"
          label={t.chatComposerAdd}
          onClick={toggleMenu}
          disabled={disabled}
          tabIndex={-1}
          aria-expanded={openMenu}
          aria-haspopup="menu"
          className={`shrink-0 disabled:opacity-40 ${
            active || openMenu
              ? 'bg-neutral-200 text-neutral-700 dark:bg-neutral-700 dark:text-neutral-100'
              : ''
          }`}
        >
          <Plus size={18} strokeWidth={1.75} />
        </IconButton>
        {openMenu && (
          <div
            ref={popoverRef}
            className={`chat-motion-popover chat-popover-scroll absolute left-0 z-50 w-[min(280px,calc(100vw-24px))] overflow-y-auto kv-menu ${placement}`}
            style={{ ['--chat-popover-origin' as string]: origin, maxHeight: maxH }}
            data-tauri-drag-region="false"
            role="menu"
          >
            {view === 'sources' && sourcesPanel ? (
              <>
                <button type="button" className="kv-menu-item" onClick={() => setView('root')}>
                  <span className="grid size-4 shrink-0 place-items-center text-neutral-500 dark:text-neutral-400">
                    <ChevronLeft size={13} strokeWidth={1.75} />
                  </span>
                  <span className="min-w-0 flex-1 truncate font-semibold">{t.chatComposerSources}</span>
                </button>
                <div className="kv-menu-sep" />
                {sourcesPanel}
              </>
            ) : (
              <>
                <button type="button" className="kv-menu-item" disabled={disabled} onClick={pickAttachment}>
                  <span className="grid size-4 shrink-0 place-items-center text-neutral-500 dark:text-neutral-400">
                    <Paperclip size={13} strokeWidth={1.75} />
                  </span>
                  <span className="min-w-0 flex-1 truncate">{t.chatAddAttachment}</span>
                </button>
                {canAttachDirs && (
                  <>
                    <button
                      type="button"
                      className="kv-menu-item"
                      disabled={disabled || atLimit}
                      onClick={() => void addFromFolder()}
                    >
                      <span className="grid size-4 shrink-0 place-items-center text-neutral-500 dark:text-neutral-400">
                        <FolderPlus size={13} strokeWidth={1.75} />
                      </span>
                      <span className="min-w-0 flex-1 truncate">{t.chatAddAdditionalDirectory}</span>
                    </button>
                    {promptOnly && (
                      <div className="px-2.5 py-1 text-[11px] leading-snug text-amber-700 dark:text-amber-300">
                        {t.chatAdditionalDirectoryPromptOnly.replace('{agent}', externalAgentId ?? '')}
                      </div>
                    )}
                  </>
                )}
                {sourcesPanel && (
                  <>
                    <div className="kv-menu-sep" />
                    <button
                      type="button"
                      className="kv-menu-item"
                      disabled={disabled}
                      onClick={() => setView('sources')}
                    >
                      <span className="grid size-4 shrink-0 place-items-center text-neutral-500 dark:text-neutral-400">
                        <SlidersHorizontal size={13} strokeWidth={1.75} />
                      </span>
                      <span className="min-w-0 flex-1 truncate">{t.chatComposerSources}</span>
                      <ChevronRight size={13} strokeWidth={1.75} className="shrink-0 text-neutral-400" />
                    </button>
                  </>
                )}
                {canAttachDirs && (
                  <>
                    {directories.length > 0 && (
                      <>
                        <div className="kv-menu-sep" />
                        <div className="kv-menu-label">{t.chatAdditionalDirectories}</div>
                        {directories.map((entry) => (
                          <div key={entry.path} className="kv-menu-item pr-1">
                            <span className="grid size-4 shrink-0 place-items-center text-neutral-500 dark:text-neutral-400">
                              <Folders size={13} strokeWidth={1.75} />
                            </span>
                            <span className="min-w-0 flex-1 truncate" title={entry.path}>
                              {displayName(entry)}
                            </span>
                            <button
                              type="button"
                              className="grid size-6 shrink-0 place-items-center rounded-md text-neutral-400 hover:bg-neutral-100 hover:text-neutral-700 dark:hover:bg-neutral-800 dark:hover:text-neutral-200"
                              aria-label={t.chatRemoveAdditionalDirectory.replace('{name}', displayName(entry))}
                              onClick={() =>
                                commit(directories.filter((item) => normalizeDirPath(item.path) !== normalizeDirPath(entry.path)))
                              }
                            >
                              <X size={12} strokeWidth={2} />
                            </button>
                          </div>
                        ))}
                      </>
                    )}
                    {atLimit && (
                      <div className="px-2.5 py-1 text-[11px] text-neutral-400">
                        {t.chatAdditionalDirectoryLimit.replace('{n}', String(MAX_ADDITIONAL_DIRECTORIES))}
                      </div>
                    )}
                    {error && (
                      <div className="px-2.5 py-1 text-[11px] text-rose-600 dark:text-rose-400">{error}</div>
                    )}
                  </>
                )}
              </>
            )}
          </div>
        )}
      </div>
    </ComposerAddMenuCloseContext.Provider>
  )
}
