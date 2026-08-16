import { describe, it, expect } from 'vitest'
import { splitCitations, remarkCitations, buildCitationMap, type MdNode } from './citations'
import type { ToolCallRecord } from './types'

describe('splitCitations', () => {
  it('splits valid [n] into link nodes, leaves text around', () => {
    const out = splitCitations('see [1] and [2] here', new Set([1, 2]))
    expect(out.map((n) => n.type)).toEqual(['text', 'link', 'text', 'link', 'text'])
    expect(out[1]).toMatchObject({ type: 'link', url: '#kb-cite-1' })
    expect(out[3]).toMatchObject({ type: 'link', url: '#kb-cite-2' })
  })

  it('leaves unknown citation numbers as plain text', () => {
    const out = splitCitations('ref [9] only', new Set([1]))
    expect(out).toEqual([{ type: 'text', value: 'ref [9] only' }])
  })

  it('handles adjacent citations with no text between', () => {
    const out = splitCitations('[1][2]', new Set([1, 2]))
    expect(out.map((n) => n.type)).toEqual(['link', 'link'])
  })
})

describe('remarkCitations', () => {
  it('rewrites text children but skips inside link/code nodes', () => {
    const tree: MdNode = {
      type: 'root',
      children: [
        { type: 'paragraph', children: [{ type: 'text', value: 'a [1] b' }] },
        { type: 'link', url: 'x', children: [{ type: 'text', value: 'keep [1]' }] },
        { type: 'inlineCode', value: 'arr[1]' },
      ],
    }
    remarkCitations(new Set([1]))()(tree)
    // paragraph: text split into [text, link, text]
    expect(tree.children![0].children!.map((n) => n.type)).toEqual(['text', 'link', 'text'])
    // link's inner text is untouched (no nested link)
    expect(tree.children![1].children).toEqual([{ type: 'text', value: 'keep [1]' }])
    // inlineCode leaf untouched
    expect(tree.children![2]).toEqual({ type: 'inlineCode', value: 'arr[1]' })
  })
})

describe('buildCitationMap', () => {
  function tool(overrides: Partial<ToolCallRecord> = {}): ToolCallRecord {
    return { id: 't', toolName: 'web_search', status: 'success', ...overrides }
  }

  it('indexes knowledge_search hits by n', () => {
    const hits = tool({
      id: 'kb',
      toolName: 'knowledge_search',
      source: 'native',
      structured_content: { hits: [{ n: 1, docName: 'doc.md', score: 0.9, text: '段落' }] },
    })
    const map = buildCitationMap([hits])
    expect(map.get(1)).toMatchObject({ docName: 'doc.md', text: '段落' })
  })

  it('indexes web_search citations by their 1-based position', () => {
    const web = tool({
      id: 'ws',
      source: 'native',
      structured_content: {
        type: 'builtin_web_search',
        provider: 'OpenAI',
        queries: ['kivio'],
        citations: [
          { title: 'A', url: 'https://a.com' },
          { title: 'B', url: 'https://www.b.com', snippet: '摘要', published_date: '2025-06-01' },
        ],
      },
    })
    const map = buildCitationMap([web])
    expect(map.get(1)).toMatchObject({ kind: 'web', title: 'A', url: 'https://a.com', host: 'a.com' })
    expect(map.get(2)).toMatchObject({
      kind: 'web',
      title: 'B',
      host: 'b.com',
      snippet: '摘要',
      publishedDate: '2025-06-01',
    })
  })

  it('lets KB hits win over web citations on number collisions', () => {
    const kb = tool({
      id: 'kb',
      toolName: 'knowledge_search',
      source: 'native',
      structured_content: { hits: [{ n: 1, docName: 'doc.md', score: 0.9, text: '段落' }] },
    })
    const web = tool({
      id: 'ws',
      source: 'native',
      structured_content: {
        type: 'builtin_web_search',
        queries: [],
        citations: [{ title: '网页来源', url: 'https://web.com' }],
      },
    })
    const map = buildCitationMap([kb, web])
    // KB 命中优先占据 [1]，web 同号让位（不顶替、也不挪号）。
    expect(map.size).toBe(1)
    expect(map.get(1)).toMatchObject({ docName: 'doc.md' })
  })

  it('lets a later web card overwrite an earlier one on the same number', () => {
    const planning = tool({
      id: 'ws-planning',
      source: 'native',
      structured_content: {
        type: 'builtin_web_search',
        queries: [],
        citations: [{ title: '规划来源', url: 'https://planning.com' }],
      },
    })
    const synthesis = tool({
      id: 'ws-synthesis',
      source: 'native',
      structured_content: {
        type: 'builtin_web_search',
        queries: [],
        citations: [{ title: '正文来源', url: 'https://synthesis.com' }],
      },
    })
    const map = buildCitationMap([planning, synthesis])
    // 两张 web 卡同号竞争 → 后卡（synthesis）覆盖。
    expect(map.size).toBe(1)
    expect(map.get(1)).toMatchObject({ kind: 'web', title: '正文来源' })
  })

  it('skips web citations without a url and non-web-search records', () => {
    const web = tool({
      id: 'ws',
      source: 'native',
      structured_content: {
        type: 'builtin_web_search',
        queries: [],
        citations: [{ title: '无链接' }, { title: 'A', url: 'https://a.com' }],
      },
    })
    const map = buildCitationMap([web, tool({ id: 'read', toolName: 'read' })])
    expect(map.size).toBe(1)
    expect(map.get(1)).toMatchObject({ kind: 'web', title: 'A' })
  })
})
