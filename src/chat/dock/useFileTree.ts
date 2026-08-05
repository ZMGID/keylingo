// 文件树数据 hook：懒加载 + 搜索 + 增删改 + workspace-activity 失效。
// 请求去重用 ref 内同步记录（同一 render 周期内的重复调用也会被挡），
// 每个路径一个 epoch 计数丢弃乱序/过期响应（切 workdir 后旧响应不会污染新树）。
import { useCallback, useEffect, useRef, useState } from 'react'
import { dockApi } from './api'
import {
  applyListResponse,
  createRootNode,
  joinPath,
  markNodeLoading,
  parentDirOf,
  ROOT_PATH,
  type FileTreeNodes,
} from './fileTreeModel'
import type { DockFsEntry, DockFsEntryKind } from './types'
import { workspaceActivity } from './workspaceActivity'

const SEARCH_DEBOUNCE_MS = 180
const FALLBACK_POLL_MS = 10_000
/** 单事件变更路径达到该数量即放弃定向刷新，整树可见部分重载。 */
const TARGETED_REFRESH_LIMIT = 64

type LoadOptions = {
  force?: boolean
  silent?: boolean
}

export type UseFileTreeOptions = {
  workdir: string
  active: boolean
  showHidden: boolean
  /** 受控展开集合（持久化在 Chat.tsx），失效刷新只碰已展开且已加载的目录。 */
  expandedPaths: ReadonlySet<string>
}

export function useFileTree({ workdir, active, showHidden, expandedPaths }: UseFileTreeOptions) {
  const [nodes, setNodes] = useState<FileTreeNodes>(() => ({ [ROOT_PATH]: createRootNode() }))
  const [searchQuery, setSearchQuery] = useState('')
  const [searchResults, setSearchResults] = useState<DockFsEntry[] | null>(null)
  const [searchTruncated, setSearchTruncated] = useState(false)
  const [searching, setSearching] = useState(false)

  const nodesRef = useRef(nodes)
  nodesRef.current = nodes
  const workdirRef = useRef(workdir)
  workdirRef.current = workdir
  const activeRef = useRef(active)
  activeRef.current = active
  const showHiddenRef = useRef(showHidden)
  showHiddenRef.current = showHidden
  const expandedRef = useRef(expandedPaths)
  expandedRef.current = expandedPaths
  const inFlightRef = useRef<Set<string>>(new Set())
  const epochRef = useRef<Map<string, number>>(new Map())
  const dirtyRef = useRef(false)

  const bumpEpoch = useCallback((path: string): number => {
    const next = (epochRef.current.get(path) ?? 0) + 1
    epochRef.current.set(path, next)
    return next
  }, [])

  const loadChildren = useCallback(
    async (path: string, options: LoadOptions = {}) => {
      const { force = false, silent = false } = options
      const currentWorkdir = workdirRef.current
      if (!currentWorkdir) return
      const node = nodesRef.current[path]
      if (!node) return
      // 同步去重：同一路径已有请求在飞就不再发。
      if (inFlightRef.current.has(path)) return
      if (!force && node.loaded && !node.error) return

      inFlightRef.current.add(path)
      const epoch = bumpEpoch(path)
      if (!silent) {
        setNodes((prev) => markNodeLoading(prev, path, true))
      }
      try {
        const result = await dockApi.fsList(currentWorkdir, path, 1000, showHiddenRef.current)
        // 过期响应（期间又发起过同路径请求或已切 workdir）直接丢弃。
        if (epochRef.current.get(path) !== epoch || workdirRef.current !== currentWorkdir) return
        setNodes((prev) => applyListResponse(prev, path, result.entries))
      } catch (err) {
        if (epochRef.current.get(path) !== epoch || workdirRef.current !== currentWorkdir) return
        const message = err instanceof Error ? err.message : String(err)
        setNodes((prev) => markNodeLoading(prev, path, false, message))
      } finally {
        inFlightRef.current.delete(path)
      }
    },
    [bumpEpoch],
  )

  /** 根目录 + 所有已展开且已加载的目录，静默强制重载。 */
  const refreshVisible = useCallback(() => {
    const currentNodes = nodesRef.current
    const targets: string[] = [ROOT_PATH]
    for (const path of expandedRef.current) {
      const node = currentNodes[path]
      if (node && node.kind === 'dir' && node.loaded) targets.push(path)
    }
    for (const path of targets) {
      void loadChildren(path, { force: true, silent: true })
    }
  }, [loadChildren])

  // workdir 变化：整树重置，重新加载根。
  useEffect(() => {
    epochRef.current.clear()
    inFlightRef.current.clear()
    setNodes({ [ROOT_PATH]: createRootNode() })
    setSearchResults(null)
    setSearchQuery('')
    dirtyRef.current = false
    if (workdir && active) {
      // nodesRef 此时还指向旧树（旧根已 loaded），必须强制加载。
      void loadChildren(ROOT_PATH, { force: true })
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [workdir])

  // active 由 false → true：冲刷非激活期攒下的脏标记；首次激活时补加载根。
  useEffect(() => {
    if (!active || !workdir) return
    const root = nodesRef.current[ROOT_PATH]
    if (root && !root.loaded && !root.loading) {
      void loadChildren(ROOT_PATH)
    }
    if (dirtyRef.current) {
      dirtyRef.current = false
      refreshVisible()
    }
  }, [active, workdir, loadChildren, refreshVisible])

  // workspace-activity 失效：changedPaths 映射到父目录定向刷新；git-only 事件忽略；
  // 事件过多或截断 → 可见部分整体刷新。非激活期只攒脏标记。
  useEffect(() => {
    if (!workdir) return
    return workspaceActivity.subscribe(workdir, (activity) => {
      if (!activity.fs && activity.git) return
      if (!activeRef.current) {
        dirtyRef.current = true
        return
      }
      if (activity.truncated || activity.changedPaths.length >= TARGETED_REFRESH_LIMIT) {
        refreshVisible()
        return
      }
      const dirs = new Set<string>()
      const currentNodes = nodesRef.current
      for (const changed of activity.changedPaths) {
        const parent = parentDirOf(changed)
        dirs.add(parent)
        // 变更路径本身是已加载目录（被整个删掉/改名）时也刷它自身。
        if (currentNodes[changed]?.kind === 'dir') dirs.add(changed)
      }
      for (const dir of dirs) {
        const node = currentNodes[dir]
        if (!node || !node.loaded) continue
        if (dir !== ROOT_PATH && !expandedRef.current.has(dir)) continue
        void loadChildren(dir, { force: true, silent: true })
      }
    })
  }, [workdir, loadChildren, refreshVisible])

  // watcher 不可用（浏览器预览）时的 10s 兜底轮询。
  useEffect(() => {
    if (!active || !workdir || workspaceActivity.isAvailable()) return
    const timer = window.setInterval(() => refreshVisible(), FALLBACK_POLL_MS)
    return () => window.clearInterval(timer)
  }, [active, workdir, refreshVisible])

  // 搜索（180ms 去抖）。空串退出搜索态。
  useEffect(() => {
    const query = searchQuery.trim()
    if (!query || !workdir) {
      setSearchResults(null)
      setSearchTruncated(false)
      setSearching(false)
      return
    }
    setSearching(true)
    const timer = window.setTimeout(() => {
      const currentWorkdir = workdirRef.current
      dockApi
        .fsSearch(currentWorkdir, query, 200, showHiddenRef.current)
        .then((result) => {
          if (workdirRef.current !== currentWorkdir) return
          setSearchResults(result.entries)
          setSearchTruncated(result.truncated)
          setSearching(false)
        })
        .catch(() => {
          if (workdirRef.current !== currentWorkdir) return
          setSearchResults([])
          setSearchTruncated(false)
          setSearching(false)
        })
    }, SEARCH_DEBOUNCE_MS)
    return () => window.clearTimeout(timer)
  }, [searchQuery, workdir])

  // showHidden 切换：整树重载（隐藏项参与排序/合并，无法局部补救）。
  const prevShowHiddenRef = useRef(showHidden)
  useEffect(() => {
    if (prevShowHiddenRef.current === showHidden) return
    prevShowHiddenRef.current = showHidden
    if (!activeRef.current) {
      dirtyRef.current = true
      return
    }
    epochRef.current.clear()
    inFlightRef.current.clear()
    setNodes({ [ROOT_PATH]: createRootNode() })
    void loadChildren(ROOT_PATH, { force: true })
  }, [showHidden, loadChildren])

  const createEntry = useCallback(
    async (parentPath: string, name: string, kind: DockFsEntryKind): Promise<string> => {
      const path = joinPath(parentPath, name)
      await dockApi.fsCreate(workdirRef.current, path, kind)
      await loadChildren(parentPath, { force: true, silent: true })
      return path
    },
    [loadChildren],
  )

  const renameEntry = useCallback(
    async (path: string, newName: string): Promise<string> => {
      const parent = parentDirOf(path)
      const toPath = joinPath(parent, newName)
      await dockApi.fsRename(workdirRef.current, path, toPath)
      await loadChildren(parent, { force: true, silent: true })
      return toPath
    },
    [loadChildren],
  )

  const deleteEntry = useCallback(
    async (path: string): Promise<void> => {
      await dockApi.fsDelete(workdirRef.current, path)
      await loadChildren(parentDirOf(path), { force: true, silent: true })
    },
    [loadChildren],
  )

  /** 拖拽移动：移入 toDir（ROOT_PATH = 根）。源/目标两个父目录都要刷新。 */
  const moveEntry = useCallback(
    async (path: string, toDir: string): Promise<string> => {
      const result = await dockApi.fsMove(workdirRef.current, path, toDir === ROOT_PATH ? '' : toDir)
      await Promise.all([
        loadChildren(parentDirOf(path), { force: true, silent: true }),
        loadChildren(toDir, { force: true, silent: true }),
      ])
      return result.path
    },
    [loadChildren],
  )

  const openEntry = useCallback(async (path: string, mode: 'open' | 'reveal') => {
    await dockApi.fsOpenPath(workdirRef.current, path, mode)
  }, [])

  return {
    nodes,
    loadChildren,
    refreshVisible,
    searchQuery,
    setSearchQuery,
    searchResults,
    searchTruncated,
    searching,
    createEntry,
    renameEntry,
    deleteEntry,
    moveEntry,
    openEntry,
  }
}
