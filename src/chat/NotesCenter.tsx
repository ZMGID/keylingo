import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  ArrowLeft,
  Check,
  ChevronLeft,
  Folder,
  FolderInput,
  FolderOpen,
  FolderPlus,
  MessageSquare,
  NotebookPen,
  Pencil,
  Plus,
  Search,
  Trash2,
} from 'lucide-react'
import { Crepe } from '@milkdown/crepe'
import '@milkdown/crepe/theme/common/style.css'
import '@milkdown/crepe/theme/frame.css'
import { api, isTauriRuntime, type Note, type NoteMeta } from '../api/tauri'
import { Button, IconButton } from '../components/Button'
import { workspaceActivity } from './dock/workspaceActivity'
import { useLang, useT } from '../settings/i18n'

const SAVE_DEBOUNCE_MS = 800

/** 顶部入口：最近（全部按时间）/ 聊天保存（对话存来）/ 库（手动笔记 + 文件夹）。 */
type NotesTab = 'recent' | 'chat' | 'library'

/**
 * Obsidian 风格的一体化写作面：Milkdown Crepe 提供 markdown 原生 live-preview
 * （输入 `# ` 直接成标题、**粗体** 内联渲染，光标行才露语法）。Crepe 是非受控编辑器，
 * defaultValue 只设一次，靠 markdownUpdated 回传变更；切笔记时用 key 重挂即可。
 */
function MilkdownNoteEditor({
  initialMarkdown,
  onChange,
}: {
  initialMarkdown: string
  onChange: (markdown: string) => void
}) {
  const rootRef = useRef<HTMLDivElement>(null)
  const onChangeRef = useRef(onChange)
  onChangeRef.current = onChange

  useEffect(() => {
    const el = rootRef.current
    if (!el) return
    const crepe = new Crepe({ root: el, defaultValue: initialMarkdown })
    crepe.on((listener) => {
      listener.markdownUpdated((_ctx, markdown) => onChangeRef.current(markdown))
    })
    const ready = crepe.create()
    return () => {
      void ready.then(() => crepe.destroy())
    }
    // 挂载一次；切笔记由外层 key 触发重挂
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  return <div ref={rootRef} className="kv-note-editor min-h-full" />
}

function formatDateTime(iso: string, locale: string): string {
  try {
    return new Date(iso).toLocaleString(locale, {
      month: 'short',
      day: 'numeric',
      hour: '2-digit',
      minute: '2-digit',
    })
  } catch {
    return iso
  }
}

function displayTitle(title: string | undefined, untitled: string): string {
  return title?.trim() || untitled
}

export function NotesCenter() {
  const t = useT()
  const lang = useLang()
  const dateLocale = lang === 'en' ? 'en-US' : 'zh-CN'
  const [notes, setNotes] = useState<NoteMeta[]>([])
  const [folders, setFolders] = useState<string[]>([])
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState('')
  const [search, setSearch] = useState('')

  const [tab, setTab] = useState<NotesTab>('recent')
  // 库内当前文件夹：null = 库根（显示文件夹 + 散笔记），字符串 = 进入该文件夹
  const [currentFolder, setCurrentFolder] = useState<string | null>(null)
  // 文件夹命名弹框：WKWebView 不支持 window.prompt（恒返回 null），故用内联输入。
  // 输入走非受控 ref，规避中文 IME 合成期受控写回吞字（与编辑器同款处理）。
  const [folderDialog, setFolderDialog] = useState<
    | { mode: 'create'; assignNoteId?: string }
    | { mode: 'rename'; original: string }
    | null
  >(null)
  const folderInputRef = useRef<HTMLInputElement>(null)
  // 卡片「移动到文件夹」菜单：当前展开的笔记 id。
  const [moveMenuFor, setMoveMenuFor] = useState<string | null>(null)

  // 编辑器态：null 表示列表态
  const [editing, setEditing] = useState<Note | null>(null)
  // 目录监听回调里读它：编辑期我们自己在写文件，不该被自己的写入触发重读。
  const editingRef = useRef(false)
  // 标题/文件夹/正文都走 ref 非受控：受控 input 在中文 IME 合成期被 React 写回 value 会打断输入 → 吞字
  const titleRef = useRef('')
  const folderRef = useRef('')
  const contentRef = useRef('')
  const [charCount, setCharCount] = useState(0)
  const [saving, setSaving] = useState(false)

  const saveTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const countTimerRef = useRef<ReturnType<typeof setTimeout> | null>(null)
  const saveRequestRef = useRef<number>(0)

  const loadNotes = useCallback(async () => {
    setError('')
    try {
      const [list, folderList] = await Promise.all([api.notesList(), api.notesFoldersList()])
      setNotes(list)
      setFolders(folderList)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      setLoading(false)
    }
  }, [])

  /**
   * 打开笔记目录，让用户把外部 `.md` 拖进来。目录是扁平的、`parse_note` 对缺
   * frontmatter 的手工文件有回退（文件名当标题、mtime 当时间），所以不需要导入
   * 流程——目录监听（见下方 effect）会在文件落地后自动刷新列表。
   */
  const openNotesFolder = useCallback(async () => {
    setError('')
    try {
      await api.notesOpenFolder()
    } catch (err) {
      setError(err instanceof Error ? err.message : t.chatNotesOpenFolderFailed)
    }
  }, [t])

  useEffect(() => {
    if (!isTauriRuntime()) {
      setLoading(false)
      setError(t.chatNotesAppOnly)
      return
    }
    void loadNotes()
  }, [loadNotes, t])

  useEffect(() => {
    editingRef.current = editing !== null
  }, [editing])

  /**
   * 监听笔记目录本身：用户把外部 `.md` 拖进来（或在别处编辑、删除）后自动重读，
   * 不需要手动刷新。复用 dock 的 workspace watcher（notify 递归监听 + 250ms 去抖
   * + 2s 轮询兜底），而不是自己起一份监听——那套去抖/兜底是实测调过的。
   * 注意 `subscribe` 的后端语义是「整体替换 watch 集合」，而 workspaceActivity
   * 模块内部按订阅方合并全集，所以这里与 dock 的 workdir 订阅可以共存。
   */
  useEffect(() => {
    if (!isTauriRuntime()) return
    let cancelled = false
    let unsubscribe: (() => void) | null = null
    void api
      .notesDirPath()
      .then((dir) => {
        if (cancelled || !dir) return
        unsubscribe = workspaceActivity.subscribe(dir, (event) => {
          // 编辑期跳过：正在编辑的笔记是我们自己在防抖写盘，重读会把列表状态
          // 拽回去；退出编辑时 backToList 已经自己 loadNotes 了。
          if (editingRef.current) return
          if (event.fs || event.truncated) void loadNotes()
        })
      })
      .catch(() => {
        // 拿不到目录就退化为「打开文件夹按钮 + 重进页面刷新」，不打扰用户。
      })
    return () => {
      cancelled = true
      unsubscribe?.()
    }
  }, [loadNotes])

  /** 库文件夹的笔记数（仅手动笔记）。 */
  const folderCounts = useMemo(() => {
    const m = new Map<string, number>()
    for (const n of notes) {
      if (n.origin === 'chat') continue
      const f = n.folder.trim()
      if (f) m.set(f, (m.get(f) ?? 0) + 1)
    }
    return m
  }, [notes])

  /** 当前 tab / 文件夹 / 搜索下可见的笔记。 */
  const visibleNotes = useMemo(() => {
    let list: NoteMeta[]
    if (tab === 'recent') {
      list = notes
    } else if (tab === 'chat') {
      list = notes.filter((n) => n.origin === 'chat')
    } else {
      const target = currentFolder ?? ''
      list = notes.filter((n) => n.origin !== 'chat' && n.folder.trim() === target)
    }
    const needle = search.trim().toLowerCase()
    if (needle) {
      list = list.filter(
        (n) =>
          displayTitle(n.title, t.chatNotesUntitled).toLowerCase().includes(needle) ||
          n.preview.toLowerCase().includes(needle),
      )
    }
    return list
  }, [notes, tab, currentFolder, search, t])

  /** 立即落盘挂起的编辑（若有变更）。 */
  const flushSave = useCallback(async () => {
    if (saveTimerRef.current) {
      clearTimeout(saveTimerRef.current)
      saveTimerRef.current = null
    }
    if (!editing) return
    const title = titleRef.current
    const content = contentRef.current
    const folder = folderRef.current
    if (title === editing.title && content === editing.content && folder === editing.folder) return

    const requestId = ++saveRequestRef.current
    setSaving(true)
    try {
      const updated = await api.notesUpdate(editing.id, title, content, folder)
      if (saveRequestRef.current === requestId) {
        setEditing(updated)
        titleRef.current = updated.title
        contentRef.current = updated.content
        folderRef.current = updated.folder
      }
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    } finally {
      if (saveRequestRef.current === requestId) {
        setSaving(false)
      }
    }
  }, [editing])

  const scheduleSave = useCallback(() => {
    if (saveTimerRef.current) {
      clearTimeout(saveTimerRef.current)
    }
    saveTimerRef.current = setTimeout(() => {
      saveTimerRef.current = null
      void flushSave()
    }, SAVE_DEBOUNCE_MS)
  }, [flushSave])

  /** 编辑器回传：正文只落 ref + 防抖保存；字数计数节流刷新，不逐字触发重渲染。 */
  const onEditorChange = useCallback(
    (markdown: string) => {
      contentRef.current = markdown
      scheduleSave()
      if (!countTimerRef.current) {
        countTimerRef.current = setTimeout(() => {
          countTimerRef.current = null
          setCharCount(contentRef.current.length)
        }, 400)
      }
    },
    [scheduleSave],
  )

  const openNote = useCallback(
    async (id: string) => {
      await flushSave()
      setError('')
      try {
        const note = await api.notesRead(id)
        setEditing(note)
        titleRef.current = note.title
        contentRef.current = note.content
        folderRef.current = note.folder
        setCharCount(note.content.length)
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err))
      }
    },
    [flushSave],
  )

  const backToList = useCallback(async () => {
    await flushSave()
    setEditing(null)
    titleRef.current = ''
    folderRef.current = ''
    contentRef.current = ''
    void loadNotes()
  }, [flushSave, loadNotes])

  const createNote = useCallback(async () => {
    setError('')
    // 库内新建归入当前文件夹；其他视图归库根。手动笔记一律 origin=user。
    const folder = tab === 'library' && currentFolder ? currentFolder : ''
    try {
      const note = await api.notesCreate('', '', folder, 'user')
      await loadNotes()
      setEditing(note)
      titleRef.current = note.title
      contentRef.current = note.content
      folderRef.current = note.folder
      setCharCount(0)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [loadNotes, tab, currentFolder])

  const deleteNote = useCallback(
    async (id: string) => {
      const meta = notes.find((n) => n.id === id)
      const ok = window.confirm(
        t.chatNotesDeleteNoteConfirm.replace('{title}', displayTitle(meta?.title, t.chatNotesUntitled)),
      )
      if (!ok) return
      setError('')
      try {
        await api.notesDelete(id)
        if (editing?.id === id) {
          setEditing(null)
          titleRef.current = ''
          folderRef.current = ''
          contentRef.current = ''
        }
        setNotes((prev) => prev.filter((n) => n.id !== id))
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err))
      }
    },
    [editing?.id, notes, t],
  )

  /* ===== 文件夹管理（用原生 prompt/confirm，不做自定义弹窗） ===== */
  const moveNoteToFolder = useCallback(
    async (id: string, folder: string) => {
      setMoveMenuFor(null)
      setError('')
      try {
        const note = await api.notesRead(id)
        if (note.folder === folder) return
        await api.notesUpdate(id, note.title, note.content, folder)
        await loadNotes()
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err))
      }
    },
    [loadNotes],
  )

  const createFolder = useCallback(() => {
    setFolderDialog({ mode: 'create' })
  }, [])

  const renameFolder = useCallback((name: string) => {
    setFolderDialog({ mode: 'rename', original: name })
  }, [])

  const submitFolderDialog = useCallback(async () => {
    if (!folderDialog) return
    const name = (folderInputRef.current?.value ?? '').trim()
    if (!name) return
    if (folderDialog.mode === 'rename' && name === folderDialog.original) {
      setFolderDialog(null)
      return
    }
    setError('')
    try {
      if (folderDialog.mode === 'create') {
        setFolders(await api.notesFolderCreate(name))
        // 若来自卡片「新建文件夹并移入」，创建后把该笔记归入新文件夹。
        if (folderDialog.assignNoteId) {
          const note = await api.notesRead(folderDialog.assignNoteId)
          await api.notesUpdate(note.id, note.title, note.content, name)
          await loadNotes()
        }
      } else {
        await api.notesFolderRename(folderDialog.original, name)
        if (currentFolder === folderDialog.original) setCurrentFolder(name)
        await loadNotes()
      }
      setFolderDialog(null)
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err))
    }
  }, [folderDialog, currentFolder, loadNotes])

  const deleteFolder = useCallback(
    async (name: string) => {
      const ok = window.confirm(t.chatNotesDeleteFolderConfirm.replace('{name}', name))
      if (!ok) return
      setError('')
      try {
        await api.notesFolderDelete(name)
        if (currentFolder === name) setCurrentFolder(null)
        await loadNotes()
      } catch (err) {
        setError(err instanceof Error ? err.message : String(err))
      }
    },
    [currentFolder, loadNotes, t],
  )

  const changeTab = useCallback((next: NotesTab) => {
    setTab(next)
    setCurrentFolder(null)
    setSearch('')
  }, [])

  /* ===== 编辑器态 ===== */
  if (editing) {
    const isChat = editing.origin === 'chat'
    return (
      <div className="assistant-center-root flex h-full min-h-0 flex-col text-neutral-900 dark:text-neutral-100">
        <div className="mx-auto flex h-full w-full min-h-0 max-w-[820px] flex-col px-9 pb-4 pt-6">
          <div className="flex shrink-0 items-center justify-between gap-3">
            <Button variant="ghost" size="sm" onClick={() => void backToList()}>
              <ArrowLeft size={14} />
              {t.chatNotesBack}
            </Button>
            <div className="flex shrink-0 items-center gap-2">
              <span className="text-[12px] text-neutral-400 dark:text-neutral-500">
                {saving ? t.annotateSaving : t.saved}
              </span>
              <IconButton
                size="sm"
                variant="ghost"
                label={t.chatDelete}
                onClick={() => void deleteNote(editing.id)}
              >
                <Trash2 size={15} />
              </IconButton>
            </div>
          </div>

          <input
            key={editing.id}
            type="text"
            defaultValue={editing.title}
            onChange={(e) => {
              titleRef.current = e.target.value
              scheduleSave()
            }}
            placeholder={t.chatNotesUntitled}
            className="mt-5 w-full shrink-0 bg-transparent text-[26px] font-semibold tracking-normal text-neutral-950 placeholder:text-neutral-300 focus:outline-none dark:text-neutral-50 dark:placeholder:text-neutral-600"
          />
          <p className="mt-1.5 shrink-0 text-[12px] text-neutral-400 dark:text-neutral-500">
            {t.chatNotesUpdatedInfo
              .replace('{time}', formatDateTime(editing.updatedAt, dateLocale))
              .replace('{n}', String(charCount))}
          </p>

          {isChat && (
            <div className="mt-2.5 flex shrink-0 items-center gap-1.5">
              <span className="inline-flex items-center gap-1.5 rounded-md bg-neutral-100/70 px-2 py-0.5 text-[12.5px] text-neutral-500 dark:bg-neutral-800/60 dark:text-neutral-400">
                <MessageSquare size={13} />
                {t.chatNotesFromChat}
              </span>
            </div>
          )}

          <div className="custom-scrollbar mt-3 min-h-0 flex-1 overflow-y-auto">
            <MilkdownNoteEditor
              key={editing.id}
              initialMarkdown={editing.content}
              onChange={onEditorChange}
            />
          </div>

          {error && (
            <div className="mt-3 shrink-0 rounded-md border border-red-200 bg-red-50 px-4 py-2.5 text-[13px] text-red-700 dark:border-red-900/50 dark:bg-red-950/30 dark:text-red-300">
              {error}
            </div>
          )}
        </div>
      </div>
    )
  }

  /* ===== 列表态 ===== */
  const inLibraryRoot = tab === 'library' && currentFolder === null
  const showFolderGrid = inLibraryRoot && folders.length > 0
  const emptyEverything = !showFolderGrid && visibleNotes.length === 0

  const emptyText =
    tab === 'chat'
      ? t.chatNotesEmptyChat
      : tab === 'library'
        ? currentFolder
          ? t.chatNotesEmptyFolder
          : t.chatNotesEmptyLibrary
        : t.chatNotesEmptyRecent

  return (
    <div className="assistant-center-root flex h-full min-h-0 flex-col text-neutral-900 dark:text-neutral-100">
      <main className="custom-scrollbar min-h-0 flex-1 overflow-y-auto">
        <div className="mx-auto w-full max-w-[1040px] px-9 pb-10 pt-7">
          {/* 头部：标题 + 副标题 */}
          <div className="border-b border-neutral-200 pb-5 dark:border-neutral-800">
            <h1 className="flex items-center gap-2.5 text-[28px] font-semibold tracking-normal text-neutral-950 dark:text-neutral-50">
              <NotebookPen size={24} className="text-neutral-500" />
              {t.chatNavNotes}
            </h1>
            <p className="mt-3 text-[14px] leading-relaxed text-neutral-500 dark:text-neutral-400">
              {t.chatNotesSubtitle}
            </p>
          </div>

          {/* 一行：tab（左） + 搜索（中） + 操作（右） */}
          <div className="mt-5 flex items-center gap-3">
            <div className="flex shrink-0 items-center gap-1 rounded-lg bg-neutral-100 p-0.5 dark:bg-neutral-800/80">
              {(
                [
                  ['recent', t.chatTabRecent],
                  ['chat', t.chatNotesTabChat],
                  ['library', t.chatNotesTabLibrary],
                ] as const
              ).map(([id, label]) => (
                <button
                  key={id}
                  type="button"
                  onClick={() => changeTab(id)}
                  className={`rounded-md px-3.5 py-1.5 text-[13px] transition-colors ${
                    tab === id
                      ? 'bg-white font-medium text-neutral-900 shadow-sm dark:bg-neutral-900 dark:text-neutral-50'
                      : 'text-neutral-500 hover:text-neutral-800 dark:text-neutral-400 dark:hover:text-neutral-200'
                  }`}
                >
                  {label}
                </button>
              ))}
            </div>

            <div className="relative w-full max-w-xs">
              <Search size={14} className="absolute left-2.5 top-1/2 -translate-y-1/2 text-neutral-400" />
              <input
                type="text"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                placeholder={t.chatNotesSearchPlaceholder}
                className="w-full rounded-lg border border-neutral-200 bg-white py-1.5 pl-8 pr-3 text-[13px] text-neutral-800 placeholder:text-neutral-400 focus:border-neutral-400 focus:outline-none dark:border-neutral-700 dark:bg-neutral-900 dark:text-neutral-100"
              />
            </div>
            {visibleNotes.length > 0 && (
              <span className="shrink-0 text-[12px] tabular-nums text-neutral-400 dark:text-neutral-500">
                {t.chatNotesCount.replace('{n}', String(visibleNotes.length))}
              </span>
            )}

            <div className="ml-auto flex shrink-0 items-center gap-2">
              <Button variant="ghost" onClick={() => void openNotesFolder()}>
                <FolderOpen size={14} />
                {t.chatNotesOpenFolder}
              </Button>
              {inLibraryRoot && (
                <Button variant="ghost" onClick={() => void createFolder()}>
                  <FolderPlus size={14} />
                  {t.dockNewFolder}
                </Button>
              )}
              {tab !== 'chat' && (
                <Button onClick={() => void createNote()}>
                  <Plus size={14} />
                  {t.chatNotesNewNote}
                </Button>
              )}
            </div>
          </div>

          {/* 库文件夹内的面包屑返回 */}
          {tab === 'library' && currentFolder !== null && (
            <button
              type="button"
              onClick={() => setCurrentFolder(null)}
              className="mt-4 inline-flex items-center gap-1 text-[13px] text-neutral-500 hover:text-neutral-800 dark:text-neutral-400 dark:hover:text-neutral-200"
            >
              <ChevronLeft size={15} />
              {t.chatNotesTabLibrary}
              <span className="text-neutral-300 dark:text-neutral-600">/</span>
              <span className="font-medium text-neutral-700 dark:text-neutral-200">{currentFolder}</span>
            </button>
          )}

          {error && (
            <div className="mt-4 rounded-md border border-red-200 bg-red-50 px-4 py-3 text-[13px] text-red-700 dark:border-red-900/50 dark:bg-red-950/30 dark:text-red-300">
              {error}
            </div>
          )}

          {loading && notes.length === 0 ? (
            <div className="mt-6 grid gap-4 sm:grid-cols-2 xl:grid-cols-3">
              {Array.from({ length: 3 }, (_, i) => (
                <div key={i} className="rounded-xl border border-neutral-200/80 p-4 dark:border-neutral-800/70">
                  <div className="kv-skeleton h-4 w-1/3 rounded" />
                  <div className="kv-skeleton mt-2.5 h-3 w-full rounded" />
                  <div className="kv-skeleton mt-1.5 h-3 w-2/3 rounded" />
                  <div className="kv-skeleton mt-4 h-3 w-16 rounded" />
                </div>
              ))}
            </div>
          ) : (
            <>
              {/* 库根：文件夹卡片 */}
              {showFolderGrid && (
                <div className="mt-5 grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
                  {folders.map((name) => (
                    <div
                      key={name}
                      role="button"
                      tabIndex={0}
                      onClick={() => setCurrentFolder(name)}
                      onKeyDown={(e) => {
                        if (e.key === 'Enter' || e.key === ' ') {
                          e.preventDefault()
                          setCurrentFolder(name)
                        }
                      }}
                      className="group flex cursor-pointer items-center gap-3 rounded-xl border border-neutral-200 bg-white p-3.5 shadow-sm transition-[border-color,box-shadow] duration-[var(--kv-dur-fast)] hover:border-neutral-300 hover:shadow dark:border-neutral-800 dark:bg-neutral-950/40 dark:hover:border-neutral-700"
                    >
                      <Folder size={20} className="shrink-0 text-neutral-400 dark:text-neutral-500" />
                      <div className="min-w-0 flex-1">
                        <div className="truncate text-[14px] font-medium text-neutral-900 dark:text-neutral-50">
                          {name}
                        </div>
                        <div className="text-[11px] tabular-nums text-neutral-400 dark:text-neutral-500">
                          {t.chatNotesCount.replace('{n}', String(folderCounts.get(name) ?? 0))}
                        </div>
                      </div>
                      <div className="flex shrink-0 items-center gap-0.5 opacity-0 transition-opacity group-hover:opacity-100">
                        <IconButton
                          size="xs"
                          variant="ghost"
                          label={t.chatRename}
                          onClick={(e) => {
                            e.stopPropagation()
                            void renameFolder(name)
                          }}
                        >
                          <Pencil size={13} />
                        </IconButton>
                        <IconButton
                          size="xs"
                          variant="ghost"
                          label={t.chatDelete}
                          onClick={(e) => {
                            e.stopPropagation()
                            void deleteFolder(name)
                          }}
                        >
                          <Trash2 size={13} />
                        </IconButton>
                      </div>
                    </div>
                  ))}
                </div>
              )}

              {/* 空状态 */}
              {emptyEverything ? (
                <div className="mt-16 flex flex-col items-center justify-center text-center">
                  <div className="flex h-14 w-14 items-center justify-center rounded-md bg-neutral-100 text-neutral-400 dark:bg-neutral-800 dark:text-neutral-500">
                    {tab === 'chat' ? (
                      <MessageSquare size={28} strokeWidth={1.5} />
                    ) : (
                      <NotebookPen size={28} strokeWidth={1.5} />
                    )}
                  </div>
                  <p className="mt-4 text-[15px] font-medium text-neutral-700 dark:text-neutral-200">
                    {search.trim() ? t.chatNotesNoMatch : emptyText}
                  </p>
                  {!search.trim() && tab === 'chat' && (
                    <p className="mt-1 text-[13px] text-neutral-500 dark:text-neutral-400">
                      {t.chatNotesSaveHint}
                    </p>
                  )}
                  {!search.trim() && tab !== 'chat' && (
                    <div className="mt-5 flex items-center gap-2">
                      {inLibraryRoot && (
                        <Button variant="ghost" onClick={() => void createFolder()}>
                          <FolderPlus size={14} />
                          {t.dockNewFolder}
                        </Button>
                      )}
                      <Button onClick={() => void createNote()}>
                        <Plus size={14} />
                        {t.chatNotesNewNote}
                      </Button>
                    </div>
                  )}
                </div>
              ) : (
                visibleNotes.length > 0 && (
                  <div className="chat-motion-tab-in mt-5 grid items-start gap-4 sm:grid-cols-2 xl:grid-cols-3">
                    {visibleNotes.map((note) => (
                      <article
                        key={note.id}
                        role="button"
                        tabIndex={0}
                        onClick={() => void openNote(note.id)}
                        onKeyDown={(e) => {
                          if (e.key === 'Enter' || e.key === ' ') {
                            e.preventDefault()
                            void openNote(note.id)
                          }
                        }}
                        className="chat-motion-fade-up group flex min-h-[132px] min-w-0 cursor-pointer flex-col gap-2 rounded-xl border border-neutral-200 bg-white p-4 shadow-sm transition-[border-color,box-shadow] duration-[var(--kv-dur-fast)] hover:border-neutral-300 hover:shadow dark:border-neutral-800 dark:bg-neutral-950/40 dark:hover:border-neutral-700"
                      >
                        <div className="flex min-w-0 items-start justify-between gap-2">
                          <h3 className="min-w-0 flex-1 truncate text-[15px] font-semibold text-neutral-900 dark:text-neutral-50">
                            {displayTitle(note.title, t.chatNotesUntitled)}
                          </h3>
                          <div className="flex shrink-0 items-center gap-0.5">
                            <div className="relative">
                              <IconButton
                                size="xs"
                                variant="ghost"
                                label={t.chatNotesMoveToFolder}
                                className={`transition-opacity ${moveMenuFor === note.id ? 'opacity-100' : 'opacity-0 group-hover:opacity-100'}`}
                                onClick={(e) => {
                                  e.stopPropagation()
                                  setMoveMenuFor((prev) => (prev === note.id ? null : note.id))
                                }}
                              >
                                <FolderInput size={13} />
                              </IconButton>
                              {moveMenuFor === note.id && (
                                <>
                                  <div
                                    className="fixed inset-0 z-40"
                                    onClick={(e) => {
                                      e.stopPropagation()
                                      setMoveMenuFor(null)
                                    }}
                                  />
                                  <div
                                    className="absolute right-0 z-50 mt-1 max-h-64 w-44 overflow-auto kv-menu"
                                    onClick={(e) => e.stopPropagation()}
                                  >
                                    <button
                                      type="button"
                                      className="kv-menu-item"
                                      onClick={() => void moveNoteToFolder(note.id, '')}
                                    >
                                      {note.folder.trim() === '' && <Check size={12} className="text-[#2f6ff0]" />}
                                      <span className={note.folder.trim() === '' ? '' : 'ml-[18px]'}>{t.chatNotesLibraryRoot}</span>
                                    </button>
                                    {folders.map((f) => (
                                      <button
                                        key={f}
                                        type="button"
                                        className="kv-menu-item truncate"
                                        onClick={() => void moveNoteToFolder(note.id, f)}
                                      >
                                        {note.folder.trim() === f && <Check size={12} className="text-[#2f6ff0]" />}
                                        <span className={`truncate ${note.folder.trim() === f ? '' : 'ml-[18px]'}`}>{f}</span>
                                      </button>
                                    ))}
                                    <div className="my-1 border-t border-neutral-100 dark:border-neutral-800" />
                                    <button
                                      type="button"
                                      className="kv-menu-item"
                                      onClick={() => {
                                        setMoveMenuFor(null)
                                        setFolderDialog({ mode: 'create', assignNoteId: note.id })
                                      }}
                                    >
                                      <FolderPlus size={12} />
                                      {t.chatNotesNewFolderEllipsis}
                                    </button>
                                  </div>
                                </>
                              )}
                            </div>
                            <IconButton
                              size="xs"
                              variant="ghost"
                              label={t.chatDelete}
                              className={`transition-opacity ${moveMenuFor === note.id ? 'opacity-100' : 'opacity-0 group-hover:opacity-100'}`}
                              onClick={(e) => {
                                e.stopPropagation()
                                void deleteNote(note.id)
                              }}
                            >
                              <Trash2 size={13} />
                            </IconButton>
                          </div>
                        </div>
                        <p className="line-clamp-3 min-w-0 flex-1 text-[13px] leading-relaxed text-neutral-500 dark:text-neutral-400">
                          {note.preview || <span className="text-neutral-300 dark:text-neutral-600">{t.chatNotesNoContent}</span>}
                        </p>
                        <div className="mt-auto flex shrink-0 items-center justify-between gap-2">
                          <span className="text-[11px] tabular-nums text-neutral-400 dark:text-neutral-500">
                            {formatDateTime(note.updatedAt, dateLocale)}
                          </span>
                          {/* 最近视图里标注来源/文件夹，便于区分 */}
                          {tab === 'recent' && note.origin === 'chat' && (
                            <span className="inline-flex items-center gap-1 text-[11px] text-neutral-400 dark:text-neutral-500">
                              <MessageSquare size={11} />
                              {t.chatNotesSourceChat}
                            </span>
                          )}
                          {tab === 'recent' && note.origin !== 'chat' && note.folder.trim() && (
                            <span className="inline-flex max-w-[50%] items-center gap-1 truncate text-[11px] text-neutral-400 dark:text-neutral-500">
                              <Folder size={11} />
                              {note.folder.trim()}
                            </span>
                          )}
                        </div>
                      </article>
                    ))}
                  </div>
                )
              )}
            </>
          )}
        </div>
      </main>

      {folderDialog && (
        <div
          className="fixed inset-0 z-50 flex items-center justify-center bg-black/30 px-4"
          onMouseDown={() => setFolderDialog(null)}
        >
          <div
            className="w-full max-w-xs rounded-xl border border-neutral-200 bg-white p-4 shadow-xl dark:border-neutral-700 dark:bg-neutral-900"
            onMouseDown={(e) => e.stopPropagation()}
          >
            <h3 className="text-[14px] font-semibold text-neutral-900 dark:text-neutral-100">
              {folderDialog.mode === 'create' ? t.dockNewFolder : t.chatRename}
            </h3>
            <input
              ref={folderInputRef}
              autoFocus
              defaultValue={folderDialog.mode === 'rename' ? folderDialog.original : ''}
              onKeyDown={(e) => {
                if (e.key === 'Enter') {
                  e.preventDefault()
                  void submitFolderDialog()
                }
                if (e.key === 'Escape') setFolderDialog(null)
              }}
              placeholder={t.chatNotesFolderNamePlaceholder}
              className="mt-3 w-full rounded-lg border border-neutral-300 bg-white px-3 py-2 text-[13px] text-neutral-900 outline-none focus:border-[#2f6ff0] dark:border-neutral-600 dark:bg-neutral-800 dark:text-neutral-100"
            />
            <div className="mt-4 flex justify-end gap-2">
              <Button variant="ghost" size="sm" onClick={() => setFolderDialog(null)}>
                {t.cancel}
              </Button>
              <Button size="sm" onClick={() => void submitFolderDialog()}>
                {t.chatNotesConfirm}
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
