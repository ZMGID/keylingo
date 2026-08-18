import { isValidElement, memo, useContext, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { Code2, ExternalLink, Eye, Loader2 } from 'lucide-react'
import type { Components, UrlTransform } from 'streamdown'
import { defaultRemarkPlugins, Streamdown } from 'streamdown'
import type { PluggableList } from 'unified'
import { cjk } from '@streamdown/cjk'
import { code } from '@streamdown/code'
import { createMathPlugin } from '@streamdown/math'
import { mermaid } from '@streamdown/mermaid'
import remarkBreaks from 'remark-breaks'
import { normalizeMarkdownForRender, preserveLocalMarkdownLinks } from './markdownUtils'
import { MarkdownErrorBoundary } from './MarkdownErrorBoundary'
import type { ChatToolArtifact } from './types'
import { artifactDataUrl } from './artifacts'
import { loadArtifactDataUrl } from './attachmentPreview'
import { remarkCitations, type CitationView } from './citations'
import { citationPopoverPosition, type CitationPopoverPosition } from './citationPopover'
import { isWebCitation } from './webSearchCitations'
import { ChatInlineImage } from './ChatInlineImage'
import { MarkdownStreamingContext } from './markdownStreaming'
import { getSettledMarkdownCacheEntry } from './settledMarkdownCache'
import { ChatHeavyIsland } from './ChatHeavyIsland'
import { useConversationTransition } from './conversationTransitionStore'
import { api } from '../api/tauri'
import { copyToClipboard } from '../utils/clipboard'
import { IconButton } from '../components/Button'

interface ChatMarkdownProps {
  content: string
  artifacts?: ChatToolArtifact[]
  /** 用于把外置 artifact（path + 缩略图）还原成整图 */
  conversationId?: string | null
  onImageClick?: (src: string, alt: string, name?: string) => void
  variant?: 'default' | 'reasoning' | 'lens' | 'lens-muted'
  /** 引用：把答案里的 `[n]` 渲染成可点来源片段（n → KB 命中片段或联网来源）。 */
  citations?: Map<number, CitationView>
}

// 排版只走 Streamdown 默认样式；变体只改外壳字号/颜色。
// 代码块 / 表格 / 本地链接 / artifact 图仍用 components 做应用能力，不改正文排版。
function markdownShellClass(variant: ChatMarkdownProps['variant']): string {
  switch (variant) {
    case 'reasoning':
      return 'chat-markdown chat-reasoning-markdown max-w-none break-words text-sm leading-relaxed text-neutral-400 dark:text-neutral-500'
    case 'lens':
      return 'chat-markdown max-w-none break-words text-[13.5px] leading-7 text-neutral-800 dark:text-neutral-200'
    case 'lens-muted':
      return 'chat-markdown max-w-none break-words text-[12.5px] leading-6 text-neutral-500 dark:text-neutral-400'
    default:
      return 'chat-markdown max-w-none break-words text-[15px] leading-[1.7] text-neutral-900 dark:text-neutral-100'
  }
}

function codeChildrenToString(children: unknown): string {
  if (Array.isArray(children)) return children.map((child) => String(child ?? '')).join('')
  return typeof children === 'string' ? children : String(children ?? '')
}

type HighlightToken = {
  text: string
  className?: string
}

type TokenRule = {
  className: string
  pattern: RegExp
}

/** 语法高亮 token 色：浅色/暗色各一套，避免暗色主题下对比度不足。 */
const syntax = {
  comment: 'text-neutral-400 dark:text-neutral-500',
  string: 'text-emerald-700 dark:text-emerald-400',
  keyword: 'text-blue-700 dark:text-blue-400',
  literal: 'text-amber-700 dark:text-amber-400',
  fn: 'text-cyan-700 dark:text-cyan-400',
  type: 'text-violet-700 dark:text-violet-400',
  number: 'text-orange-700 dark:text-orange-400',
  punct: 'text-neutral-500 dark:text-neutral-400',
  tag: 'text-blue-700 dark:text-blue-400',
  attr: 'text-amber-700 dark:text-amber-400',
  selector: 'text-rose-700 dark:text-rose-400',
  atRule: 'text-cyan-700 dark:text-cyan-400',
  unit: 'text-orange-700 dark:text-orange-400',
  cssKw: 'text-violet-700 dark:text-violet-400',
}

const LANGUAGE_LABELS: Record<string, string> = {
  bash: 'Shell',
  cjs: 'JavaScript',
  css: 'CSS',
  html: 'HTML',
  js: 'JavaScript',
  javascript: 'JavaScript',
  json: 'JSON',
  jsx: 'JavaScript',
  markdown: 'Markdown',
  md: 'Markdown',
  mermaid: 'Mermaid',
  py: 'Python',
  python: 'Python',
  rs: 'Rust',
  rust: 'Rust',
  sh: 'Shell',
  shell: 'Shell',
  ts: 'TypeScript',
  tsx: 'TypeScript',
  typescript: 'TypeScript',
  xml: 'XML',
  yaml: 'YAML',
  yml: 'YAML',
}

const jsKeywords =
  'abstract|as|async|await|break|case|catch|class|const|continue|debugger|declare|default|delete|do|else|enum|export|extends|finally|for|from|function|get|if|implements|import|in|infer|instanceof|interface|keyof|let|module|namespace|new|of|private|protected|public|readonly|return|satisfies|set|static|super|switch|throw|try|type|typeof|var|void|while|with|yield'
const rustKeywords =
  'as|async|await|break|const|continue|crate|dyn|else|enum|extern|false|fn|for|if|impl|in|let|loop|match|mod|move|mut|pub|ref|return|self|Self|static|struct|super|trait|true|type|unsafe|use|where|while'
const pythonKeywords =
  'and|as|assert|async|await|break|class|continue|def|del|elif|else|except|False|finally|for|from|global|if|import|in|is|lambda|None|nonlocal|not|or|pass|raise|return|True|try|while|with|yield'

function normalizeCodeLanguage(language?: string): string {
  return (language ?? '').trim().toLowerCase().replace(/^language-/, '')
}

function codeLanguageLabel(language: string): string {
  if (!language) return 'Code'
  return LANGUAGE_LABELS[language] ?? language.toUpperCase()
}

function tokenPattern(source: string): RegExp {
  return new RegExp(source, 'y')
}

// 空白整段吞：**没有任何规则能以空白字符起头**（每条规则都以非空白字符类开头，`\b` 在空白位
// 也只会因后续字符类不匹配而失败），所以整段空白可以一次跳过，不必对每个空白字符把全部规则
// 都试一遍。缩进和换行在代码块里占比很大 —— 原来 5 万字符的代码要跑约 50 万次正则。
const WHITESPACE_RUN = /\s+/y

function scanTokens(code: string, rules: TokenRule[]): HighlightToken[] {
  const tokens: HighlightToken[] = []
  let index = 0
  // 无分类文本按「运行区间」攒着，末尾一次 slice 出来。原来是逐字符 `text +=`，
  // 长普通文本段会反复重建字符串。
  let plainStart = -1

  const flushPlain = (end: number) => {
    if (plainStart < 0) return
    tokens.push({ text: code.slice(plainStart, end) })
    plainStart = -1
  }

  while (index < code.length) {
    WHITESPACE_RUN.lastIndex = index
    const whitespace = WHITESPACE_RUN.exec(code)
    if (whitespace) {
      if (plainStart < 0) plainStart = index
      index += whitespace[0].length
      continue
    }

    let matched = false
    for (const rule of rules) {
      rule.pattern.lastIndex = index
      const match = rule.pattern.exec(code)
      if (!match?.[0]) continue
      flushPlain(index)
      tokens.push({ text: match[0], className: rule.className })
      index += match[0].length
      matched = true
      break
    }

    if (!matched) {
      if (plainStart < 0) plainStart = index
      index += 1
    }
  }

  flushPlain(code.length)
  return tokens
}

function cLikeRules(keywordSource: string): TokenRule[] {
  return [
    { className: syntax.comment, pattern: tokenPattern(String.raw`\/\/[^\n]*|\/\*[\s\S]*?\*\/`) },
    { className: syntax.string, pattern: tokenPattern(String.raw`'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*"`) },
    { className: syntax.keyword, pattern: tokenPattern(String.raw`\b(?:${keywordSource})\b`) },
    { className: syntax.literal, pattern: tokenPattern(String.raw`\b(?:true|false|null|undefined|Some|None|Ok|Err)\b`) },
    { className: syntax.fn, pattern: tokenPattern(String.raw`\b[A-Za-z_$][\w$]*(?=\s*\()`) },
    { className: syntax.type, pattern: tokenPattern(String.raw`\b[A-Z][A-Za-z0-9_$]*\b`) },
    { className: syntax.number, pattern: tokenPattern(String.raw`\b(?:0x[\da-fA-F]+|\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)\b`) },
    { className: syntax.punct, pattern: tokenPattern(String.raw`=>|->|::|[{}()[\].,;:+\-*/%=&|!<>?]+`) },
  ]
}

function jsxRules(keywordSource: string): TokenRule[] {
  return [
    { className: syntax.comment, pattern: tokenPattern(String.raw`\/\/[^\n]*|\/\*[\s\S]*?\*\/`) },
    { className: syntax.string, pattern: tokenPattern(String.raw`'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*"`) },
    { className: syntax.tag, pattern: tokenPattern(String.raw`<\/?[A-Za-z][\w:.-]*`) },
    { className: syntax.attr, pattern: tokenPattern(String.raw`\b[A-Za-z_:][\w:.-]*(?=\s*=)`) },
    { className: syntax.keyword, pattern: tokenPattern(String.raw`\b(?:${keywordSource})\b`) },
    { className: syntax.literal, pattern: tokenPattern(String.raw`\b(?:true|false|null|undefined)\b`) },
    { className: syntax.fn, pattern: tokenPattern(String.raw`\b[A-Za-z_$][\w$]*(?=\s*\()`) },
    { className: syntax.type, pattern: tokenPattern(String.raw`\b[A-Z][A-Za-z0-9_$]*\b`) },
    { className: syntax.number, pattern: tokenPattern(String.raw`\b(?:0x[\da-fA-F]+|\d+(?:\.\d+)?(?:[eE][+-]?\d+)?)\b`) },
    { className: syntax.punct, pattern: tokenPattern(String.raw`\/?>|=>|[{}()[\].,;:+\-*/%=&|!<>?]+`) },
  ]
}

function looksLikeJsx(code: string): boolean {
  return /<\/?[A-Za-z][\w:.-]*(?:\s|>|\/>)/.test(code)
}

function rulesForLanguage(language: string, code = ''): TokenRule[] {
  if (language === 'css') {
    return [
      { className: syntax.comment, pattern: tokenPattern(String.raw`\/\*[\s\S]*?\*\/`) },
      { className: syntax.string, pattern: tokenPattern(String.raw`'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*"`) },
      { className: syntax.selector, pattern: tokenPattern(String.raw`[#.][A-Za-z_][\w-]*`) },
      { className: syntax.atRule, pattern: tokenPattern(String.raw`@[A-Za-z-]+`) },
      { className: syntax.keyword, pattern: tokenPattern(String.raw`\b[A-Za-z-]+(?=\s*:)`) },
      { className: syntax.unit, pattern: tokenPattern(String.raw`#[\da-fA-F]{3,8}\b|\b\d+(?:\.\d+)?(?:px|rem|em|%|vh|vw|s|ms)?\b`) },
      { className: syntax.cssKw, pattern: tokenPattern(String.raw`\b(?:border-box|flex|grid|block|inline|none|relative|absolute|fixed|sticky|solid|transparent)\b`) },
      { className: syntax.punct, pattern: tokenPattern(String.raw`[{}():;,>+~*-]+`) },
    ]
  }

  if (language === 'html' || language === 'xml') {
    return [
      { className: syntax.comment, pattern: tokenPattern(String.raw`<!--[\s\S]*?-->`) },
      { className: syntax.tag, pattern: tokenPattern(String.raw`<\/?[A-Za-z][\w:-]*`) },
      { className: syntax.attr, pattern: tokenPattern(String.raw`\b[A-Za-z_:][\w:.-]*(?=\=)`) },
      { className: syntax.string, pattern: tokenPattern(String.raw`'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*"`) },
      { className: syntax.punct, pattern: tokenPattern(String.raw`\/?>|=`) },
    ]
  }

  if (language === 'json') {
    return [
      { className: syntax.keyword, pattern: tokenPattern(String.raw`"(?:\\.|[^"\\])*"(?=\s*:)`) },
      { className: syntax.string, pattern: tokenPattern(String.raw`"(?:\\.|[^"\\])*"`) },
      { className: syntax.literal, pattern: tokenPattern(String.raw`\b(?:true|false|null)\b`) },
      { className: syntax.number, pattern: tokenPattern(String.raw`-?\b\d+(?:\.\d+)?(?:[eE][+-]?\d+)?\b`) },
      { className: syntax.punct, pattern: tokenPattern(String.raw`[{}[\]:,]+`) },
    ]
  }

  if (language === 'py' || language === 'python') {
    return [
      { className: syntax.comment, pattern: tokenPattern(String.raw`#[^\n]*`) },
      { className: syntax.string, pattern: tokenPattern(String.raw`'''[\s\S]*?'''|"""[\s\S]*?"""|'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*"`) },
      { className: syntax.keyword, pattern: tokenPattern(String.raw`\b(?:${pythonKeywords})\b`) },
      { className: syntax.fn, pattern: tokenPattern(String.raw`\b[A-Za-z_]\w*(?=\s*\()`) },
      { className: syntax.number, pattern: tokenPattern(String.raw`\b\d+(?:\.\d+)?\b`) },
      { className: syntax.punct, pattern: tokenPattern(String.raw`[{}()[\].,;:+\-*/%=&|!<>?]+`) },
    ]
  }

  if (language === 'sh' || language === 'shell' || language === 'bash') {
    return [
      { className: syntax.comment, pattern: tokenPattern(String.raw`#[^\n]*`) },
      { className: syntax.string, pattern: tokenPattern(String.raw`'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*"`) },
      { className: syntax.keyword, pattern: tokenPattern(String.raw`\b(?:case|cat|cd|cp|do|done|echo|elif|else|esac|export|fi|for|function|git|grep|if|mkdir|mv|npm|rg|rm|sed|then|while)\b`) },
      { className: syntax.type, pattern: tokenPattern(String.raw`\$[A-Za-z_]\w*|\$\{[^}]+\}`) },
      { className: syntax.number, pattern: tokenPattern(String.raw`\b\d+\b`) },
      { className: syntax.punct, pattern: tokenPattern(String.raw`[|&;<>(){}[\]!*?=]+`) },
    ]
  }

  if (language === 'rust' || language === 'rs') {
    return cLikeRules(rustKeywords)
  }

  if (language === 'jsx' || language === 'tsx') {
    return jsxRules(jsKeywords)
  }

  if (language === 'js' || language === 'javascript' || language === 'ts' || language === 'typescript') {
    if (looksLikeJsx(code)) return jsxRules(jsKeywords)
    return cLikeRules(jsKeywords)
  }

  return [
    { className: syntax.comment, pattern: tokenPattern(String.raw`\/\/[^\n]*|#[^\n]*|\/\*[\s\S]*?\*\/`) },
    { className: syntax.string, pattern: tokenPattern(String.raw`'(?:\\.|[^'\\])*'|"(?:\\.|[^"\\])*"`) },
    { className: syntax.number, pattern: tokenPattern(String.raw`\b\d+(?:\.\d+)?\b`) },
  ]
}

// 高亮结果缓存：键 = 语言 + 源码。虚拟列表会卸载屏外气泡，往回翻或切回同一会话时同一批
// 代码块会整批重新挂载 —— 一个大对话里有两百多个代码块，重扫 + 重建元素数组不便宜。
// 与 mermaidSvgCache / texCache 同一模式：用外部 Map 而非 useMemo（React 可能丢弃 useMemo）。
// React 元素是不可变描述符，跨挂载复用安全。
const highlightCache = new Map<string, ReactNode[]>()
const HIGHLIGHT_CACHE_MAX = 400

// 导出给 dock 文件查看器复用（逐行调用，块注释跨行会降级——查看器场景可接受）。
// cache=false（流式中的增长块）只读不写：增长块每个 token 全文都变、键永 miss，
// 若照写会把每个前缀版本都灌进 LRU —— 一个长代码块流完能把几百条已定稿条目全部
// 挤光，回翻历史时整批重扫。
// eslint-disable-next-line react-refresh/only-export-components -- 纯函数 helper，热更新损失可接受
export function highlightCode(code: string, language: string, options?: { cache?: boolean }) {
  const key = `${language}\n${code}`
  const cached = highlightCache.get(key)
  if (cached) {
    // LRU：命中挪到队尾。
    highlightCache.delete(key)
    highlightCache.set(key, cached)
    return cached
  }
  const rendered = scanTokens(code, rulesForLanguage(language, code)).map((token, index) => (
    token.className
      ? <span key={index} className={token.className}>{token.text}</span>
      : token.text
  ))
  if (options?.cache !== false) {
    highlightCache.set(key, rendered)
    if (highlightCache.size > HIGHLIGHT_CACHE_MAX) {
      const oldest = highlightCache.keys().next().value
      if (oldest !== undefined) highlightCache.delete(oldest)
    }
  }
  return rendered
}

function normalizeCodeBlockText(code: string): string {
  return code.replace(/\n$/, '')
}

function errorDetailsFence(detail: string): string {
  const longestRun = Math.max(0, ...Array.from(detail.matchAll(/`+/g), (match) => match[0].length))
  return '`'.repeat(Math.max(3, longestRun + 1))
}

// Older external-agent failures were persisted with literal HTML. ReactMarkdown intentionally
// does not enable raw HTML, so migrate only Kivio's exact legacy disclosure shape into the safe
// fenced block below. Arbitrary model-authored HTML remains inert text.
function normalizeLegacyErrorDetails(content: string): string {
  return content.replace(
    /<details>\s*<summary>错误详情<\/summary>\s*(`{3,})\s*\n([\s\S]*?)\n\1\s*<\/details>/g,
    (_match, _oldFence: string, detail: string) => {
      const fence = errorDetailsFence(detail)
      return `${fence}kivio-error-details\n${detail}\n${fence}`
    },
  )
}

function ErrorDetails({ detail }: { detail: string }) {
  return (
    <details className="not-prose my-3 overflow-hidden rounded-md border border-red-200/80 bg-red-50/60 dark:border-red-900/70 dark:bg-red-950/20">
      <summary className="cursor-pointer select-none px-3 py-2 text-xs font-medium text-red-700 marker:text-red-400 dark:text-red-300 dark:marker:text-red-600">
        错误详情
      </summary>
      <pre className="custom-scrollbar m-0 max-h-64 overflow-auto border-t border-red-200/70 bg-transparent px-3 py-2 text-xs leading-5 text-red-800 dark:border-red-900/60 dark:text-red-200">
        <code className="whitespace-pre-wrap break-words font-mono">{normalizeCodeBlockText(detail)}</code>
      </pre>
    </details>
  )
}

function readDocumentDark(): boolean {
  return typeof document !== 'undefined' && document.documentElement.classList.contains('dark')
}

function useDocumentDark(): boolean {
  const [dark, setDark] = useState(readDocumentDark)

  useEffect(() => {
    const root = document.documentElement
    const sync = () => setDark(root.classList.contains('dark'))
    const observer = new MutationObserver(sync)
    observer.observe(root, { attributes: true, attributeFilter: ['class'] })
    return () => observer.disconnect()
  }, [])

  return dark
}

function mermaidThemeVariables(dark: boolean) {
  if (dark) {
    return {
      background: 'transparent',
      primaryColor: '#334155',
      primaryBorderColor: '#64748b',
      primaryTextColor: '#f1f5f9',
      lineColor: '#94a3b8',
      secondaryColor: '#1e293b',
      tertiaryColor: '#0f172a',
      fontFamily: 'ui-sans-serif, system-ui, sans-serif',
    }
  }
  return {
    background: 'transparent',
    primaryColor: '#f8fafc',
    primaryBorderColor: '#94a3b8',
    primaryTextColor: '#111827',
    lineColor: '#64748b',
    secondaryColor: '#f1f5f9',
    tertiaryColor: '#ffffff',
    fontFamily: 'ui-sans-serif, system-ui, sans-serif',
  }
}

function CodeBlock({ code, language, actions }: { code: string; language: string; actions?: ReactNode }) {
  const normalizedCode = useMemo(() => normalizeCodeBlockText(code), [code])
  // 流式中的增长块只读缓存不写（见 highlightCode 注释），定稿后首次渲染才入缓存。
  const streaming = useContext(MarkdownStreamingContext)
  const highlighted = useMemo(
    () => highlightCode(normalizedCode, language, { cache: !streaming }),
    [normalizedCode, language, streaming],
  )
  const [copied, setCopied] = useState(false)

  const handleCopy = async () => {
    const ok = await copyToClipboard(normalizedCode)
    if (!ok) return
    setCopied(true)
    window.setTimeout(() => setCopied(false), 1600)
  }

  // 无独立头栏：语言 + 复制浮在右上。长行横向滚动时会从按钮下穿过，
  // 所以控件要有不透明底；首行用 pt 让开控件高度，避免和 "Code / 复制" 叠字。
  //
  // 语言标签和复制图标都用**伪元素**画（`.kv-code-toolbar::before` 取 data-code-lang，
  // `.kv-copy-glyph` 的 ::before/::after 画两个方块或一个勾）。伪元素不进 DOM 树，
  // 而一个大对话里有两百多个代码块：原来每块是 figure + 工具条 div + 语言 span + button
  // + lucide svg + svg 内的 rect/path + pre + code = 9 个节点，现在 6 个。
  // 每块省 3 个节点，231 块省约 700 个。
  return (
    <figure className="not-prose relative my-3 overflow-hidden rounded-lg border border-[var(--border-input)] bg-[var(--bg-input)] text-neutral-950 shadow-sm dark:text-neutral-100">
      <div
        className="kv-code-toolbar absolute right-1.5 top-1.5 z-10 flex items-center gap-1 rounded-md bg-[var(--bg-input)] pl-2"
        data-code-lang={codeLanguageLabel(language)}
      >
        {actions}
        <IconButton
          size="sm"
          onClick={() => void handleCopy()}
          label={copied ? '已复制' : '复制代码'}
        >
          <span className={copied ? 'kv-copy-glyph is-copied' : 'kv-copy-glyph'} aria-hidden="true" />
        </IconButton>
      </div>
      <pre className="custom-scrollbar m-0 max-w-full overflow-x-auto bg-transparent px-4 pb-4 pt-10 text-[13px] leading-6 text-neutral-900 dark:text-neutral-100">
        <code className="font-mono">{highlighted}</code>
      </pre>
    </figure>
  )
}

function DeferredCodeBlock({ code, language }: { code: string; language: string }) {
  // 会话切换 / 导航·回底 settle / 流式中 / 流式结束短窗：同步 hydrate。
  // 流式时若仍 180ms 延迟，代码块从占位撑开 → 高度突跳 → 贴底丢帧再抽。
  // 平常回翻历史仍延迟，省成本。
  const { loading: conversationOpening } = useConversationTransition()
  const streaming = useContext(MarkdownStreamingContext)
  // ⚠️ fallback 必须与 CodeBlock 逐像素同几何：同 figure（my-3 + border）、同 pre
  // padding（pt-10 pb-4 px-4）、同 nowrap 横向滚动、渲染**全文**（不截断）。
  // 回翻历史时行先按 fallback 首测入列，~180ms 后 hydrate 换真身；backward 滚动中的
  // re-measure 刻意不做滚动补偿（shouldAdjustChatItemSizeChange 对齐上游默认），
  // fallback 与真身的任何高度差都会直接变成「翻历史时抽一下」。旧 fallback 是裸
  // pre（少 24px 外边距/边框、py-4 vs pt-10）+ pre-wrap（长行换行）+ >14k 截断，
  // 三处全在制造高度差。纯文本是单个 text node，渲染很便宜 —— 贵的是高亮 token
  // span，所以全文照渲、只延后高亮。
  return (
    <ChatHeavyIsland
      minHeight={112}
      delayMs={180}
      eager={conversationOpening || streaming}
      fallback={(
        <figure className="not-prose relative my-3 overflow-hidden rounded-lg border border-[var(--border-input)] bg-[var(--bg-input)] text-neutral-950 shadow-sm dark:text-neutral-100">
          <pre className="custom-scrollbar m-0 max-w-full overflow-x-auto bg-transparent px-4 pb-4 pt-10 text-[13px] leading-6 text-neutral-900 dark:text-neutral-100">
            <code className="font-mono">{normalizeCodeBlockText(code)}</code>
          </pre>
        </figure>
      )}
    >
      <CodeBlock code={code} language={language} />
    </ChatHeavyIsland>
  )
}


let mermaidRenderCounter = 0

// 已渲染 mermaid SVG 的缓存：键 = 主题 + 源码。虚拟列表会卸载屏外的消息气泡，
// 往回翻时图会重新挂载；若每次都重新 import+parse+render，会出现 spinner(小)→大SVG 的高度
// 突变，导致 virtualizer 纠正滚动 → 抽搐/闪烁。缓存后命中即同步拿到完整 SVG，挂载时高度即确定，
// 消除回滚 jank。用外部 Map 而非 useMemo（React 可能在内存压力下丢弃 useMemo 缓存）。
const mermaidSvgCache = new Map<string, string>()
const MERMAID_SVG_CACHE_MAX = 80
function cacheMermaidSvg(key: string, svg: string) {
  if (mermaidSvgCache.has(key)) mermaidSvgCache.delete(key)
  mermaidSvgCache.set(key, svg)
  if (mermaidSvgCache.size > MERMAID_SVG_CACHE_MAX) {
    const oldest = mermaidSvgCache.keys().next().value
    if (oldest !== undefined) mermaidSvgCache.delete(oldest)
  }
}

function MermaidBlock({ code }: { code: string }) {
  const normalizedCode = useMemo(() => normalizeCodeBlockText(code), [code])
  const isDark = useDocumentDark()
  const cacheKey = `${isDark ? 'd' : 'l'}\n${normalizedCode}`
  const renderBaseId = useRef('')
  const renderSeq = useRef(0)
  const [view, setView] = useState<'diagram' | 'source'>('diagram')
  // 初始即读缓存：命中则首帧就有完整 SVG（高度确定、无 spinner、无闪烁）。
  const [svg, setSvg] = useState(() => mermaidSvgCache.get(cacheKey) ?? '')
  const [error, setError] = useState('')
  const [loading, setLoading] = useState(() => !mermaidSvgCache.has(cacheKey))
  // hooks 必须在 early return 之前：源码/错误分支也会走到下面的 eager 语义。
  const { loading: conversationOpening } = useConversationTransition()

  if (!renderBaseId.current) {
    mermaidRenderCounter += 1
    renderBaseId.current = `chat-mermaid-${mermaidRenderCounter}`
  }

  useEffect(() => {
    // 命中缓存：同步设回（处理主题/源码切换时的更新；首帧已由 useState 初始值覆盖）。无异步、无闪烁。
    const cached = mermaidSvgCache.get(cacheKey)
    if (cached) {
      setSvg(cached)
      setError('')
      setLoading(false)
      return
    }
    let cancelled = false
    let errorTimer: ReturnType<typeof setTimeout> | undefined
    renderSeq.current += 1
    const renderId = `${renderBaseId.current}-${renderSeq.current}`

    // 业界标准做法（Vercel AI 实践 / Open WebUI）：渲染前先用 mermaid.parse 校验。
    // suppressErrors=true 时非法/半截代码返回 false 而非抛错——流式中的不完整代码直接跳过
    // 渲染、不报错，语法完整时立刻 render。错误只在“代码已稳定仍解析失败”后才显示，
    // 不在流式途中报红。
    void (async () => {
      try {
        const { default: mermaid } = await import('mermaid')
        mermaid.initialize({
          startOnLoad: false,
          securityLevel: 'strict',
          theme: 'base',
          themeVariables: mermaidThemeVariables(isDark),
        })
        const valid = await mermaid.parse(normalizedCode, { suppressErrors: true })
        if (cancelled) return
        if (valid) {
          const { svg: rendered } = await mermaid.render(renderId, normalizedCode)
          if (cancelled) return
          cacheMermaidSvg(cacheKey, rendered)
          setSvg(rendered)
          setError('')
          setLoading(false)
        } else {
          // 尚不合法：可能流式未写完，也可能最终就是错的。先保持上一次结果/加载态、不报错；
          // 若 ~600ms 内代码不再变化仍不合法，视为“写完且确实有语法错”，取真实报错信息再显示。
          errorTimer = setTimeout(() => {
            void mermaid
              .parse(normalizedCode)
              .then(() => {
                if (!cancelled) setLoading(false)
              })
              .catch((err) => {
                if (cancelled) return
                setError(err instanceof Error ? err.message : String(err))
                setLoading(false)
              })
          }, 600)
        }
      } catch (err) {
        if (cancelled) return
        setError(err instanceof Error ? err.message : String(err))
        setLoading(false)
      }
    })()

    return () => {
      cancelled = true
      if (errorTimer) clearTimeout(errorTimer)
    }
  }, [cacheKey, isDark, normalizedCode])

  // 与 CodeBlock 同风格：无独立头栏，"Mermaid" 标签 + 切换按钮悬浮在右上角。
  const toggle = (
    <IconButton
      size="sm"
      onClick={() => setView((current) => (current === 'diagram' ? 'source' : 'diagram'))}
      label={view === 'diagram' ? '查看源码' : '查看图表'}
    >
      {view === 'diagram' ? <Code2 size={15} strokeWidth={2} /> : <Eye size={15} strokeWidth={2} />}
    </IconButton>
  )

  // 源码视图直接复用 CodeBlock（自带卡片 + Mermaid 标签 + 复制），切换按钮塞进它的角标行，
  // 不再套外层卡片（套了会读成「卡片里还有个卡片」）。
  if (view === 'source') {
    return <CodeBlock code={normalizedCode} language="mermaid" actions={toggle} />
  }

  if (error) {
    return (
      <>
        <div className="my-3 -mb-1 rounded-lg border border-red-100 bg-red-50 px-4 py-2 text-[12px] leading-5 text-red-600 dark:border-red-900/50 dark:bg-red-950/30 dark:text-red-300">
          Mermaid 渲染失败：{error}
        </div>
        <CodeBlock code={normalizedCode} language="mermaid" />
      </>
    )
  }

  return (

    <ChatHeavyIsland
      minHeight={112}
      eager={conversationOpening}
      fallback={<CodeBlock code={normalizedCode} language="mermaid" actions={toggle} />}
    >
      <figure
        data-chat-async-pending={loading ? 'true' : undefined}
        className="not-prose relative my-3 overflow-hidden rounded-lg border border-[var(--border-input)] bg-[var(--bg-input)] text-neutral-950 shadow-sm dark:text-neutral-100"
      >
      <div className="absolute right-1.5 top-1.5 z-10 flex items-center gap-1 rounded-md bg-[var(--bg-input)] pl-2">
        <span className="text-[12px] leading-none text-neutral-400 dark:text-neutral-500">Mermaid</span>
        {toggle}
      </div>
      {loading ? (
        <div className="flex min-h-28 items-center justify-center gap-2 px-4 py-8 text-[13px] text-neutral-400 dark:text-neutral-500">
          <Loader2 size={15} className="animate-spin" />
          正在渲染图表
        </div>
      ) : (
        <div
          className="custom-scrollbar max-w-full overflow-x-auto overflow-y-hidden [contain:content] bg-white px-4 pb-4 pt-10 dark:bg-neutral-950 [&>svg]:mx-auto [&>svg]:max-w-none"
          dangerouslySetInnerHTML={{ __html: svg }}
        />
      )}
      </figure>
    </ChatHeavyIsland>
  )
}

function htmlPreviewSrcDoc(html: string): string {
  const trimmed = html.trim()
  if (!trimmed) return html

  if (/^(?:<!doctype\s+html[^>]*>\s*)?<html[\s>]/i.test(trimmed)) {
    let repaired = trimmed
    if (/<style[\s>]/i.test(repaired) && !/<\/style>/i.test(repaired)) {
      repaired += '\n</style>'
    }
    if (/<head[\s>]/i.test(repaired) && !/<\/head>/i.test(repaired)) {
      repaired += '\n</head>'
    }
    if (!/<body[\s>]/i.test(repaired)) {
      repaired += '\n<body></body>'
    }
    if (!/<\/body>/i.test(repaired)) {
      repaired += '\n</body>'
    }
    if (!/<\/html>/i.test(repaired)) {
      repaired += '\n</html>'
    }
    return repaired
  }

  return html
}

// 流式期间 html 每来一个 delta 就变一次。srcDoc 一变 iframe 就整篇重载 —— 页面闪，
// 而且重载那一下的高度重测会让虚拟列表把视口重新拽回底部（滚不上去）。
// 对策：内容还在长的时候**根本不挂 iframe**，只显示源码；静默 SETTLE_MS 后才挂一次。
const HTML_PREVIEW_SETTLE_MS = 600

// assumeSettled=false（消息还在流式生成）时首帧不能把当前值当定稿 —— 那正是「边生成边挂 iframe」。
function useSettled(value: string, delay: number, assumeSettled: boolean): string | null {
  const [settled, setSettled] = useState<string | null>(assumeSettled ? value : null)
  useEffect(() => {
    if (value === settled) return
    const timer = setTimeout(() => setSettled(value), delay)
    return () => clearTimeout(timer)
  }, [value, settled, delay])
  return settled
}

function HtmlCodePreview({ html }: { html: string }) {
  const [view, setView] = useState<'preview' | 'source'>('preview')
  const streaming = useContext(MarkdownStreamingContext)
  const settledHtml = useSettled(html, HTML_PREVIEW_SETTLE_MS, !streaming)
  // 一旦定稿过就不再退回源码：生成中途停顿超过 SETTLE_MS 会让预览/源码来回跳。
  const readyRef = useRef(false)
  if (settledHtml === html) readyRef.current = true
  const showPreview = view === 'preview' && !streaming && readyRef.current
  const previewHtml = useMemo(
    () => htmlPreviewSrcDoc(streaming ? settledHtml ?? '' : html),
    [html, settledHtml, streaming],
  )

  const openInBrowser = () => {
    void api.openHtmlPreview(htmlPreviewSrcDoc(html)).catch((err) => {
      console.error('Failed to open HTML preview:', err)
    })
  }

  return (
    <>
      {showPreview ? (
        <ChatHeavyIsland
          minHeight={520}
          fallback={<CodeBlock code={html} language="html" />}
          eager
        >
          <div className="my-3 overflow-hidden rounded-lg border border-[var(--border-input)] bg-white dark:bg-neutral-950">
            <iframe
              title="HTML 预览"
              srcDoc={previewHtml}
              // 模型输出是不可信输入。允许脚本用于交互式预览，但绝不允许 same-origin：
              // 否则 srcDoc 可直接访问父聊天页及 Tauri 注入的 IPC 全局。
              sandbox="allow-scripts"
              referrerPolicy="no-referrer"
              className="h-[520px] w-full border-0 bg-white dark:bg-neutral-950"
            />
          </div>
        </ChatHeavyIsland>
      ) : (
        <CodeBlock code={html} language="html" />
      )}
      <div className="-mt-1 mb-2 flex justify-end gap-0.5">
        {readyRef.current ? (
          <IconButton
            size="sm"
            onClick={() => setView((current) => (current === 'preview' ? 'source' : 'preview'))}
            label={view === 'preview' ? '查看源码' : '查看预览'}
          >
            {view === 'preview' ? <Code2 size={14} strokeWidth={2} /> : <Eye size={14} strokeWidth={2} />}
          </IconButton>
        ) : null}
        <IconButton size="sm" onClick={openInBrowser} label="在浏览器打开">
          <ExternalLink size={14} strokeWidth={2} />
        </IconButton>
      </div>
    </>
  )
}

function MarkdownPre({ children }: { children?: ReactNode }) {
  // 流式中避免 ChatHeavyIsland 延迟 hydrate：fallback(112px) → 真代码块 的高度跳变
  // 会在贴底 pin 之后再撑开，整段生成内容看起来「往下闪」一下。
  const streaming = useContext(MarkdownStreamingContext)
  const child = Array.isArray(children) ? children[0] : children
  if (isValidElement<{ className?: string; children?: unknown }>(child)) {
    const languageMatch = /language-([\w-]+)/.exec(child.props.className ?? '')
    const language = normalizeCodeLanguage(languageMatch?.[1])
    const code = codeChildrenToString(child.props.children)
    if (language === 'html') {
      return <HtmlCodePreview html={code} />
    }
    if (language === 'mermaid') {
      // 流式中只显示源码：异步 mermaid.render 完成后的高度突变同样会触发底部闪动。
      if (streaming) return <CodeBlock code={code} language="mermaid" />
      return <MermaidBlock code={code} />
    }
    if (language === 'kivio-error-details') {
      return <ErrorDetails detail={code} />
    }
    if (streaming) return <CodeBlock code={code} language={language} />
    return <DeferredCodeBlock code={code} language={language} />
  }
  if (streaming) return <CodeBlock code={codeChildrenToString(children)} language="" />
  return <DeferredCodeBlock code={codeChildrenToString(children)} language="" />
}

// streamdown Components 的 pre 签名在版本间不完全一致；功能组件只消费 children。
const markdownComponents = {
  pre: MarkdownPre,

  // 表格：**每个单元格一个独立圆角块**，横竖都靠 border-spacing 的空隙分隔。
  // **没有任何边框线**，别加 border，也别给容器加外框。
  table: ({ children }) => (
    <div className="custom-scrollbar my-3 max-w-full overflow-x-auto">
      <table className="w-full min-w-[240px] border-separate [border-spacing:3px] text-[13px] leading-snug">
        {children}
      </table>
    </div>
  ),
  // `style` **必须透传**：markdown 的列对齐（`:---:` / `---:`）由 remark-gfm 转成单元格上的
  // `text-align` 内联样式。原来只接 `children`，对齐被整个丢掉 —— 声明了居中/右对齐的表格
  // 全部渲染成左对齐。内联样式优先级高于下面的 `text-left`，两者不冲突。
  th: ({ children, style }) => (
    <th
      style={style}
      className="rounded-md bg-[var(--bg-hover)] px-3 py-2 text-left font-semibold text-neutral-800 dark:text-neutral-100"
    >
      {children}
    </th>
  ),
  td: ({ children, style }) => (
    <td
      style={style}
      className="rounded-md bg-neutral-500/[0.09] px-3 py-2 align-top text-neutral-700 dark:bg-neutral-400/[0.1] dark:text-neutral-300"
    >
      {children}
    </td>
  ),
  a: ({ href, children }) => <LinkAnchor href={typeof href === 'string' ? href : ''}>{children}</LinkAnchor>,
} as Components


function LinkAnchor({
  href,
  children,
  conversationId,
}: {
  href: string
  children?: ReactNode
  /** 相对路径链接要靠它在后端解析会话工作目录；没有就只能放弃打开（但仍不导航）。 */
  conversationId?: string | null
}) {
  const decodedHref = decodeKivioInternalUrl(href)
  const isWeb = /^https?:\/\//i.test(decodedHref)
  // 这些 scheme 保留 <a> 默认行为，由系统协议处理器接走（不会导航 webview 自身）。
  const isSystemScheme = /^(mailto|tel|sms):/i.test(decodedHref)
  // 页内锚点（目录跳转）保留默认行为：它不会导航走，只是滚动。
  const isHashOnly = decodedHref.startsWith('#')
  return (
    <a
      // 不能加 target="_blank"：WRY 的 new-window 处理在 WKWebView 委托层，
      // JS preventDefault 拦不住，会和下面的 openExternal 各开一个网页（双开）。
      href={decodedHref || undefined}
      rel="noopener noreferrer"
      onClick={(event) => {
        // 除了系统 scheme 和页内锚点，**一律**掐掉默认导航。<a> 的默认行为会把 Tauri
        // webview 自己导航走，整个聊天 UI（含未落盘的会话状态）随之消失——实测点一条 CLI
        // 生成的本地文件链接，窗口直接白掉。此前这里只挡 http(s)，本地文件链接（绝对路径 /
        // 相对路径）走的正是「放行默认导航」那条路。
        if (isSystemScheme || isHashOnly) return
        event.preventDefault()
        if (isWeb) {
          void api.openExternal(decodedHref).catch((err) => console.error('openExternal failed', err))
          return
        }
        if (!decodedHref) return
        // 其余一律当本地文件交给系统默认程序。相对路径的基准由后端按会话工作目录解析
        // （与 agent 写文件的目录同一个解析器），前端不拼路径。
        void api
          .openLocalFile(decodedHref, conversationId)
          .catch((err) => console.error('openLocalFile failed', err))
      }}
    >
      {children}
    </a>
  )
}

/** 引用角标 `[n]`：点击弹出对应来源片段。KB 命中显示「文档 · 标题 · 正文」；
 *  联网来源显示「标题 · 域名 · 日期 · 摘要」，标题可点直接在浏览器打开。 */
function CitationChip({ n, hit }: { n: number; hit?: CitationView }) {
  const [open, setOpen] = useState(false)
  const [popoverPosition, setPopoverPosition] = useState<CitationPopoverPosition | null>(null)
  const triggerRef = useRef<HTMLSpanElement>(null)
  const popoverRef = useRef<HTMLSpanElement>(null)
  const web = isWebCitation(hit) ? hit : null

  useLayoutEffect(() => {
    if (!open) {
      setPopoverPosition(null)
      return
    }

    const place = () => {
      const trigger = triggerRef.current
      const popover = popoverRef.current
      if (!trigger || !popover) return
      setPopoverPosition(citationPopoverPosition(
        trigger.getBoundingClientRect(),
        popover.getBoundingClientRect(),
        { width: window.innerWidth, height: window.innerHeight },
      ))
    }

    place()
    window.addEventListener('resize', place)
    window.addEventListener('scroll', place, true)
    return () => {
      window.removeEventListener('resize', place)
      window.removeEventListener('scroll', place, true)
    }
  }, [open, hit])

  useEffect(() => {
    if (!open) return
    const onDown = (event: MouseEvent) => {
      const target = event.target as Node
      if (triggerRef.current?.contains(target) || popoverRef.current?.contains(target)) return
      setOpen(false)
    }
    const onKeyDown = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setOpen(false)
    }
    document.addEventListener('mousedown', onDown)
    document.addEventListener('keydown', onKeyDown)
    return () => {
      document.removeEventListener('mousedown', onDown)
      document.removeEventListener('keydown', onKeyDown)
    }
  }, [open])

  return (
    <>
      <span ref={triggerRef} className="inline-block align-baseline">
        <button
          type="button"
          onClick={() => setOpen((value) => !value)}
          className="mx-0.5 rounded bg-indigo-500/15 px-1 align-baseline text-[0.82em] font-medium text-indigo-500 transition hover:bg-indigo-500/25"
          aria-label={`来源 ${n}`}
          aria-expanded={open}
        >
          [{n}]
        </button>
      </span>
      {open && createPortal(
        <span
          ref={popoverRef}
          role="dialog"
          aria-label={`来源 ${n}`}
          className="fixed z-[200] block max-h-[calc(100vh-1rem)] w-80 max-w-[calc(100vw-1rem)] overflow-auto rounded-lg border border-[var(--border-input)] bg-[var(--bg-input)] p-2.5 text-left text-xs shadow-lg"
          style={{
            left: popoverPosition?.left ?? 0,
            top: popoverPosition?.top ?? 0,
            visibility: popoverPosition ? 'visible' : 'hidden',
          }}
          data-tauri-drag-region="false"
        >
          {web ? (
            <>
              <button
                type="button"
                onClick={() => {
                  void api.openExternal(web.url).catch((err) => console.error('openExternal failed', err))
                }}
                className="mb-1 flex w-full items-center gap-1 font-medium text-neutral-700 hover:underline dark:text-neutral-200"
                title={web.url}
              >
                <span className="shrink-0 rounded bg-indigo-500/15 px-1 text-indigo-500">[{n}]</span>
                <span className="min-w-0 flex-1 truncate text-left">{web.title}</span>
                <ExternalLink size={10.5} className="shrink-0 text-neutral-400 dark:text-neutral-500" />
              </button>
              <span className="mb-1 block truncate text-[10.5px] text-neutral-400 dark:text-neutral-500">
                {web.host}
                {web.publishedDate ? ` · ${web.publishedDate}` : ''}
              </span>
              {web.snippet && (
                <span className="custom-scrollbar block max-h-48 overflow-auto whitespace-pre-wrap break-words leading-relaxed text-neutral-600 dark:text-neutral-300">
                  {web.snippet}
                </span>
              )}
            </>
          ) : hit && !isWebCitation(hit) ? (
            <>
              <span className="mb-1 flex items-center gap-1 font-medium text-neutral-700 dark:text-neutral-200">
                <span className="shrink-0 rounded bg-indigo-500/15 px-1 text-indigo-500">[{n}]</span>
                <span className="truncate">
                  {hit.docName}
                  {hit.headingPath ? ` · ${hit.headingPath}` : ''}
                </span>
              </span>
              <span className="custom-scrollbar block max-h-48 overflow-auto whitespace-pre-wrap break-words leading-relaxed text-neutral-600 dark:text-neutral-300">
                {hit.text}
              </span>
            </>
          ) : (
            <span className="text-neutral-400">未找到对应来源片段</span>
          )}
        </span>,
        document.body,
      )}
    </>
  )
}

function safeDecodeURIComponent(value: string): string {
  try {
    return decodeURIComponent(value)
  } catch {
    return value
  }
}

function decodeKivioInternalUrl(value: string): string {
  const internalLink = /^https:\/\/kivio\.local\/__kivio-(file|local)\?target=(.*)$/i.exec(value)
  return internalLink ? safeDecodeURIComponent(internalLink[2]) : value
}

function artifactKey(name: string): string {
  return safeDecodeURIComponent(name)
    .trim()
    .replace(/^\.?\//, '')
    .replace(/\\/g, '/')
    .toLowerCase()
}

function artifactBasename(name: string): string {
  return artifactKey(name).split('/').filter(Boolean).pop() ?? artifactKey(name)
}

function isExternalOrAbsoluteImageSrc(src: string): boolean {
  return /^(https?:|data:|blob:|tauri:|asset:|file:|\/)/i.test(src)
}

const chatMarkdownUrlTransform: UrlTransform = (url) => {
  // LinkAnchor/MarkdownArtifactImage perform the app-level routing and artifact
  // lookup. Streamdown's default transform intentionally rejects file:/relative
  // URLs, which would turn those links into its "Blocked URL" placeholder before
  // our components get a chance to handle them.
  return url
}

function buildArtifactLookup(artifacts: ChatToolArtifact[]): Map<string, ChatToolArtifact> {
  const lookup = new Map<string, ChatToolArtifact>()
  for (const artifact of artifacts) {
    if (!artifact.name) continue
    const dataUrl = artifactDataUrl(artifact)
    const hasImage =
      dataUrl.startsWith('data:image/') ||
      Boolean((artifact.path ?? '').trim()) ||
      (artifact.mimeType ?? artifact.mime_type ?? '').toLowerCase().startsWith('image/')
    if (!hasImage) continue
    // 同 basename 多张图时保留第一张，避免后写覆盖；唯一文件名应在 MCP 侧保证
    const key = artifactKey(artifact.name)
    const base = artifactBasename(artifact.name)
    if (!lookup.has(key)) lookup.set(key, artifact)
    if (!lookup.has(base)) lookup.set(base, artifact)
  }
  return lookup
}

/** Markdown 内图片：有 path 时懒加载整图，缩略图仅作占位（重载对话后不再显示 256px 小图）。 */
function MarkdownArtifactImage({
  rawSrc,
  alt,
  artifact,
  conversationId,
  onImageClick,
}: {
  rawSrc: string
  alt: string
  artifact?: ChatToolArtifact
  conversationId?: string | null
  onImageClick?: (src: string, alt: string, name?: string) => void
}) {
  const inline = artifact ? artifactDataUrl(artifact) : ''
  const initial =
    inline ||
    (isExternalOrAbsoluteImageSrc(rawSrc) ? rawSrc : '')
  const [src, setSrc] = useState(initial)

  useEffect(() => {
    let cancelled = false
    if (artifact?.path && conversationId) {
      if (inline) setSrc(inline)
      void loadArtifactDataUrl(artifact, conversationId).then((loaded) => {
        if (!cancelled && loaded) setSrc(loaded)
      })
      return () => {
        cancelled = true
      }
    }
    if (inline) {
      setSrc(inline)
      return
    }
    if (isExternalOrAbsoluteImageSrc(rawSrc)) setSrc(rawSrc)
    return () => {
      cancelled = true
    }
  }, [artifact, conversationId, inline, rawSrc])

  if (!src) return null
  const openViewer = () => onImageClick?.(src, alt, rawSrc)
  return (
    <ChatInlineImage
      src={src}
      alt={alt}
      name={artifact?.name ?? rawSrc}
      onOpenViewer={openViewer}
      className="my-3"
    />
  )
}

const streamdownPlugins = {
  cjk,
  code,
  math: createMathPlugin({ singleDollarTextMath: true }),
  mermaid,
}
const streamdownRemarkPlugins: PluggableList = [
  ...Object.values(defaultRemarkPlugins),
  remarkBreaks,
]

const FullSettledMarkdown = memo(function FullSettledMarkdown({
  content,
  components,
  remarkPlugins,
  useCache,
  streaming,
}: {
  content: string
  components: Components
  remarkPlugins: PluggableList
  useCache: boolean
  streaming: boolean
}) {
  const normalized = useMemo(() => {
    const build = () => {
      const normalizedContent = preserveLocalMarkdownLinks(
        normalizeMarkdownForRender(normalizeLegacyErrorDetails(content)),
      )
      return { normalized: normalizedContent }
    }
    return useCache
      ? getSettledMarkdownCacheEntry(content, build).normalized
      : build().normalized
  }, [content, useCache])

  // Streamdown streaming 模式对「非前缀扩展」的整段替换可能卡住旧块（如 frame 0→frame 1）。
  // 真实流式几乎总是前缀增长；一旦不是，换 key 强制重挂，避免 DOM 停在旧正文。
  const streamEpochRef = useRef(0)
  const prevStreamContentRef = useRef(content)
  if (streaming) {
    const prev = prevStreamContentRef.current
    if (prev && content !== prev && !content.startsWith(prev)) {
      streamEpochRef.current += 1

    }
    prevStreamContentRef.current = content
  } else {
    prevStreamContentRef.current = content
  }
  const streamEpoch = streamEpochRef.current

  // 对齐 LiveAgent Markdown：
  // - 流式消息固定走 Streamdown streaming 模式（块级 memo + parseIncomplete），
  //   不要在每个 token 上整篇 static 重解析——那会放大行高抖动。
  // - isAnimating 跟「还在出字」绑定；animated 始终 false（不做字级动画）。
  // - 模式只由 streaming 上下文决定；settled 后才切 static，避免中途整树重挂。
  return (
    <Streamdown
      key={streaming ? `stream-${streamEpoch}` : 'static'}
      mode={streaming ? 'streaming' : 'static'}
      dir="auto"
      parseIncompleteMarkdown
      normalizeHtmlIndentation
      plugins={streamdownPlugins}
      remarkPlugins={remarkPlugins}
      components={components}
      shikiTheme={['github-light', 'github-dark']}
      controls={{
        code: false,
        mermaid: { copy: !streaming, download: false, fullscreen: !streaming, panZoom: !streaming },
        table: false,
      }}
      isAnimating={streaming}
      animated={false}
      urlTransform={chatMarkdownUrlTransform}
      linkSafety={{ enabled: false }}
    >
      {normalized}
    </Streamdown>
  )
})


function ChatMarkdownComponent({
  content,
  artifacts = [],
  conversationId = null,
  onImageClick,
  variant = 'default',
  citations,
}: ChatMarkdownProps) {
  const streaming = useContext(MarkdownStreamingContext)
  const remarkPlugins = useMemo<PluggableList>(() => {
    const plugins: PluggableList = [...streamdownRemarkPlugins]
    if (citations && citations.size > 0) {
      plugins.push(remarkCitations(new Set(citations.keys())))
    }
    return plugins
  }, [citations])
  const components = useMemo<Components>(() => {
    const artifactLookup = buildArtifactLookup(artifacts)
    return {
      ...markdownComponents,
      a: ({ href, children }) => {
        const url = typeof href === 'string' ? href : ''
        const cite = /^#kb-cite-(\d{1,3})$/.exec(url)
        if (cite) {
          const n = Number(cite[1])
          return <CitationChip n={n} hit={citations?.get(n)} />
        }
        return <LinkAnchor href={url} conversationId={conversationId}>{children}</LinkAnchor>
      },
      img: ({ src, alt }) => {
        const rawSrc = decodeKivioInternalUrl(typeof src === 'string' ? src : '')
        const altText = alt ?? ''
        const artifact =
          rawSrc && !isExternalOrAbsoluteImageSrc(rawSrc)
            ? artifactLookup.get(artifactKey(rawSrc)) ??
              artifactLookup.get(artifactBasename(rawSrc))
            : undefined
        return (
          <MarkdownArtifactImage
            rawSrc={rawSrc}
            alt={altText}
            artifact={artifact}
            conversationId={conversationId}
            onImageClick={onImageClick}
          />
        )
      },
    }
  }, [artifacts, conversationId, onImageClick, citations])

  return (
    <div className={markdownShellClass(variant)}>
      <MarkdownErrorBoundary fallbackText={content}>
        <FullSettledMarkdown
          content={content}
          components={components}
          remarkPlugins={remarkPlugins}
          useCache={!streaming}
          streaming={streaming}
        />
      </MarkdownErrorBoundary>
    </div>
  )
}

// memo：仅当 content / artifacts 变化时才重渲染（配合 MessageBubble 的 memo）
export const ChatMarkdown = memo(ChatMarkdownComponent)
