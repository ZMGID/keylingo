import { Fragment, memo, useMemo, useRef, type ReactNode } from 'react'
import { recordLiveMarkdownParsedCharacters } from './liveMarkdownDiagnostics'

type LiveBlock =
  | { kind: 'paragraph'; text: string; startOffset: number; endOffset: number }
  | { kind: 'heading'; level: number; text: string; startOffset: number; endOffset: number }
  | { kind: 'list'; ordered: boolean; items: string[]; startOffset: number; endOffset: number }
  | { kind: 'quote'; text: string; startOffset: number; endOffset: number }
  | { kind: 'code'; language: string; lines: string[]; closed: boolean; startOffset: number; endOffset: number }

const LIVE_CODE_PREVIEW_LIMIT = 12_000
const LIVE_INLINE_MARKDOWN_LIMIT = 2_048

function isPlainAppend(previous: string, appended: string): boolean {
  if (appended.length > LIVE_INLINE_MARKDOWN_LIMIT) return false
  const previousTail = previous.slice(-LIVE_INLINE_MARKDOWN_LIMIT)
  return !/[`*_#[\]()>+-]/.test(previousTail)
    && !/[`*_#[\]()>+-]/.test(appended)
    && !/\n\s*\n/.test(appended)
}

function isSafeOpaqueParagraphAppend(appended: string): boolean {
  if (/\n\s*\n/.test(appended)) return false
  return !/(?:^|\n)\s*(?:```|#{1,6}\s|[-+*]\s|\d+\.\s|>\s?)/m.test(appended)
}

function appendCodeLines(lines: string[], appended: string): string[] {
  const next = [...lines]
  const chunks = appended.split('\n')
  const lastIndex = Math.max(0, next.length - 1)
  next[lastIndex] = `${next[lastIndex] ?? ''}${chunks[0] ?? ''}`
  if (chunks.length > 1) next.push(...chunks.slice(1))
  return next
}

function inlineMarkdown(value: string): ReactNode[] {
  const nodes: ReactNode[] = []
  const pattern = /(`[^`]+`|\*\*[^*]+\*\*|\*[^*]+\*|\[([^\]]+)\]\(([^)]+)\))/g
  let cursor = 0
  let match: RegExpExecArray | null
  while ((match = pattern.exec(value)) !== null) {
    if (match.index > cursor) nodes.push(value.slice(cursor, match.index))
    const token = match[0]
    const key = `${match.index}:${token}`
    if (token.startsWith('`')) {
      nodes.push(<code key={key}>{token.slice(1, -1)}</code>)
    } else if (token.startsWith('**')) {
      nodes.push(<strong key={key}>{token.slice(2, -2)}</strong>)
    } else if (token.startsWith('*')) {
      nodes.push(<em key={key}>{token.slice(1, -1)}</em>)
    } else {
      const href = match[3] ?? ''
      const label = match[2] ?? token
      if (/^https?:\/\//i.test(href)) {
        nodes.push(
          <a key={key} href={href} target="_blank" rel="noreferrer">
            {label}
          </a>,
        )
      } else {
        nodes.push(label)
      }
    }
    cursor = match.index + token.length
  }
  if (cursor < value.length) nodes.push(value.slice(cursor))
  return nodes
}

function parseBlocks(value: string, offsetBase = 0): LiveBlock[] {
  recordLiveMarkdownParsedCharacters(value.length)
  const lines = value.split('\n')
  const blocks: LiveBlock[] = []
  let paragraphLines: string[] = []
  let paragraphStart = offsetBase
  let listItems: string[] = []
  let listStart = offsetBase
  let listOrdered = false
  let quoteLines: string[] = []
  let quoteStart = offsetBase
  let codeLines: string[] | null = null
  let codeStart = offsetBase
  let codeLanguage = ''
  let lineStart = offsetBase

  const flushParagraph = () => {
    const text = paragraphLines.join(' ').trim()
    if (text) {
      blocks.push({
        kind: 'paragraph',
        text,
        startOffset: paragraphStart,
        endOffset: lineStart,
      })
    }
    paragraphLines = []
  }
  const flushList = () => {
    if (listItems.length > 0) {
      blocks.push({
        kind: 'list',
        ordered: listOrdered,
        items: listItems,
        startOffset: listStart,
        endOffset: lineStart,
      })
    }
    listItems = []
  }
  const flushQuote = () => {
    const text = quoteLines.join(' ').trim()
    if (text) {
      blocks.push({
        kind: 'quote',
        text,
        startOffset: quoteStart,
        endOffset: lineStart,
      })
    }
    quoteLines = []
  }
  const flushFlow = () => {
    flushParagraph()
    flushList()
    flushQuote()
  }
  const flushCode = (closed: boolean) => {
    blocks.push({
      kind: 'code',
      language: codeLanguage,
      lines: codeLines ?? [],
      closed,
      startOffset: codeStart,
      endOffset: lineStart,
    })
    codeLines = null
    codeLanguage = ''
  }

  for (const line of lines) {
    const trimmed = line.trim()
    const fence = trimmed.match(/^```\s*([^\s]*)?\s*$/)
    if (fence) {
      if (codeLines) flushCode(true)
      else {
        flushFlow()
        codeLines = []
        codeStart = lineStart
        codeLanguage = fence[1] ?? ''
      }
      lineStart += line.length + 1
      continue
    }
    if (codeLines) {
      codeLines.push(line)
      lineStart += line.length + 1
      continue
    }
    if (!trimmed) {
      flushFlow()
      lineStart += line.length + 1
      continue
    }
    const heading = trimmed.match(/^(#{1,6})\s+(.+)$/)
    if (heading) {
      flushFlow()
      blocks.push({
        kind: 'heading',
        level: heading[1].length,
        text: heading[2],
        startOffset: lineStart,
        endOffset: lineStart + line.length + 1,
      })
    } else if (/^>\s?/.test(trimmed)) {
      flushParagraph()
      flushList()
      if (quoteLines.length === 0) quoteStart = lineStart
      quoteLines.push(trimmed.replace(/^>\s?/, ''))
    } else {
      const list = trimmed.match(/^(?:[-+*]|(\d+)\.)\s+(.+)$/)
      if (list) {
        flushParagraph()
        flushQuote()
        if (listItems.length === 0) {
          listStart = lineStart
          listOrdered = Boolean(list[1])
        }
        listItems.push(list[2])
      } else {
        flushList()
        flushQuote()
        if (paragraphLines.length === 0) paragraphStart = lineStart
        paragraphLines.push(trimmed)
      }
    }
    lineStart += line.length + 1
  }
  if (codeLines) flushCode(false)
  else flushFlow()
  return blocks
}

function renderBlock(block: LiveBlock): ReactNode {
  if (block.kind === 'heading') {
    const content = inlineMarkdown(block.text)
    const props = { children: content }
    if (block.level === 1) return <h1>{props.children}</h1>
    if (block.level === 2) return <h2>{props.children}</h2>
    if (block.level === 3) return <h3>{props.children}</h3>
    if (block.level === 4) return <h4>{props.children}</h4>
    if (block.level === 5) return <h5>{props.children}</h5>
    return <h6>{props.children}</h6>
  }
  if (block.kind === 'list') {
    const items = block.items.map((item, index) => <li key={`${block.endOffset}:${index}`}>{inlineMarkdown(item)}</li>)
    return block.ordered ? <ol>{items}</ol> : <ul>{items}</ul>
  }
  if (block.kind === 'quote') return <blockquote>{inlineMarkdown(block.text)}</blockquote>
  if (block.kind === 'code') {
    const source = block.lines.join('\n')
    const visible = source.length > LIVE_CODE_PREVIEW_LIMIT
      ? `${source.slice(0, 8_000)}\n\n…\n\n${source.slice(-2_000)}`
      : source
    return (
      <pre>
        <code data-language={block.language || undefined}>{visible}</code>
      </pre>
    )
  }
  return <p>{block.text.length > LIVE_INLINE_MARKDOWN_LIMIT ? block.text : inlineMarkdown(block.text)}</p>
}

export const LiveMarkdown = memo(function LiveMarkdown({ value }: { value: string }) {
  const cacheRef = useRef<{ value: string; blocks: LiveBlock[] }>({ value: '', blocks: [] })
  const blocks = useMemo(() => {
    const normalized = value.replace(/\r\n/g, '\n')
    const previous = cacheRef.current
    if (
      previous.blocks.length > 0
      && normalized.length > previous.value.length
      && normalized.startsWith(previous.value)
    ) {
      const appended = normalized.slice(previous.value.length)
      const last = previous.blocks.at(-1)
      if (last?.kind === 'code' && !last.closed && !appended.includes('`')) {
        const merged = [
          ...previous.blocks.slice(0, -1),
          {
            ...last,
            lines: appendCodeLines(last.lines, appended),
            endOffset: normalized.length,
          },
        ] as LiveBlock[]
        cacheRef.current = { value: normalized, blocks: merged }
        return merged
      }
      if (last?.kind === 'paragraph' && isPlainAppend(previous.value, appended)) {
        const appendedText = appended.replace(/\s+/g, ' ').trim()
        const separator = appendedText
          && (/\s$/.test(previous.value) || /^\s/.test(appended))
          ? ' '
          : ''
        const merged = [
          ...previous.blocks.slice(0, -1),
          {
            ...last,
            text: appendedText ? `${last.text}${separator}${appendedText}` : last.text,
            endOffset: normalized.length,
          },
        ] as LiveBlock[]
        cacheRef.current = { value: normalized, blocks: merged }
        return merged
      }
      // Once a paragraph is beyond the inline parser's bounded preview size,
      // it is intentionally rendered as opaque text. Keep appending that block
      // without reparsing its historical emphasis/link markers; structural
      // boundaries still fall through to the incremental block parser.
      if (
        last?.kind === 'paragraph'
        && last.text.length > LIVE_INLINE_MARKDOWN_LIMIT
        && isSafeOpaqueParagraphAppend(appended)
      ) {
        const appendedText = appended.replace(/\s+/g, ' ').trim()
        const separator = appendedText
          && (/\s$/.test(previous.value) || /^\s/.test(appended))
          ? ' '
          : ''
        const merged = [
          ...previous.blocks.slice(0, -1),
          {
            ...last,
            text: appendedText ? `${last.text}${separator}${appendedText}` : last.text,
            endOffset: normalized.length,
          },
        ] as LiveBlock[]
        cacheRef.current = { value: normalized, blocks: merged }
        return merged
      }
      const reusable = previous.blocks.slice(0, -1)
      const resumeOffset = Math.min(
        normalized.length,
        reusable.at(-1)?.endOffset ?? 0,
      )
      const merged = [...reusable, ...parseBlocks(normalized.slice(resumeOffset), resumeOffset)]
      cacheRef.current = { value: normalized, blocks: merged }
      return merged
    }
    const parsed = parseBlocks(normalized)
    cacheRef.current = { value: normalized, blocks: parsed }
    return parsed
  }, [value])

  return <>{blocks.map((block) => <Fragment key={`${block.kind}:${block.startOffset}`}>{renderBlock(block)}</Fragment>)}</>
})
