import { describe, expect, it } from 'vitest'
import {
  flattenPiSessionTree,
  isPiForkableEntry,
  piSessionEntryText,
  piSessionLeafPath,
  type PiSessionTreeNode,
} from './piSessionTree'

const tree: PiSessionTreeNode[] = [
  {
    entry: { type: 'message', id: 'u1', parentId: null, message: { role: 'user', content: 'First prompt' } },
    children: [
      {
        entry: {
          type: 'message',
          id: 'a1',
          parentId: 'u1',
          message: { role: 'assistant', content: [{ type: 'text', text: 'First answer' }] },
        },
        children: [
          {
            entry: { type: 'message', id: 'u2', parentId: 'a1', message: { role: 'user', content: 'Branch A' } },
            children: [],
          },
          {
            entry: { type: 'message', id: 'u3', parentId: 'a1', message: { role: 'user', content: 'Branch B' } },
            children: [],
          },
        ],
      },
    ],
  },
]

describe('Pi session tree model', () => {
  it('flattens only expanded branches while preserving depth', () => {
    expect(flattenPiSessionTree(tree, new Set())).toEqual([
      expect.objectContaining({ depth: 0, hasChildren: true }),
    ])
    const rows = flattenPiSessionTree(tree, new Set(['u1', 'a1']))
    expect(rows.map((row) => [row.node.entry.id, row.depth])).toEqual([
      ['u1', 0],
      ['a1', 1],
      ['u2', 2],
      ['u3', 2],
    ])
  })

  it('finds the active leaf path for initial expansion', () => {
    expect(piSessionLeafPath(tree, 'u3')).toEqual(['u1', 'a1', 'u3'])
    expect(piSessionLeafPath(tree, 'missing')).toEqual([])
  })

  it('extracts text from heterogeneous entries', () => {
    expect(piSessionEntryText(tree[0].entry)).toBe('First prompt')
    expect(piSessionEntryText(tree[0].children[0].entry)).toBe('First answer')
    expect(piSessionEntryText({ type: 'compaction', id: 'c1', summary: 'Older context' })).toBe('Older context')
    expect(piSessionEntryText({ type: 'model_change', provider: 'openai', modelId: 'gpt-5' })).toBe('openai/gpt-5')
  })

  it('allows native fork only for Pi-reported user message ids', () => {
    const ids = new Set(['u1'])
    expect(isPiForkableEntry(tree[0].entry, ids)).toBe(true)
    expect(isPiForkableEntry(tree[0].children[0].entry, ids)).toBe(false)
    expect(isPiForkableEntry({ type: 'message', id: 'u2', message: { role: 'user' } }, ids)).toBe(false)
  })
})
