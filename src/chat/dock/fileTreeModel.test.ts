import { describe, expect, it } from 'vitest'
import {
  addExpanded,
  ancestorsOf,
  applyListResponse,
  basenameOf,
  createRootNode,
  flattenTreeRows,
  joinPath,
  parentDirOf,
  remapExpandedForRename,
  ROOT_PATH,
  sortEntries,
  toggleExpanded,
  type FileTreeNodes,
} from './fileTreeModel'
import type { DockFsEntry } from './types'

function entry(path: string, kind: 'file' | 'dir' = 'file', hidden = false): DockFsEntry {
  return { path, kind, hidden }
}

function nodesWithRoot(): FileTreeNodes {
  return { [ROOT_PATH]: createRootNode() }
}

describe('path helpers', () => {
  it('basenameOf / parentDirOf / joinPath', () => {
    expect(basenameOf('src/a/b.ts')).toBe('b.ts')
    expect(basenameOf('b.ts')).toBe('b.ts')
    expect(parentDirOf('src/a/b.ts')).toBe('src/a')
    expect(parentDirOf('b.ts')).toBe(ROOT_PATH)
    expect(joinPath('src/a', 'b.ts')).toBe('src/a/b.ts')
    expect(joinPath('', 'b.ts')).toBe('b.ts')
  })

  it('ancestorsOf returns shallow-to-deep ancestor dirs', () => {
    expect(ancestorsOf('src/a/b.ts')).toEqual(['src', 'src/a'])
    expect(ancestorsOf('b.ts')).toEqual([])
  })
})

describe('sortEntries', () => {
  it('dirs first, then localeCompare on lowercase basename', () => {
    const sorted = sortEntries([
      entry('zeta.ts'),
      entry('Beta', 'dir'),
      entry('alpha.ts'),
      entry('apple', 'dir'),
    ])
    expect(sorted.map((e) => e.path)).toEqual(['apple', 'Beta', 'alpha.ts', 'zeta.ts'])
  })

  it('does not mutate the input array', () => {
    const input = [entry('b.ts'), entry('a.ts')]
    sortEntries(input)
    expect(input.map((e) => e.path)).toEqual(['b.ts', 'a.ts'])
  })
})

describe('applyListResponse', () => {
  it('fills children on first load and sorts them', () => {
    const nodes = applyListResponse(nodesWithRoot(), ROOT_PATH, [
      entry('b.ts'),
      entry('a', 'dir'),
    ])
    expect(nodes[ROOT_PATH].children).toEqual(['a', 'b.ts'])
    expect(nodes[ROOT_PATH].loaded).toBe(true)
    expect(nodes.a.kind).toBe('dir')
  })

  it('returns the previous reference when content is unchanged (idempotent)', () => {
    const entries = [entry('a', 'dir'), entry('b.ts')]
    const first = applyListResponse(nodesWithRoot(), ROOT_PATH, entries)
    const second = applyListResponse(first, ROOT_PATH, entries)
    expect(second).toBe(first)
  })

  it('preserves child node identity across reloads', () => {
    let nodes = applyListResponse(nodesWithRoot(), ROOT_PATH, [entry('a', 'dir'), entry('b.ts')])
    nodes = applyListResponse(nodes, 'a', [entry('a/c.ts')])
    const childBefore = nodes.a
    const reloaded = applyListResponse(nodes, ROOT_PATH, [entry('a', 'dir'), entry('b.ts'), entry('d.ts')])
    expect(reloaded.a).toBe(childBefore)
    expect(reloaded.a.children).toEqual(['a/c.ts'])
    expect(reloaded[ROOT_PATH].children).toEqual(['a', 'b.ts', 'd.ts'])
  })

  it('prunes vanished subtrees', () => {
    let nodes = applyListResponse(nodesWithRoot(), ROOT_PATH, [entry('a', 'dir'), entry('b.ts')])
    nodes = applyListResponse(nodes, 'a', [entry('a/c.ts')])
    const pruned = applyListResponse(nodes, ROOT_PATH, [entry('b.ts')])
    expect(pruned[ROOT_PATH].children).toEqual(['b.ts'])
    expect(pruned.a).toBeUndefined()
    expect(pruned['a/c.ts']).toBeUndefined()
  })

  it('recreates the node when kind changes (file became dir)', () => {
    let nodes = applyListResponse(nodesWithRoot(), ROOT_PATH, [entry('x')])
    nodes = applyListResponse(nodes, ROOT_PATH, [entry('x', 'dir')])
    expect(nodes.x.kind).toBe('dir')
    expect(nodes.x.loaded).toBe(false)
  })
})

describe('flattenTreeRows', () => {
  function buildTree(): FileTreeNodes {
    let nodes = applyListResponse(nodesWithRoot(), ROOT_PATH, [
      entry('a', 'dir'),
      entry('b', 'dir'),
      entry('c.ts'),
    ])
    nodes = applyListResponse(nodes, 'a', [entry('a/inner.ts')])
    return nodes
  }

  it('collapses everything by default (root children only)', () => {
    const rows = flattenTreeRows(buildTree(), new Set())
    expect(rows.map((row) => row.path)).toEqual(['a', 'b', 'c.ts'])
    expect(rows.every((row) => row.depth === 0)).toBe(true)
  })

  it('DFS with depth when dirs are expanded', () => {
    const rows = flattenTreeRows(buildTree(), new Set(['a']))
    expect(rows.map((row) => [row.path, row.depth])).toEqual([
      ['a', 0],
      ['a/inner.ts', 1],
      ['b', 0],
      ['c.ts', 0],
    ])
  })

  it('emits an error row under an expanded dir that failed to load', () => {
    let nodes = buildTree()
    nodes = { ...nodes, a: { ...nodes.a, error: 'boom' } }
    const rows = flattenTreeRows(nodes, new Set(['a']))
    const errorRow = rows.find((row) => row.type === 'error')
    expect(errorRow).toMatchObject({ path: 'a', depth: 1 })
  })
})

describe('expanded set helpers', () => {
  it('toggleExpanded adds and removes', () => {
    const empty = new Set<string>()
    const added = toggleExpanded(empty, 'a')
    expect([...added]).toEqual(['a'])
    expect(empty.size).toBe(0)
    expect(toggleExpanded(added, 'a').size).toBe(0)
  })

  it('addExpanded keeps identity-friendly copies', () => {
    const set = new Set(['a'])
    expect(addExpanded(set, 'a').has('a')).toBe(true)
    expect(addExpanded(set, 'b').has('b')).toBe(true)
  })

  it('remapExpandedForRename remaps the dir and its descendants', () => {
    const set = new Set(['src', 'src/a', 'src/a/deep', 'other'])
    const remapped = remapExpandedForRename(set, 'src/a', 'src/b')
    expect([...remapped].sort()).toEqual(['other', 'src', 'src/b', 'src/b/deep'])
  })

  it('remapExpandedForRename does not touch prefix lookalikes', () => {
    const set = new Set(['src/ab'])
    const remapped = remapExpandedForRename(set, 'src/a', 'src/b')
    expect([...remapped]).toEqual(['src/ab'])
  })
})
