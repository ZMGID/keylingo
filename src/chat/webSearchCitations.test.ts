import { describe, expect, it } from 'vitest'
import { hostOf, isWebCitation, webSearchCardView } from './webSearchCitations'
import type { ToolCallRecord } from './types'

function tool(overrides: Partial<ToolCallRecord> = {}): ToolCallRecord {
  return { id: 't', toolName: 'web_search', status: 'success', ...overrides }
}

describe('hostOf', () => {
  it('strips www and returns the hostname', () => {
    expect(hostOf('https://www.example.com/path?q=1')).toBe('example.com')
    expect(hostOf('https://sub.example.org')).toBe('sub.example.org')
  })
  it('returns the raw value when the url is unparseable', () => {
    expect(hostOf('not a url')).toBe('not a url')
  })
})

describe('webSearchCardView', () => {
  it('returns null for non-web-search records', () => {
    expect(webSearchCardView(tool({ toolName: 'read' }))).toBeNull()
    expect(webSearchCardView(tool({ toolName: 'knowledge_search' }))).toBeNull()
  })

  it('parses builtin structured content into numbered source views', () => {
    const view = webSearchCardView(
      tool({
        source: 'native',
        structured_content: {
          type: 'builtin_web_search',
          provider: 'OpenAI',
          queries: ['kivio release', '  kivio 下载  '],
          citations: [
            { title: 'A 站', url: 'https://a.com' },
            { title: '', url: 'https://www.b.com' },
          ],
        },
      }),
    )
    expect(view).not.toBeNull()
    expect(view!.provider).toBe('OpenAI')
    expect(view!.queries).toEqual(['kivio release', 'kivio 下载'])
    expect(view!.citations).toHaveLength(2)
    expect(view!.citations[0]).toEqual({ n: 1, title: 'A 站', url: 'https://a.com', host: 'a.com' })
    // 空标题 → 兜底成域名。
    expect(view!.citations[1]).toMatchObject({ n: 2, title: 'b.com', host: 'b.com' })
  })

  it('accepts snake_case and camelCase date fields, drops entries without url', () => {
    const view = webSearchCardView(
      tool({
        structured_content: {
          type: 'third_party_web_search',
          provider: 'Tavily',
          queries: ['天气'],
          citations: [
            { title: '缺链接' },
            { title: '气象台', url: 'https://weather.example/1', published_date: '2025-06-02' },
            { title: 'C 站', url: 'https://c.com', snippet: '摘要', publishedDate: '2025-06-03' },
          ],
        },
      }),
    )
    expect(view!.citations).toHaveLength(2)
    expect(view!.citations[0]).toMatchObject({ n: 1, publishedDate: '2025-06-02' })
    expect(view!.citations[1]).toMatchObject({ n: 2, snippet: '摘要', publishedDate: '2025-06-03' })
  })

  it('handles the search_web alias and empty structured content', () => {
    const view = webSearchCardView(
      tool({
        toolName: 'search_web',
        structured_content: { provider: 'Exa' },
      }),
    )
    expect(view).toEqual({ provider: 'Exa', queries: [], citations: [] })
    expect(webSearchCardView(tool({ structured_content: undefined }))).toBeNull()
  })
})

describe('isWebCitation', () => {
  it('discriminates web citations from KB hits by the url field', () => {
    expect(isWebCitation({ kind: 'web', n: 1, title: 'A', url: 'https://a.com', host: 'a.com' })).toBe(true)
    expect(isWebCitation({ n: 1, docName: 'doc.md', score: 0.9, text: '段落' })).toBe(false)
    expect(isWebCitation(null)).toBe(false)
  })
})
