// 文件树纯数据模型：节点表 + 排序 + list 响应合并 + 扁平化 + 展开集合操作。
// 无 React 依赖，全部纯函数，直接可单测。
import type { DockFsEntry } from './types'

export const ROOT_PATH = ''

export type FileTreeNode = {
  /** 相对 workdir 路径；根节点为 ROOT_PATH('')。 */
  path: string
  name: string
  kind: 'file' | 'dir'
  hidden: boolean
  /** 子节点路径列表（已排序）。 */
  children: string[]
  /** 子节点至少成功加载过一次。 */
  loaded: boolean
  loading: boolean
  error?: string | null
}

export type FileTreeNodes = Record<string, FileTreeNode>

export type FileTreeRow = {
  type: 'node' | 'error'
  path: string
  depth: number
}

export function basenameOf(path: string): string {
  const idx = path.lastIndexOf('/')
  return idx < 0 ? path : path.slice(idx + 1)
}

export function parentDirOf(path: string): string {
  const idx = path.lastIndexOf('/')
  return idx < 0 ? ROOT_PATH : path.slice(0, idx)
}

export function joinPath(parent: string, name: string): string {
  return parent ? `${parent}/${name}` : name
}

/** path 的所有祖先目录（不含自身），从浅到深。reveal 定位用。 */
export function ancestorsOf(path: string): string[] {
  const result: string[] = []
  let current = parentDirOf(path)
  while (current) {
    result.unshift(current)
    current = parentDirOf(current)
  }
  return result
}

/** 目录优先，同级按小写 basename localeCompare。 */
export function sortEntries(entries: DockFsEntry[]): DockFsEntry[] {
  return [...entries].sort((a, b) => {
    if (a.kind !== b.kind) return a.kind === 'dir' ? -1 : 1
    return basenameOf(a.path).toLowerCase().localeCompare(basenameOf(b.path).toLowerCase())
  })
}

export function createRootNode(): FileTreeNode {
  return {
    path: ROOT_PATH,
    name: '',
    kind: 'dir',
    hidden: false,
    children: [],
    loaded: false,
    loading: false,
    error: null,
  }
}

function createChildNode(entry: DockFsEntry): FileTreeNode {
  return {
    path: entry.path,
    name: basenameOf(entry.path),
    kind: entry.kind,
    hidden: entry.hidden,
    children: [],
    loaded: false,
    loading: false,
    error: null,
  }
}

/** 递归收集 path 及其全部后代路径。 */
function collectSubtreePaths(nodes: FileTreeNodes, path: string, into: string[]): void {
  into.push(path)
  const node = nodes[path]
  if (!node) return
  for (const child of node.children) collectSubtreePaths(nodes, child, into)
}

function sameStringArray(a: string[], b: string[]): boolean {
  return a.length === b.length && a.every((value, index) => value === b[index])
}

/**
 * 把一层目录的 list 响应合并进节点表。
 * - 子节点已存在且 kind 相同 → 保留原节点对象（children/loaded 等状态不丢）。
 * - 响应里消失的子树整体剪除。
 * - 内容无变化 → 返回原 nodes 引用（配合 React 引用比较跳过重渲）。
 */
export function applyListResponse(
  nodes: FileTreeNodes,
  path: string,
  entries: DockFsEntry[],
): FileTreeNodes {
  const parent = nodes[path]
  if (!parent) return nodes

  const sorted = sortEntries(entries)
  const nextChildren: string[] = []
  const nextNodes: FileTreeNodes = { ...nodes }
  let childrenChanged = false

  for (const entry of sorted) {
    nextChildren.push(entry.path)
    const existing = nodes[entry.path]
    if (existing && existing.kind === entry.kind) {
      nextNodes[entry.path] = existing
    } else {
      nextNodes[entry.path] = createChildNode(entry)
      childrenChanged = true
    }
  }

  // 剪除消失子树
  const nextChildSet = new Set(nextChildren)
  for (const oldChild of parent.children) {
    if (nextChildSet.has(oldChild)) continue
    const doomed: string[] = []
    collectSubtreePaths(nodes, oldChild, doomed)
    for (const doomedPath of doomed) delete nextNodes[doomedPath]
    childrenChanged = true
  }

  const parentUnchanged =
    !childrenChanged &&
    parent.loaded &&
    !parent.loading &&
    !parent.error &&
    sameStringArray(parent.children, nextChildren)

  if (parentUnchanged) return nodes

  nextNodes[path] = {
    ...parent,
    children: nextChildren,
    loaded: true,
    loading: false,
    error: null,
  }
  return nextNodes
}

/** 标记某目录加载中 / 加载失败（供 hook 在请求前后打点）。 */
export function markNodeLoading(nodes: FileTreeNodes, path: string, loading: boolean, error?: string | null): FileTreeNodes {
  const node = nodes[path]
  if (!node) return nodes
  if (node.loading === loading && (node.error ?? null) === (error ?? null)) return nodes
  return { ...nodes, [path]: { ...node, loading, error: error ?? null } }
}

/** DFS 扁平化可见行。展开且加载出错的目录在其下方补一行 error 行。 */
export function flattenTreeRows(nodes: FileTreeNodes, expandedSet: ReadonlySet<string>): FileTreeRow[] {
  const rows: FileTreeRow[] = []
  const root = nodes[ROOT_PATH]
  if (!root) return rows
  if (root.error) rows.push({ type: 'error', path: ROOT_PATH, depth: 0 })

  const walk = (dirPath: string, depth: number): void => {
    const dir = nodes[dirPath]
    if (!dir) return
    for (const childPath of dir.children) {
      const child = nodes[childPath]
      if (!child) continue
      rows.push({ type: 'node', path: childPath, depth })
      if (child.kind === 'dir' && expandedSet.has(childPath)) {
        if (child.error) rows.push({ type: 'error', path: childPath, depth: depth + 1 })
        walk(childPath, depth + 1)
      }
    }
  }
  walk(ROOT_PATH, 0)
  return rows
}

// ---------- 展开集合（不可变 helpers） ----------

export function addExpanded(set: ReadonlySet<string>, path: string): Set<string> {
  if (set.has(path)) return new Set(set)
  return new Set(set).add(path)
}

export function removeExpanded(set: ReadonlySet<string>, path: string): Set<string> {
  if (!set.has(path)) return new Set(set)
  const next = new Set(set)
  next.delete(path)
  return next
}

export function toggleExpanded(set: ReadonlySet<string>, path: string): Set<string> {
  const next = new Set(set)
  if (next.has(path)) next.delete(path)
  else next.add(path)
  return next
}

/** 目录重命名/移动后，把展开集合里的旧前缀整体改写成新前缀。 */
export function remapExpandedForRename(set: ReadonlySet<string>, fromPath: string, toPath: string): Set<string> {
  const prefix = `${fromPath}/`
  let touched = false
  const next = new Set<string>()
  for (const entry of set) {
    if (entry === fromPath) {
      next.add(toPath)
      touched = true
    } else if (entry.startsWith(prefix)) {
      next.add(`${toPath}/${entry.slice(prefix.length)}`)
      touched = true
    } else {
      next.add(entry)
    }
  }
  return touched ? next : new Set(set)
}
