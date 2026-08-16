// `web_search` 工具结果解析：把 structured_content 转成来源目录视图模型。
// 两条联网搜索路径共用同一工具名 `web_search`（内置 builtin 合成卡 / 第三方
// search_web 原生工具），structured_content.citations 是统一形状：
//   { title, url, snippet?, published_date? }[]（n 由前端按下标 +1 赋，即卡片目录编号）。
// 拆成独立模块（非组件），避免 ToolCallBlock.tsx 触发 react-refresh/only-export-components。
import type { ToolCallRecord } from './types'
import { canonicalToolName } from './segments'

export interface WebCitationView {
  /** 卡片目录里的 1-based 编号（对应答案正文里的 `[n]` 角标）。 */
  n: number
  title: string
  url: string
  /** 去掉 `www.` 的域名，作展示与兜底标题。 */
  host: string
  snippet?: string
  publishedDate?: string
}

export interface WebSearchCardView {
  /** 实际使用的搜索服务名（内置=provider 标签，第三方=Tavily/Exa/…）。 */
  provider?: string
  queries: string[]
  citations: WebCitationView[]
}

function asObject(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value)
    ? (value as Record<string, unknown>)
    : null
}

function asString(value: unknown): string {
  return typeof value === 'string' ? value : ''
}

function asStringArray(value: unknown): string[] {
  if (!Array.isArray(value)) return []
  return value
    .filter((item): item is string => typeof item === 'string' && item.trim().length > 0)
    .map((item) => item.trim())
}

/** URL → 展示域名（去 www.；解析失败原样返回，绝不 throw）。 */
export function hostOf(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, '')
  } catch {
    return url
  }
}

/** 解析 web_search 记录的卡片视图；非 web_search 记录或解析不到任何引用时返回 null。
 *  名字走 `canonicalToolName` 折叠，外部 CLI 的 `WebSearch`/`websearch` 也能对上。 */
export function webSearchCardView(toolCall: ToolCallRecord): WebSearchCardView | null {
  const name = canonicalToolName(toolCall)
  if (name !== 'web_search' && name !== 'search_web') return null
  const structured = asObject(toolCall.structured_content ?? toolCall.structuredContent)
  if (!structured) return null
  const rawCitations = Array.isArray(structured.citations) ? structured.citations : []
  const citations = rawCitations
    .map((raw): Omit<WebCitationView, 'n'> | null => {
      const o = asObject(raw)
      const url = asString(o?.url)
      if (!url) return null
      return {
        title: asString(o?.title) || hostOf(url),
        url,
        host: hostOf(url),
        snippet: asString(o?.snippet) || undefined,
        publishedDate: asString(o?.published_date) || asString(o?.publishedDate) || undefined,
      }
    })
    .filter((view): view is Omit<WebCitationView, 'n'> => Boolean(view))
    // n 按「过滤后」的下标 +1 赋——后端 citations 数组顺序即来源目录顺序，
    // 缺 url 的脏条目直接剔除，不占编号位。
    .map((view, index) => ({ ...view, n: index + 1 }))
  return {
    provider: asString(structured.provider) || undefined,
    queries: asStringArray(structured.queries),
    citations,
  }
}

/** 引用视图的判别联合：知识库命中 vs 联网来源（ChatMarkdown 角标弹窗用）。 */
export type WebCitationRef = WebCitationView & { kind: 'web' }

/** 判断一条引用视图是不是联网来源（KB 命中没有 url 字段）。 */
export function isWebCitation(view: unknown): view is WebCitationRef {
  return Boolean(view && typeof view === 'object' && 'url' in (view as object))
}
