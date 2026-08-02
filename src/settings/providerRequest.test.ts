import { describe, expect, it } from 'vitest'
import {
  effectiveUserAgent,
  HeaderImportError,
  headerIssue,
  mergeImportedHeaders,
  parseHeaderImport,
  suggestHeaderKeys,
} from './providerRequest'

describe('headerIssue', () => {
  it('accepts a normal header', () => {
    expect(headerIssue({ key: 'X-Title', value: 'kivio' }, true)).toBeNull()
  })

  it('rejects reserved keys regardless of case', () => {
    expect(headerIssue({ key: 'Authorization', value: 'x' }, true)).toBe('reserved')
    expect(headerIssue({ key: 'X-API-Key', value: 'x' }, true)).toBe('reserved')
  })

  it('rejects malformed keys and CRLF / non-ASCII values', () => {
    expect(headerIssue({ key: 'Bad Name', value: 'v' }, true)).toBe('invalid-key')
    expect(headerIssue({ key: 'X-A', value: 'a\r\nB: c' }, true)).toBe('invalid-value')
    expect(headerIssue({ key: 'X-A', value: '中文' }, true)).toBe('invalid-value')
  })

  it('stays quiet on a freshly added empty row', () => {
    expect(headerIssue({ key: '', value: '' }, false)).toBeNull()
    expect(headerIssue({ key: '', value: '' }, true)).toBe('invalid-key')
  })
})

describe('parseHeaderImport', () => {
  it('reads a JSON object', () => {
    const result = parseHeaderImport('{"X-Title":"kivio","X-Count":3}')
    expect(result.headers).toEqual([
      { key: 'X-Title', value: 'kivio' },
      { key: 'X-Count', value: '3' },
    ])
    expect(result.issues).toEqual([])
  })

  it('reads a JSON array of key/value objects and reports rejected entries', () => {
    const result = parseHeaderImport(
      '[{"key":"X-Title","value":"kivio"},{"key":"Authorization","value":"Bearer x"},{"key":"X-Obj","value":{}}]',
    )
    expect(result.headers).toEqual([{ key: 'X-Title', value: 'kivio' }])
    expect(result.issues).toEqual([
      { key: 'Authorization', reason: 'reserved' },
      { key: 'X-Obj', reason: 'unsupported-value' },
    ])
  })

  it('reads -H flags out of a cURL command, quotes and line continuations included', () => {
    const curl = [
      'curl https://api.example.com/v1/chat/completions \\',
      "  -H 'X-Title: my app' \\",
      '  -H "HTTP-Referer: https://kivio.dev" \\',
      '  --header X-Env:prod \\',
      "  -d '{\"model\":\"gpt-4o\"}'",
    ].join('\n')
    const result = parseHeaderImport(curl)
    expect(result.headers).toEqual([
      { key: 'X-Title', value: 'my app' },
      { key: 'HTTP-Referer', value: 'https://kivio.dev' },
      { key: 'X-Env', value: 'prod' },
    ])
  })

  it('falls back to plain "Name: value" lines', () => {
    const result = parseHeaderImport('X-Title: kivio\nHTTP-Referer: https://kivio.dev')
    expect(result.headers).toHaveLength(2)
  })

  it('throws on empty input, broken JSON and unterminated quotes', () => {
    expect(() => parseHeaderImport('   ')).toThrow(HeaderImportError)
    expect(() => parseHeaderImport('{oops')).toThrow(HeaderImportError)
    expect(() => parseHeaderImport("curl -H 'X-A: b")).toThrow(HeaderImportError)
    // 错误码要能区分，UI 才能给出对应文案。
    expect(() => parseHeaderImport('{oops')).toThrowError(
      expect.objectContaining({ code: 'invalid-json' }),
    )
    expect(() => parseHeaderImport('')).toThrowError(expect.objectContaining({ code: 'empty' }))
  })

  it('reads the --header=X: y form', () => {
    const result = parseHeaderImport('curl https://x/v1 --header=X-Title:kivio --header="X-Env: prod"')
    expect(result.headers).toEqual([
      { key: 'X-Title', value: 'kivio' },
      { key: 'X-Env', value: 'prod' },
    ])
  })

  it('handles bash escapes outside quotes: escaped spaces and the \'\\\'\' idiom', () => {
    // 转义空格：curl https://x/v1 -H X-Title:\ my\ app
    expect(parseHeaderImport(String.raw`curl https://x/v1 -H X-Title:\ my\ app`).headers).toEqual([
      { key: 'X-Title', value: 'my app' },
    ])
    // 单引号里嵌单引号，bash 的标准写法 '\''
    expect(parseHeaderImport(String.raw`curl https://x/v1 -H 'X-Title: it'\''s'`).headers).toEqual([
      { key: 'X-Title', value: "it's" },
    ])
  })

  it('keeps only the last value when the same key repeats', () => {
    const result = parseHeaderImport('[{"key":"X-A","value":"1"},{"key":"x-a","value":"2"}]')
    expect(result.headers).toEqual([{ key: 'x-a', value: '2' }])
  })
})

describe('mergeImportedHeaders', () => {
  it('adds new keys and overwrites existing ones case-insensitively', () => {
    const { headers, added, overwritten } = mergeImportedHeaders(
      [{ key: 'X-Title', value: 'old' }],
      [
        { key: 'x-title', value: 'new' },
        { key: 'X-Env', value: 'prod' },
      ],
    )
    expect(headers).toEqual([
      { key: 'x-title', value: 'new' },
      { key: 'X-Env', value: 'prod' },
    ])
    expect({ added, overwritten }).toEqual({ added: 1, overwritten: 1 })
  })

  it('collapses pre-existing duplicate rows of the same key', () => {
    // UI 允许手动添两行同名，导入后不能留一条不知道哪条生效的僵尸。
    const { headers, overwritten } = mergeImportedHeaders(
      [
        { key: 'X-Title', value: 'a' },
        { key: 'x-title', value: 'b' },
        { key: 'X-Env', value: 'dev' },
      ],
      [{ key: 'X-Title', value: 'c' }],
    )
    expect(headers).toEqual([
      { key: 'X-Title', value: 'c' },
      { key: 'X-Env', value: 'dev' },
    ])
    expect(overwritten).toBe(1)
  })
})

describe('effectiveUserAgent', () => {
  it('falls back to the preset when no custom UA is set', () => {
    expect(effectiveUserAgent([], 'claude_code', '')).toEqual({
      value: 'claude-cli/2.1.71 (external, cli)',
      source: 'preset',
    })
    expect(effectiveUserAgent([], 'codex', '1.2.3').value).toContain('codex_cli_rs/1.2.3')
  })

  it('lets a custom User-Agent win, taking the last row when duplicated', () => {
    const result = effectiveUserAgent(
      [
        { key: 'user-agent', value: 'first/1' },
        { key: 'User-Agent', value: 'last/2' },
      ],
      'claude_code',
      '',
    )
    // Rust 侧 header_pairs 也是后写的覆盖前面的，两边必须一致。
    expect(result).toEqual({ value: 'last/2', source: 'custom' })
  })

  it('reports none when identity is off and nothing custom is set', () => {
    expect(effectiveUserAgent([], '', '')).toEqual({ value: null, source: 'none' })
  })
})

describe('suggestHeaderKeys', () => {
  it('filters by substring and caps the list', () => {
    expect(suggestHeaderKeys('title')).toEqual(['X-Title'])
    expect(suggestHeaderKeys('')).toHaveLength(6)
  })
})
