/// <reference lib="webworker" />

import type { Root } from 'hast'
import remarkCjkFriendly from 'remark-cjk-friendly'
import remarkGfm from 'remark-gfm'
import remarkMath from 'remark-math'
import remarkParse from 'remark-parse'
import remarkRehype from 'remark-rehype'
import { unified } from 'unified'
import { remarkCitations } from './citations'
import { normalizeMarkdownForRender } from './markdownUtils'

interface MarkdownAstRequest {
  id: number
  content: string
  citationNumbers: number[]
}

interface MarkdownAstResponse {
  id: number
  tree?: Root
  error?: string
}

function errorDetailsFence(detail: string): string {
  const longestRun = Math.max(0, ...Array.from(detail.matchAll(/`+/g), (match) => match[0].length))
  return '`'.repeat(Math.max(3, longestRun + 1))
}

function normalizeLegacyErrorDetails(content: string): string {
  return content.replace(
    /<details>\s*<summary>错误详情<\/summary>\s*(`{3,})\s*\n([\s\S]*?)\n\1\s*<\/details>/g,
    (_match, _oldFence: string, detail: string) => {
      const fence = errorDetailsFence(detail)
      return `${fence}kivio-error-details\n${detail}\n${fence}`
    },
  )
}

const remarkRehypeOptions = {
  allowDangerousHtml: true,
  handlers: {
    math: (_state: unknown, node: { value?: string }) => ({
      type: 'element',
      tagName: 'kvmath',
      properties: { display: 'true', tex: node.value ?? '' },
      children: [],
    }),
    inlineMath: (_state: unknown, node: { value?: string }) => ({
      type: 'element',
      tagName: 'kvmath',
      properties: { display: 'false', tex: node.value ?? '' },
      children: [],
    }),
  },
}

self.onmessage = (event: MessageEvent<MarkdownAstRequest>) => {
  const { id, content, citationNumbers } = event.data
  try {
    let processor = unified()
      .use(remarkParse)
      .use(remarkGfm)
      .use(remarkMath)
      .use(remarkCjkFriendly)
    if (citationNumbers.length > 0) {
      processor = processor.use(remarkCitations(new Set(citationNumbers)))
    }
    const normalized = normalizeMarkdownForRender(normalizeLegacyErrorDetails(content))
    const tree = processor
      .use(remarkRehype, remarkRehypeOptions as never)
      .runSync(processor.parse(normalized)) as Root
    const response: MarkdownAstResponse = { id, tree }
    self.postMessage(response)
  } catch (err) {
    const response: MarkdownAstResponse = {
      id,
      error: err instanceof Error ? err.message : String(err),
    }
    self.postMessage(response)
  }
}
