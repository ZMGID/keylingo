// 文件树面板：懒加载树（virtua 虚拟化）+ 搜索 + 内联新建/重命名 + 右键菜单 + 删除确认。
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { VList, type VListHandle } from 'virtua'
import {
  AtSign,
  ChevronDown,
  ChevronRight,
  Copy,
  Database,
  ExternalLink,
  Eye,
  EyeOff,
  File,
  FileArchive,
  FileCode,
  FileCog,
  FileImage,
  FileJson,
  FileLock,
  FilePlus2,
  FileText,
  Folder,
  FolderOpen,
  FolderPlus,
  Loader2,
  Pencil,
  RefreshCw,
  Search,
  Terminal,
  Trash2,
  X,
} from 'lucide-react'
import { i18n, type Lang } from '../../settings/i18n'
import { IconButton } from '../../components/Button'
import { ConfirmDialog } from './ConfirmDialog'
import { DockContextMenu, type DockMenuAnchor, type DockMenuItem } from './DockContextMenu'
import {
  addExpanded,
  ancestorsOf,
  basenameOf,
  flattenTreeRows,
  parentDirOf,
  remapExpandedForRename,
  ROOT_PATH,
  toggleExpanded,
  type FileTreeNode,
} from './fileTreeModel'
import { useFileTree } from './useFileTree'
import { FileViewer } from './FileViewer'
import { DiffView } from './DiffView'
import type { DockFsEntry } from './types'
import type { DockPreviewRequest } from './RightDock'

const ROW_HEIGHT = 28

type IconComponent = typeof File

type FileVisual = { Icon: IconComponent; className: string }

/** 扩展名 → 图标 + 颜色（对齐 VS Code 文件图标主题的语言色：JS 黄 / TS 蓝 / Rust 橙…）。 */
const FILE_VISUAL_BY_EXT: Record<string, FileVisual> = {
  ts: { Icon: FileCode, className: 'text-sky-500 dark:text-sky-400' },
  tsx: { Icon: FileCode, className: 'text-sky-500 dark:text-sky-400' },
  mts: { Icon: FileCode, className: 'text-sky-500 dark:text-sky-400' },
  cts: { Icon: FileCode, className: 'text-sky-500 dark:text-sky-400' },
  js: { Icon: FileCode, className: 'text-yellow-500 dark:text-yellow-400' },
  jsx: { Icon: FileCode, className: 'text-yellow-500 dark:text-yellow-400' },
  mjs: { Icon: FileCode, className: 'text-yellow-500 dark:text-yellow-400' },
  cjs: { Icon: FileCode, className: 'text-yellow-500 dark:text-yellow-400' },
  vue: { Icon: FileCode, className: 'text-emerald-500 dark:text-emerald-400' },
  svelte: { Icon: FileCode, className: 'text-orange-500 dark:text-orange-400' },
  rs: { Icon: FileCode, className: 'text-orange-600 dark:text-orange-400' },
  py: { Icon: FileCode, className: 'text-green-600 dark:text-green-400' },
  go: { Icon: FileCode, className: 'text-cyan-600 dark:text-cyan-400' },
  java: { Icon: FileCode, className: 'text-red-500 dark:text-red-400' },
  kt: { Icon: FileCode, className: 'text-violet-500 dark:text-violet-400' },
  kts: { Icon: FileCode, className: 'text-violet-500 dark:text-violet-400' },
  c: { Icon: FileCode, className: 'text-blue-500 dark:text-blue-400' },
  h: { Icon: FileCode, className: 'text-blue-500 dark:text-blue-400' },
  cc: { Icon: FileCode, className: 'text-indigo-500 dark:text-indigo-400' },
  cpp: { Icon: FileCode, className: 'text-indigo-500 dark:text-indigo-400' },
  cxx: { Icon: FileCode, className: 'text-indigo-500 dark:text-indigo-400' },
  hpp: { Icon: FileCode, className: 'text-indigo-500 dark:text-indigo-400' },
  rb: { Icon: FileCode, className: 'text-rose-500 dark:text-rose-400' },
  php: { Icon: FileCode, className: 'text-violet-400 dark:text-violet-300' },
  swift: { Icon: FileCode, className: 'text-orange-500 dark:text-orange-400' },
  html: { Icon: FileCode, className: 'text-orange-500 dark:text-orange-400' },
  htm: { Icon: FileCode, className: 'text-orange-500 dark:text-orange-400' },
  css: { Icon: FileCode, className: 'text-sky-500 dark:text-sky-400' },
  scss: { Icon: FileCode, className: 'text-pink-500 dark:text-pink-400' },
  less: { Icon: FileCode, className: 'text-pink-500 dark:text-pink-400' },
  sh: { Icon: Terminal, className: 'text-emerald-600 dark:text-emerald-400' },
  bash: { Icon: Terminal, className: 'text-emerald-600 dark:text-emerald-400' },
  zsh: { Icon: Terminal, className: 'text-emerald-600 dark:text-emerald-400' },
  ps1: { Icon: Terminal, className: 'text-sky-500 dark:text-sky-400' },
  bat: { Icon: Terminal, className: 'text-sky-500 dark:text-sky-400' },
  sql: { Icon: Database, className: 'text-sky-500 dark:text-sky-400' },
  json: { Icon: FileJson, className: 'text-amber-500 dark:text-amber-400' },
  jsonc: { Icon: FileJson, className: 'text-amber-500 dark:text-amber-400' },
  yaml: { Icon: FileJson, className: 'text-rose-400 dark:text-rose-300' },
  yml: { Icon: FileJson, className: 'text-rose-400 dark:text-rose-300' },
  toml: { Icon: FileJson, className: 'text-amber-600 dark:text-amber-400' },
  xml: { Icon: FileCode, className: 'text-orange-400 dark:text-orange-300' },
  ini: { Icon: FileCog, className: 'text-neutral-400' },
  cfg: { Icon: FileCog, className: 'text-neutral-400' },
  conf: { Icon: FileCog, className: 'text-neutral-400' },
  env: { Icon: FileCog, className: 'text-neutral-400' },
  md: { Icon: FileText, className: 'text-sky-500 dark:text-sky-400' },
  markdown: { Icon: FileText, className: 'text-sky-500 dark:text-sky-400' },
  txt: { Icon: FileText, className: 'text-neutral-400' },
  log: { Icon: FileText, className: 'text-neutral-400' },
  pdf: { Icon: FileText, className: 'text-red-500 dark:text-red-400' },
  png: { Icon: FileImage, className: 'text-violet-500 dark:text-violet-400' },
  jpg: { Icon: FileImage, className: 'text-violet-500 dark:text-violet-400' },
  jpeg: { Icon: FileImage, className: 'text-violet-500 dark:text-violet-400' },
  gif: { Icon: FileImage, className: 'text-violet-500 dark:text-violet-400' },
  webp: { Icon: FileImage, className: 'text-violet-500 dark:text-violet-400' },
  ico: { Icon: FileImage, className: 'text-violet-500 dark:text-violet-400' },
  bmp: { Icon: FileImage, className: 'text-violet-500 dark:text-violet-400' },
  svg: { Icon: FileImage, className: 'text-orange-400 dark:text-orange-300' },
  zip: { Icon: FileArchive, className: 'text-amber-600 dark:text-amber-400' },
  tar: { Icon: FileArchive, className: 'text-amber-600 dark:text-amber-400' },
  gz: { Icon: FileArchive, className: 'text-amber-600 dark:text-amber-400' },
  '7z': { Icon: FileArchive, className: 'text-amber-600 dark:text-amber-400' },
  rar: { Icon: FileArchive, className: 'text-amber-600 dark:text-amber-400' },
  lock: { Icon: FileLock, className: 'text-neutral-400' },
}

const DEFAULT_FILE_VISUAL: FileVisual = { Icon: File, className: 'text-neutral-400' }

function fileVisualFor(name: string): FileVisual {
  const lower = name.toLowerCase()
  const ext = lower.includes('.') ? lower.slice(lower.lastIndexOf('.') + 1) : ''
  return FILE_VISUAL_BY_EXT[ext] ?? DEFAULT_FILE_VISUAL
}

type EditingState = {
  mode: 'create-file' | 'create-dir' | 'rename'
  /** create：父目录路径（ROOT_PATH 表示根）。 */
  dirPath: string
  /** rename：目标路径。 */
  path?: string
  value: string
  error?: string
}

type DisplayRow =
  | { kind: 'node'; path: string; depth: number }
  | { kind: 'error'; path: string; depth: number }
  | { kind: 'edit'; depth: number }

type FileTreePanelProps = {
  workdir: string
  active: boolean
  lang: Lang
  expandedPaths: string[]
  onExpandedChange: (paths: string[]) => void
  /** 外部「定位到此文件」请求；nonce 变化才触发（同一路径可重复 reveal）。 */
  revealPath?: string | null
  revealNonce?: number
  /** 工具卡片点文件名 → 查看器预览（workdir 可以不同于文件树）。 */
  previewRequest?: DockPreviewRequest
  onInsertMention?: (path: string) => void
}

export function FileTreePanel({
  workdir,
  active,
  lang,
  expandedPaths,
  onExpandedChange,
  revealPath = null,
  revealNonce = 0,
  previewRequest = null,
  onInsertMention,
}: FileTreePanelProps) {
  const t = i18n[lang]
  const [showHidden, setShowHidden] = useState(false)
  const expandedSet = useMemo(() => new Set(expandedPaths), [expandedPaths])
  const tree = useFileTree({ workdir, active, showHidden, expandedPaths: expandedSet })

  /** 多选集合（框选 / Ctrl / Shift）；单击 = 单选。 */
  const [selected, setSelected] = useState<ReadonlySet<string>>(new Set())
  /** Shift 范围选择的锚点（最近一次非 Shift 的点选）。 */
  const selectionAnchorRef = useRef<string | null>(null)
  const [editing, setEditing] = useState<EditingState | null>(null)
  /** 就地查看器：点树里的文件 / 工具卡片文件预览（workdir 任意）/ 工具卡片 diff 预览。 */
  const [viewer, setViewer] = useState<
    | { kind: 'file'; workdir: string; path: string }
    | { kind: 'diff'; title: string; patch: string }
    | null
  >(null)
  /** 拖拽悬停中的目标目录（ROOT_PATH = 根）。 */
  const [dropTarget, setDropTarget] = useState<string | null>(null)
  /** 框选矩形（树容器视口坐标，仅显示用；选中集在 mousemove 里按内容坐标算）。 */
  const [marqueeRect, setMarqueeRect] = useState<{ left: number; top: number; width: number; height: number } | null>(null)
  const treeAreaRef = useRef<HTMLDivElement>(null)
  const [menu, setMenu] = useState<{ anchor: DockMenuAnchor; path: string | null } | null>(null)
  const [deleteTarget, setDeleteTarget] = useState<FileTreeNode[] | null>(null)
  const [deleting, setDeleting] = useState(false)
  const vlistRef = useRef<VListHandle>(null)
  const editInputRef = useRef<HTMLInputElement>(null)
  // 提交/取消要读「最新」编辑态：Enter→blur、Escape→blur 这类连发事件里，闭包里的
  // editing 是旧渲染的快照，会把已经关闭的编辑器再提交一次。
  const editingRef = useRef(editing)
  editingRef.current = editing
  // 提交进行中标记：挡住双击回车/回车后紧跟的 blur 造成的重复创建（第二次会报
  // 「目标已存在」并把已关闭的输入行带着错误复活）。
  const submittingRef = useRef(false)

  const emitExpanded = useCallback(
    (next: ReadonlySet<string>) => onExpandedChange([...next]),
    [onExpandedChange],
  )

  useEffect(() => {
    setSelected(new Set())
    selectionAnchorRef.current = null
    setEditing(null)
    setMenu(null)
    setDeleteTarget(null)
    setViewer(null)
    setDropTarget(null)
    dropTargetRef.current = null
  }, [workdir])

  // ---------- 行数据 ----------

  const rows = useMemo<DisplayRow[]>(() => {
    const base: DisplayRow[] = flattenTreeRows(tree.nodes, expandedSet).map((row) => ({
      kind: row.type,
      path: row.path,
      depth: row.depth,
    }))
    if (!editing) return base
    if (editing.mode === 'rename' && editing.path) {
      return base.map((row) =>
        row.kind === 'node' && row.path === editing.path ? { kind: 'edit', depth: row.depth } : row,
      )
    }
    // create：插入到父目录行之后（根创建则置顶）。
    const result: DisplayRow[] = []
    const parentIndex = base.findIndex((row) => row.kind === 'node' && row.path === editing.dirPath)
    for (let i = 0; i < base.length; i += 1) {
      result.push(base[i])
      if (i === parentIndex) {
        result.push({ kind: 'edit', depth: base[i].depth + 1 })
      }
    }
    if (parentIndex < 0 && editing.dirPath === ROOT_PATH) {
      result.unshift({ kind: 'edit', depth: 0 })
    }
    return result
  }, [tree.nodes, expandedSet, editing])

  // ---------- reveal 定位 ----------

  const pendingRevealRef = useRef<string | null>(null)
  useEffect(() => {
    if (!revealPath || revealNonce === 0) return
    let next = expandedSet
    for (const ancestor of ancestorsOf(revealPath)) {
      next = addExpanded(next, ancestor)
    }
    emitExpanded(next)
    for (const ancestor of ancestorsOf(revealPath)) {
      void tree.loadChildren(ancestor)
    }
    pendingRevealRef.current = revealPath
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [revealNonce])

  useEffect(() => {
    const target = pendingRevealRef.current
    if (!target || tree.searchResults !== null) return
    const index = rows.findIndex((row) => row.kind === 'node' && row.path === target)
    if (index < 0) return
    pendingRevealRef.current = null
    setSelected(new Set([target]))
    selectionAnchorRef.current = target
    vlistRef.current?.scrollToIndex(index, { align: 'center' })
  }, [rows, tree.searchResults])

  // 工具卡片预览请求：nonce 变化直接开查看器（workdir 由请求携带，可以在树外）。
  useEffect(() => {
    if (!previewRequest || previewRequest.nonce === 0) return
    setViewer(
      previewRequest.kind === 'file'
        ? { kind: 'file', workdir: previewRequest.workdir, path: previewRequest.path }
        : { kind: 'diff', title: previewRequest.title, patch: previewRequest.patch },
    )
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [previewRequest?.nonce])

  // ---------- 操作 ----------

  const handleToggleDir = useCallback(
    (node: FileTreeNode) => {
      const next = toggleExpanded(expandedSet, node.path)
      emitExpanded(next)
      if (next.has(node.path)) void tree.loadChildren(node.path)
    },
    [expandedSet, emitExpanded, tree],
  )

  const handleOpenEntry = useCallback(
    (path: string, mode: 'open' | 'reveal') => {
      void tree.openEntry(path, mode).catch(() => {})
    },
    [tree],
  )

  const startCreate = (dirPath: string, kind: 'file' | 'dir') => {
    // 在非根目录里新建时确保父目录展开，否则输入行不可见。
    if (dirPath !== ROOT_PATH && !expandedSet.has(dirPath)) {
      emitExpanded(addExpanded(expandedSet, dirPath))
      void tree.loadChildren(dirPath)
    }
    setEditing({ mode: kind === 'file' ? 'create-file' : 'create-dir', dirPath, value: '' })
  }

  // 进入重命名时预选中不含扩展名的部分（VS Code 行为）；只在编辑会话开启时跑一次。
  useEffect(() => {
    if (editing?.mode !== 'rename') return
    const input = editInputRef.current
    if (!input) return
    const dot = input.value.lastIndexOf('.')
    input.setSelectionRange(0, dot > 0 ? dot : input.value.length)
  }, [editing?.mode, editing?.path])

  const confirmEditing = async () => {
    const current = editingRef.current
    if (!current || submittingRef.current) return
    const name = current.value.trim()
    if (!name) {
      setEditing(null)
      return
    }
    if (name.includes('/') || name.includes('\\')) {
      setEditing({ ...current, error: t.dockNameInvalid })
      return
    }
    submittingRef.current = true
    try {
      if (current.mode === 'rename' && current.path) {
        const target = current.path
        const isDir = tree.nodes[target]?.kind === 'dir'
        const toPath = await tree.renameEntry(target, name)
        if (isDir) emitExpanded(remapExpandedForRename(expandedSet, target, toPath))
        setSelected(new Set([toPath]))
        selectionAnchorRef.current = toPath
        pendingRevealRef.current = toPath
      } else {
        const kind = current.mode === 'create-file' ? 'file' : 'dir'
        const newPath = await tree.createEntry(current.dirPath, name, kind)
        // 建完选中并滚动到新条目（复用 reveal 定位：树刷新出该行时自动定位）。
        setSelected(new Set([newPath]))
        selectionAnchorRef.current = newPath
        pendingRevealRef.current = newPath
      }
      setEditing(null)
    } catch (err) {
      // 函数式更新：只给「还开着的」编辑器挂错误，不把已关闭的输入行复活。
      const message = err instanceof Error ? err.message : String(err)
      setEditing((value) => (value ? { ...value, error: message } : value))
    } finally {
      submittingRef.current = false
    }
  }

  const cancelOrConfirmOnBlur = () => {
    // 失焦：有内容按确认处理（VS Code 行为），空内容取消。提交中/已关闭则不动——
    // Enter 或 Escape 都会紧跟一次 blur。
    if (submittingRef.current) return
    const current = editingRef.current
    if (!current) return
    if (current.value.trim()) void confirmEditing()
    else setEditing(null)
  }

  const handleDeleteConfirm = async () => {
    const targets = deleteTarget
    if (!targets?.length) return
    setDeleting(true)
    try {
      for (const target of targets) {
        await tree.deleteEntry(target.path)
        if (target.kind === 'dir') {
          // 删掉展开的目录时同步清理展开集合里的该子树。
          const prefix = `${target.path}/`
          emitExpanded(
            new Set([...expandedSet].filter((path) => path !== target.path && !path.startsWith(prefix))),
          )
        }
      }
      const removed = new Set(targets.map((target) => target.path))
      setSelected((prev) => new Set([...prev].filter((path) => !removed.has(path))))
      setDeleteTarget(null)
    } catch {
      // 删除失败保持对话框打开，用户可取消；错误由下一次刷新兜底呈现。
    } finally {
      setDeleting(false)
    }
  }

  // ---------- 框选 / 拖拽 ----------
  // 拖拽用鼠标事件自绘（ghost + elementFromPoint 命中），不用 HTML5 DnD——
  // Tauri 窗口开着原生 dragDrop 拦截（InputBar/知识库靠它接收外部文件拖入），
  // WebView 内的 HTML5 拖拽在 Windows 上会整个失效。

  /** 拖拽悬停目标的 ref 镜像：mouseup 闭包里读 state 是旧值。 */
  const dropTargetRef = useRef<string | null>(null)
  const setDrop = (value: string | null) => {
    dropTargetRef.current = value
    setDropTarget(value)
  }
  const dragStateRef = useRef<{ paths: string[]; startX: number; startY: number; active: boolean } | null>(null)
  const [dragGhost, setDragGhost] = useState<{ x: number; y: number; label: string } | null>(null)
  /** 刚完成一次拖拽时吞掉紧随的 click（否则松手会触发选中/打开查看器）。 */
  const suppressClickRef = useRef(false)

  /** 内容坐标 y 区间 → 命中的 node 行路径（行高固定 ROW_HEIGHT）。
   *  ponytail: 编辑行报错时高度会撑开、索引换算会偏一行；框选与内联编辑并发场景可忽略。 */
  const nodePathsInContentRange = (y1: number, y2: number): string[] => {
    const lo = Math.min(y1, y2)
    const hi = Math.max(y1, y2)
    const paths: string[] = []
    for (let i = Math.max(0, Math.floor(lo / ROW_HEIGHT)); i <= Math.floor(hi / ROW_HEIGHT) && i < rows.length; i += 1) {
      const row = rows[i]
      if (row.kind === 'node') paths.push(row.path)
    }
    return paths
  }

  const startMarquee = (e: React.MouseEvent) => {
    if (e.button !== 0) return
    if ((e.target as HTMLElement).closest('[role="button"],input')) return
    const area = treeAreaRef.current
    if (!area) return
    const areaRect = area.getBoundingClientRect()
    const startX = e.clientX - areaRect.left
    const startContentY = e.clientY - areaRect.top + (vlistRef.current?.scrollOffset ?? 0)
    // 点空白即清空选择；拖起来才画框。
    setSelected(new Set())
    selectionAnchorRef.current = null
    const onMove = (ev: MouseEvent) => {
      const rect = area.getBoundingClientRect()
      const scroll = vlistRef.current?.scrollOffset ?? 0
      const x = Math.min(Math.max(ev.clientX - rect.left, 0), rect.width)
      const y = Math.min(Math.max(ev.clientY - rect.top, 0), rect.height)
      const contentY = y + scroll
      setMarqueeRect({
        left: Math.min(startX, x),
        top: Math.min(startContentY - scroll, y),
        width: Math.abs(x - startX),
        height: Math.abs(y - (startContentY - scroll)),
      })
      setSelected(new Set(nodePathsInContentRange(startContentY, contentY)))
    }
    const onUp = () => {
      window.removeEventListener('mousemove', onMove)
      setMarqueeRect(null)
    }
    window.addEventListener('mousemove', onMove)
    window.addEventListener('mouseup', onUp, { once: true })
  }

  /** 行点击：Ctrl 切换 / Shift 范围（按可见行序）/ 普通单选（目录展开、文件开查看器）。 */
  const handleRowSelect = (
    e: { ctrlKey: boolean; metaKey: boolean; shiftKey: boolean },
    path: string,
    node: FileTreeNode,
  ) => {
    if (e.ctrlKey || e.metaKey) {
      setSelected((prev) => {
        const next = new Set(prev)
        if (next.has(path)) next.delete(path)
        else next.add(path)
        return next
      })
      selectionAnchorRef.current = path
      return
    }
    if (e.shiftKey && selectionAnchorRef.current) {
      const order = rows.filter((row) => row.kind === 'node').map((row) => (row as { path: string }).path)
      const a = order.indexOf(selectionAnchorRef.current)
      const b = order.indexOf(path)
      if (a >= 0 && b >= 0) {
        setSelected(new Set(order.slice(Math.min(a, b), Math.max(a, b) + 1)))
        return
      }
    }
    setSelected(new Set([path]))
    selectionAnchorRef.current = path
    if (node.kind === 'dir') handleToggleDir(node)
    else setViewer({ kind: 'file', workdir, path })
  }

  const handleDropPaths = async (targetDir: string, paths: string[]) => {
    const moved: string[] = []
    for (const src of paths) {
      // 守卫：移动到原目录 / 自身 / 自身后代都跳过。
      if (!src || src === targetDir || parentDirOf(src) === targetDir) continue
      if (targetDir !== ROOT_PATH && targetDir.startsWith(`${src}/`)) continue
      try {
        moved.push(await tree.moveEntry(src, targetDir))
      } catch {
        // 单项失败（如目标重名）跳过，其余继续；树刷新呈现实际结果。
      }
    }
    if (moved.length) {
      setSelected(new Set(moved))
      selectionAnchorRef.current = moved[moved.length - 1]
    }
  }

  /** 行上按下左键：超过 6px 位移进入拖拽（ghost 跟随、elementFromPoint 命中投放目录），
   *  松手落到目标目录；没动过就交还给 click（选中/展开/打开查看器）。 */
  const startRowDrag = (e: React.MouseEvent, path: string) => {
    if (e.button !== 0) return
    const paths = selected.has(path) ? [...selected] : [path]
    dragStateRef.current = { paths, startX: e.clientX, startY: e.clientY, active: false }
    const onMove = (ev: MouseEvent) => {
      const st = dragStateRef.current
      if (!st) return
      if (!st.active) {
        if (Math.abs(ev.clientX - st.startX) + Math.abs(ev.clientY - st.startY) < 6) return
        st.active = true
        if (!selected.has(path)) {
          setSelected(new Set([path]))
          selectionAnchorRef.current = path
        }
      }
      setDragGhost({
        x: ev.clientX,
        y: ev.clientY,
        label: st.paths.length > 1 ? `${st.paths.length} 项` : basenameOf(st.paths[0]),
      })
      const el = document.elementFromPoint(ev.clientX, ev.clientY) as HTMLElement | null
      const rowEl = el?.closest('[data-drop-dir]') as HTMLElement | null
      if (rowEl) setDrop(rowEl.dataset.dropDir ?? null)
      else if (el && treeAreaRef.current?.contains(el)) setDrop(ROOT_PATH)
      else setDrop(null)
    }
    const onUp = () => {
      window.removeEventListener('mousemove', onMove)
      const st = dragStateRef.current
      dragStateRef.current = null
      setDragGhost(null)
      const target = dropTargetRef.current
      setDrop(null)
      if (st?.active) {
        suppressClickRef.current = true
        window.setTimeout(() => {
          suppressClickRef.current = false
        }, 0)
        if (target !== null) void handleDropPaths(target, st.paths)
      }
    }
    window.addEventListener('mousemove', onMove)
    window.addEventListener('mouseup', onUp, { once: true })
  }

  const menuItems = (path: string | null): DockMenuItem[] => {
    const node = path !== null ? tree.nodes[path] : null
    const dirPath = node ? (node.kind === 'dir' ? node.path : parentDirOf(node.path)) : ROOT_PATH
    const items: DockMenuItem[] = []
    if (node) {
      items.push({
        key: 'open',
        label: t.dockOpen,
        icon: <ExternalLink strokeWidth={1.75} />,
        onSelect: () => handleOpenEntry(node.path, 'open'),
      })
      items.push({
        key: 'reveal',
        label: t.dockRevealInFinder,
        icon: <FolderOpen strokeWidth={1.75} />,
        onSelect: () => handleOpenEntry(node.path, 'reveal'),
      })
    }
    items.push({
      key: 'new-file',
      label: t.dockNewFile,
      icon: <FilePlus2 strokeWidth={1.75} />,
      onSelect: () => startCreate(dirPath, 'file'),
    })
    items.push({
      key: 'new-folder',
      label: t.dockNewFolder,
      icon: <FolderPlus strokeWidth={1.75} />,
      onSelect: () => startCreate(dirPath, 'dir'),
    })
    if (node) {
      items.push({
        key: 'rename',
        label: t.dockRename,
        icon: <Pencil strokeWidth={1.75} />,
        onSelect: () =>
          setEditing({ mode: 'rename', dirPath: parentDirOf(node.path), path: node.path, value: node.name }),
      })
      items.push({
        key: 'copy-path',
        label: t.dockCopyPath,
        icon: <Copy strokeWidth={1.75} />,
        onSelect: () => void navigator.clipboard?.writeText(node.path).catch(() => {}),
      })
      if (onInsertMention) {
        items.push({
          key: 'mention',
          label: t.dockInsertMention,
          icon: <AtSign strokeWidth={1.75} />,
          onSelect: () => onInsertMention(node.path),
        })
      }
      items.push({
        key: 'delete',
        label:
          selected.has(node.path) && selected.size > 1
            ? t.dockDeleteMany.replace('{n}', String(selected.size))
            : t.dockDelete,
        icon: <Trash2 strokeWidth={1.75} />,
        danger: true,
        onSelect: () => {
          const nodes =
            selected.has(node.path) && selected.size > 1
              ? [...selected].map((p) => tree.nodes[p]).filter((n): n is FileTreeNode => Boolean(n))
              : [node]
          setDeleteTarget(nodes)
        },
      })
    }
    if (!node) {
      items.push({
        key: 'refresh',
        label: t.dockRefresh,
        icon: <RefreshCw strokeWidth={1.75} />,
        onSelect: () => tree.refreshVisible(),
      })
    }
    return items
  }

  // ---------- 行渲染 ----------

  const renderNodeRow = (path: string, depth: number) => {
    const node = tree.nodes[path]
    if (!node) return null
    const isDir = node.kind === 'dir'
    const expanded = expandedSet.has(path)
    const DirIcon = expanded ? FolderOpen : Folder
    const fileVisual = isDir ? null : fileVisualFor(node.name)
    // 拖到文件行 = 拖进它所在的目录（Finder 行为）。
    const rowDropDir = isDir ? path : parentDirOf(path)
    return (
      <div
        role="button"
        tabIndex={0}
        data-drop-dir={rowDropDir}
        className={`relative mx-1 flex h-full cursor-default items-center gap-1 rounded-md pr-2 ${
          dropTarget !== null && dropTarget === rowDropDir && isDir
            ? 'bg-[var(--accent-soft)]'
            : selected.has(path)
              ? 'bg-neutral-500/12 dark:bg-neutral-400/12'
              : 'hover:bg-neutral-500/8 dark:hover:bg-neutral-400/8'
        } ${node.hidden ? 'opacity-55' : ''}`}
        style={{ paddingLeft: 2 + depth * 14 }}
        onMouseDown={(e) => startRowDrag(e, path)}
        onClick={(e) => {
          if (suppressClickRef.current) return
          handleRowSelect(e, path, node)
        }}
        onDoubleClick={() => {
          if (!isDir) handleOpenEntry(path, 'open')
        }}
        onKeyDown={(e) => {
          if (e.key === 'Enter') handleRowSelect(e, path, node)
        }}
        onContextMenu={(e) => {
          e.preventDefault()
          if (!selected.has(path)) {
            setSelected(new Set([path]))
            selectionAnchorRef.current = path
          }
          setMenu({ anchor: { left: e.clientX, top: e.clientY }, path })
        }}
      >
        {/* 缩进参考线：每层祖先一条竖线（对齐该层 chevron 中心），行行相接连成整线。 */}
        {Array.from({ length: depth }, (_, level) => (
          <span
            key={level}
            aria-hidden="true"
            className="pointer-events-none absolute bottom-0 top-0 w-px bg-black/[0.08] dark:bg-white/[0.1]"
            style={{ left: 2 + level * 14 + 6 }}
          />
        ))}
        {isDir ? (
          node.loading ? (
            <Loader2 size={12} className="shrink-0 animate-spin text-neutral-400" />
          ) : expanded ? (
            <ChevronDown size={12} strokeWidth={2} className="shrink-0 text-neutral-400" />
          ) : (
            <ChevronRight size={12} strokeWidth={2} className="shrink-0 text-neutral-400" />
          )
        ) : (
          <span className="w-3 shrink-0" />
        )}
        {isDir ? (
          <DirIcon
            size={13}
            strokeWidth={1.75}
            className="shrink-0 text-amber-500/80 dark:text-amber-300/70"
          />
        ) : fileVisual ? (
          <fileVisual.Icon
            size={13}
            strokeWidth={1.75}
            className={`shrink-0 ${fileVisual.className}`}
          />
        ) : null}
        <span className="min-w-0 flex-1 truncate text-[12.5px] text-neutral-700 dark:text-neutral-200">
          {node.name}
        </span>
      </div>
    )
  }

  const renderEditRow = (depth: number) => {
    if (!editing) return null
    const Icon = editing.mode === 'create-dir' ? Folder : editing.mode === 'rename' ? Pencil : File
    return (
      <div className="flex flex-col pr-2" style={{ paddingLeft: 6 + depth * 14 }}>
        <div className="flex h-[28px] items-center gap-1">
          <span className="w-3 shrink-0" />
          <Icon size={13} strokeWidth={1.75} className="shrink-0 text-neutral-400" />
          <input
            ref={editInputRef}
            autoFocus
            value={editing.value}
            placeholder={t.dockNamePlaceholder}
            className={`min-w-0 flex-1 rounded border bg-transparent px-1 py-0.5 text-[12px] outline-none ${
              editing.error
                ? 'border-red-400 dark:border-red-500'
                : 'border-neutral-300 focus:border-neutral-500 dark:border-neutral-600 dark:focus:border-neutral-400'
            }`}
            onChange={(e) => setEditing({ ...editing, value: e.target.value, error: undefined })}
            onKeyDown={(e) => {
              if (e.nativeEvent.isComposing) return
              if (e.key === 'Enter') {
                e.preventDefault()
                void confirmEditing()
              } else if (e.key === 'Escape') {
                e.preventDefault()
                setEditing(null)
              }
            }}
            onBlur={cancelOrConfirmOnBlur}
          />
        </div>
        {editing.error && (
          <div className="pl-[34px] pb-1 text-[11px] leading-4 text-red-500 dark:text-red-400">
            {editing.error}
          </div>
        )}
      </div>
    )
  }

  const searching = tree.searchResults !== null

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
      {/* 工具栏 */}
      <div className="flex items-center gap-1 border-b border-neutral-200/70 px-2 py-1.5 dark:border-neutral-700/50">
        <div className="flex min-w-0 flex-1 items-center gap-1 rounded-md bg-neutral-500/10 px-1.5">
          <Search size={12} strokeWidth={2} className="shrink-0 text-neutral-400" />
          <input
            value={tree.searchQuery}
            onChange={(e) => tree.setSearchQuery(e.target.value)}
            placeholder={t.dockSearchPlaceholder}
            className="h-6 min-w-0 flex-1 bg-transparent text-[12px] outline-none placeholder:text-neutral-400"
            onKeyDown={(e) => {
              if (e.key === 'Escape' && tree.searchQuery) tree.setSearchQuery('')
            }}
          />
          {tree.searchQuery && (
            <button
              type="button"
              className="shrink-0 rounded p-0.5 text-neutral-400 hover:text-neutral-600 dark:hover:text-neutral-300"
              onClick={() => tree.setSearchQuery('')}
            >
              <X size={11} />
            </button>
          )}
        </div>
        <IconButton
          label={t.dockRefresh}
          size="sm"
          variant="ghost"
          onClick={() => tree.refreshVisible()}
        >
          <RefreshCw size={13} />
        </IconButton>
        <IconButton label={t.dockNewFile} size="sm" variant="ghost" onClick={() => startCreate(ROOT_PATH, 'file')}>
          <FilePlus2 size={13} />
        </IconButton>
        <IconButton label={t.dockNewFolder} size="sm" variant="ghost" onClick={() => startCreate(ROOT_PATH, 'dir')}>
          <FolderPlus size={13} />
        </IconButton>
        <IconButton
          label={t.dockToggleHidden}
          size="sm"
          variant="ghost"
          className={showHidden ? 'text-neutral-800 dark:text-neutral-100' : ''}
          onClick={() => setShowHidden((prev) => !prev)}
        >
          {showHidden ? <Eye size={13} /> : <EyeOff size={13} />}
        </IconButton>
      </div>

      {/* 树 / 搜索结果 */}
      <div
        ref={treeAreaRef}
        className="relative min-h-0 flex-1 select-none"
        onMouseDown={(e) => {
          if (!searching) startMarquee(e)
        }}
        onContextMenu={(e) => {
          // 空白区域右键 → 根目录菜单（行内右键已 stopPropagation 不到这里——
          // 行自身的 onContextMenu preventDefault 但未 stopPropagation，这里用 target 判断）。
          if ((e.target as HTMLElement).closest('[role="button"]')) return
          e.preventDefault()
          setMenu({ anchor: { left: e.clientX, top: e.clientY }, path: null })
        }}
      >
        {searching ? (
          <VList className="custom-scrollbar h-full">
            {tree.searching && (
              <div className="flex items-center justify-center gap-2 py-3 text-[11px] text-neutral-400">
                <Loader2 size={12} className="animate-spin" />
                {t.dockLoading}
              </div>
            )}
            {!tree.searching && (tree.searchResults ?? []).length === 0 && (
              <div className="py-6 text-center text-[12px] text-neutral-400">{t.dockSearchEmpty}</div>
            )}
            {(tree.searchResults ?? []).map((entry: DockFsEntry) => {
              const visual =
                entry.kind === 'dir'
                  ? { Icon: Folder, className: 'text-amber-500/80 dark:text-amber-300/70' }
                  : fileVisualFor(basenameOf(entry.path))
              const dir = parentDirOf(entry.path)
              return (
                <div
                  key={entry.path}
                  role="button"
                  tabIndex={0}
                  className={`mx-1 flex h-[28px] cursor-default items-center gap-1.5 rounded-md px-1.5 ${
                    selected.has(entry.path)
                      ? 'bg-neutral-500/12 dark:bg-neutral-400/12'
                      : 'hover:bg-neutral-500/8 dark:hover:bg-neutral-400/8'
                  } ${entry.hidden ? 'opacity-55' : ''}`}
                  onClick={() => {
                    setSelected(new Set([entry.path]))
                    selectionAnchorRef.current = entry.path
                    if (entry.kind !== 'dir') setViewer({ kind: 'file', workdir, path: entry.path })
                  }}
                  onDoubleClick={() => handleOpenEntry(entry.path, 'open')}
                  onKeyDown={(e) => {
                    if (e.key === 'Enter') {
                      setSelected(new Set([entry.path]))
                      selectionAnchorRef.current = entry.path
                      if (entry.kind !== 'dir') setViewer({ kind: 'file', workdir, path: entry.path })
                    }
                  }}
                  onContextMenu={(e) => {
                    e.preventDefault()
                    setSelected(new Set([entry.path]))
                    selectionAnchorRef.current = entry.path
                    setMenu({ anchor: { left: e.clientX, top: e.clientY }, path: entry.path })
                  }}
                >
                  <visual.Icon size={13} strokeWidth={1.75} className={`shrink-0 ${visual.className}`} />
                  <span className="min-w-0 flex-1 truncate text-[12.5px] text-neutral-700 dark:text-neutral-200">
                    {basenameOf(entry.path)}
                    {dir && (
                      <span className="ml-1.5 text-[11px] text-neutral-400 dark:text-neutral-500">{dir}</span>
                    )}
                  </span>
                </div>
              )
            })}
          </VList>
        ) : rows.length === 0 ? (
          <div className="flex h-full items-center justify-center px-4 text-center text-[12px] text-neutral-400">
            {tree.nodes[ROOT_PATH]?.loading ? t.dockLoading : t.dockEmptyTree}
          </div>
        ) : (
          <VList ref={vlistRef} className="custom-scrollbar h-full">
            {rows.map((row, index) => (
              <div
                key={row.kind === 'edit' ? `edit-${index}` : `${row.kind}-${row.path}`}
                style={{ height: row.kind === 'edit' && editing?.error ? undefined : ROW_HEIGHT }}
              >
                {row.kind === 'node' ? (
                  renderNodeRow(row.path, row.depth)
                ) : row.kind === 'error' ? (
                  <button
                    type="button"
                    className="flex h-full w-full items-center gap-1.5 pr-2 text-left text-[11px] text-red-500/90 hover:bg-red-500/5 dark:text-red-300/90"
                    style={{ paddingLeft: 6 + row.depth * 14 + 16 }}
                    onClick={() => void tree.loadChildren(row.path, { force: true })}
                  >
                    {t.dockRetry}
                  </button>
                ) : (
                  renderEditRow(row.depth)
                )}
              </div>
            ))}
          </VList>
        )}
        {marqueeRect && (
          <div
            className="pointer-events-none absolute z-10 rounded-sm border border-[var(--accent)] bg-[var(--accent-soft)] opacity-50"
            style={marqueeRect}
          />
        )}
      </div>

      {dragGhost && (
        <div
          className="pointer-events-none fixed z-50 rounded-md border border-neutral-300 bg-[var(--theme-surface-soft)] px-2 py-0.5 text-[11px] text-neutral-700 shadow-sm dark:border-neutral-600 dark:bg-[#262629] dark:text-neutral-200"
          style={{ left: dragGhost.x + 10, top: dragGhost.y + 12 }}
        >
          {dragGhost.label}
        </div>
      )}

      {menu && (
        <DockContextMenu anchor={menu.anchor} items={menuItems(menu.path)} onClose={() => setMenu(null)} />
      )}
      {viewer?.kind === 'file' && (
        <FileViewer
          workdir={viewer.workdir}
          path={viewer.path}
          lang={lang}
          onClose={() => setViewer(null)}
        />
      )}
      {viewer?.kind === 'diff' && (
        <div className="absolute inset-0 z-10 flex flex-col bg-[var(--theme-surface-soft)] dark:bg-[#262629]">
          <div className="flex shrink-0 items-center gap-1.5 border-b border-neutral-200/70 px-2 py-1.5 dark:border-neutral-700/50">
            <span className="min-w-0 flex-1 truncate text-[12.5px] font-medium text-neutral-800 dark:text-neutral-100">
              {viewer.title}
            </span>
            <IconButton label={t.dockViewerClose} size="sm" variant="ghost" onClick={() => setViewer(null)}>
              <X size={13} />
            </IconButton>
          </div>
          <div className="custom-scrollbar min-h-0 flex-1 overflow-auto p-2">
            <DiffView patch={viewer.patch} lang={lang} />
          </div>
        </div>
      )}
      {deleteTarget && (
        <ConfirmDialog
          lang={lang}
          title={
            deleteTarget.length > 1
              ? t.dockDeleteMany.replace('{n}', String(deleteTarget.length))
              : t.dockDeleteTitle
          }
          message={
            deleteTarget.length > 1
              ? `${deleteTarget.slice(0, 5).map((node) => node.path).join('、')}${deleteTarget.length > 5 ? '…' : ''}`
              : deleteTarget[0].path
          }
          confirmLabel={t.dockDeleteConfirm}
          busy={deleting}
          onConfirm={() => void handleDeleteConfirm()}
          onCancel={() => setDeleteTarget(null)}
        />
      )}
    </div>
  )
}
