// 供应商「请求配置」的纯逻辑：请求头校验 / 常用头联想 / 从 JSON 或 cURL 导入。
// 与 Rust 侧 `provider_request.rs` 的校验规则保持一致（后端会再拦一遍——设置文件可被手改）。

export type ProviderCustomHeader = { key: string; value: string }

/** 由 Kivio 自己管理、不允许用户覆盖的头。 */
// 必须与 Rust 侧 `provider_request.rs::RESERVED_HEADER_KEYS` **逐条一致**。少一条的后果是
// 用户在面板里能填能存，保存后 sanitize_settings 把它静默删掉、列表里当场消失。
const RESERVED_HEADER_KEYS = new Set([
  'authorization',
  'x-api-key',
  'x-goog-api-key',
  'host',
  'content-length',
  'content-encoding',
  // 适配器已经发了 `Content-Type: application/json` 和 `Accept-Encoding: identity`，
  // reqwest 的 .header() 是 append 不是覆盖——多一条 gzip 会让 SSE 变二进制垃圾。
  'content-type',
  'accept-encoding',
  'anthropic-version',
])

// RFC 7230 token 字符集。
const HEADER_KEY_PATTERN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/
// 只允许可见 ASCII 与水平制表符：CR/LF 是 header 注入，非 ASCII 会被部分网关直接 400。
const HEADER_VALUE_PATTERN = /^[\t\x20-\x7e]*$/

export const isValidHeaderKey = (key: string) => HEADER_KEY_PATTERN.test(key)
export const isValidHeaderValue = (value: string) => HEADER_VALUE_PATTERN.test(value)
export const isReservedHeaderKey = (key: string) => RESERVED_HEADER_KEYS.has(key.toLowerCase())

export type HeaderIssue = 'invalid-key' | 'reserved' | 'invalid-value'

/** 返回这一行的问题；`submitted` 为 false 时空行不报错（用户刚点「添加」还没填）。 */
export function headerIssue(header: ProviderCustomHeader, submitted: boolean): HeaderIssue | null {
  if (!header.key && !header.value && !submitted) return null
  if (!isValidHeaderKey(header.key)) return 'invalid-key'
  if (isReservedHeaderKey(header.key)) return 'reserved'
  if (!isValidHeaderValue(header.value)) return 'invalid-value'
  return null
}

/** 常用头名，输入时做前缀联想。 */
export const HEADER_KEY_PRESETS = [
  'HTTP-Referer',
  'X-Title',
  'User-Agent',
  'X-Request-ID',
  'X-Environment',
  'X-Stainless-Lang',
  'anthropic-beta',
  'OpenAI-Organization',
  'OpenAI-Project',
]

export function suggestHeaderKeys(input: string): string[] {
  const query = input.trim().toLowerCase()
  if (!query) return HEADER_KEY_PRESETS.slice(0, 6)
  return HEADER_KEY_PRESETS.filter((key) => key.toLowerCase().includes(query)).slice(0, 6)
}

export type ImportIssueReason =
  | 'invalid-item'
  | 'unsupported-value'
  | 'invalid-key'
  | 'reserved'
  | 'invalid-value'
  | 'malformed-header'

export type ImportIssue = { key?: string; reason: ImportIssueReason }
export type ImportResult = { headers: ProviderCustomHeader[]; issues: ImportIssue[] }
export type ImportErrorCode = 'empty' | 'invalid-json' | 'unsupported-json' | 'unterminated-quote'

export class HeaderImportError extends Error {
  constructor(readonly code: ImportErrorCode) {
    super(code)
    this.name = 'HeaderImportError'
  }
}

function pushHeader(result: ImportResult, rawKey: unknown, rawValue: unknown) {
  if (typeof rawKey !== 'string') {
    result.issues.push({ reason: 'invalid-item' })
    return
  }
  const key = rawKey.trim()
  if (typeof rawValue !== 'string' && typeof rawValue !== 'number' && typeof rawValue !== 'boolean') {
    result.issues.push({ key: key || undefined, reason: 'unsupported-value' })
    return
  }
  if (!isValidHeaderKey(key)) {
    result.issues.push({ key: key || undefined, reason: 'invalid-key' })
    return
  }
  if (isReservedHeaderKey(key)) {
    result.issues.push({ key, reason: 'reserved' })
    return
  }
  const value = String(rawValue)
  if (!isValidHeaderValue(value)) {
    result.issues.push({ key, reason: 'invalid-value' })
    return
  }
  // 同名覆盖：一个头只能有一条，否则上游看到哪条全凭运气。
  const existing = result.headers.findIndex((h) => h.key.toLowerCase() === key.toLowerCase())
  if (existing >= 0) result.headers[existing] = { key, value }
  else result.headers.push({ key, value })
}

/** 把 `Name: value` 一行拆成键值。 */
function pushRawHeaderLine(result: ImportResult, raw: string) {
  const colon = raw.indexOf(':')
  if (colon <= 0) {
    result.issues.push({ reason: 'malformed-header' })
    return
  }
  pushHeader(result, raw.slice(0, colon), raw.slice(colon + 1).trim())
}

/** cURL 命令分词：处理单/双引号、双引号内的转义、以及 `\` / `^` / 反引号的行尾续行。 */
function tokenizeCurl(command: string): string[] {
  const normalized = command.replace(/[\\`^][ \t]*\r?\n/g, ' ')
  const tokens: string[] = []
  let token = ''
  let quote: "'" | '"' | null = null
  let started = false

  for (let i = 0; i < normalized.length; i += 1) {
    const ch = normalized[i]
    if (quote) {
      if (ch === quote) {
        quote = null
        continue
      }
      if (ch === '\\' && quote === '"' && (normalized[i + 1] === '"' || normalized[i + 1] === '\\')) {
        token += normalized[i + 1]
        i += 1
        continue
      }
      token += ch
      continue
    }
    if (ch === "'" || ch === '"') {
      quote = ch
      started = true
      continue
    }
    // 引号外的转义：Windows 的「复制为 cURL (bash)」会产出 \' 这类序列。
    if (ch === '\\' && i + 1 < normalized.length && /[\s'"]/.test(normalized[i + 1])) {
      token += normalized[i + 1]
      started = true
      i += 1
      continue
    }
    if (/\s/.test(ch)) {
      if (started) {
        tokens.push(token)
        token = ''
        started = false
      }
      continue
    }
    token += ch
    started = true
  }
  if (quote) throw new HeaderImportError('unterminated-quote')
  if (started) tokens.push(token)
  return tokens
}

/**
 * 从粘贴的文本导入请求头。支持三种输入：
 * - JSON 对象 `{"X-Title": "kivio"}`
 * - JSON 数组 `[{"key":"X-Title","value":"kivio"}]`
 * - cURL 命令（读 `-H` / `--header`）
 *
 * 解析失败抛 `HeaderImportError`，调用方据此保持现有列表不动。
 */
export function parseHeaderImport(text: string): ImportResult {
  const trimmed = text.trim()
  if (!trimmed) throw new HeaderImportError('empty')
  const result: ImportResult = { headers: [], issues: [] }

  if (trimmed.startsWith('{') || trimmed.startsWith('[')) {
    let parsed: unknown
    try {
      parsed = JSON.parse(trimmed)
    } catch {
      throw new HeaderImportError('invalid-json')
    }
    if (Array.isArray(parsed)) {
      for (const item of parsed) {
        if (item && typeof item === 'object') {
          const record = item as Record<string, unknown>
          pushHeader(result, record.key ?? record.name, record.value)
        } else if (typeof item === 'string') {
          pushRawHeaderLine(result, item)
        } else {
          result.issues.push({ reason: 'invalid-item' })
        }
      }
      return result
    }
    if (parsed && typeof parsed === 'object') {
      for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
        pushHeader(result, key, value)
      }
      return result
    }
    throw new HeaderImportError('unsupported-json')
  }

  const tokens = tokenizeCurl(trimmed)
  let sawHeaderFlag = false
  for (let i = 0; i < tokens.length; i += 1) {
    const token = tokens[i]
    let raw: string | undefined
    if (token === '-H' || token === '--header') {
      raw = tokens[i + 1]
      i += 1
    } else if (token.startsWith('--header=')) {
      raw = token.slice('--header='.length)
    } else {
      continue
    }
    sawHeaderFlag = true
    if (raw === undefined) {
      result.issues.push({ reason: 'malformed-header' })
      continue
    }
    pushRawHeaderLine(result, raw)
  }
  // 不是 cURL、也没有 -H：多半是直接粘的 `Name: value` 若干行。
  // 但明确以 curl 开头的命令就别乱猜了——把 `-d '{...}'` 当请求头解析只会报一堆无用 issue。
  if (!sawHeaderFlag && !/^curl\b/i.test(trimmed)) {
    for (const line of trimmed.split(/\r?\n/)) {
      if (line.trim()) pushRawHeaderLine(result, line.trim())
    }
  }
  return result
}

/** 把导入结果并进现有列表（同名覆盖），返回新列表与「新增 / 覆盖」计数。 */
export function mergeImportedHeaders(
  current: ProviderCustomHeader[],
  imported: ProviderCustomHeader[],
): { headers: ProviderCustomHeader[]; added: number; overwritten: number } {
  let headers = current.slice()
  let added = 0
  let overwritten = 0
  for (const header of imported) {
    const expected = header.key.toLowerCase()
    const index = headers.findIndex((h) => h.key.toLowerCase() === expected)
    if (index < 0) {
      headers.push(header)
      added += 1
      continue
    }
    overwritten += 1
    // 列表里本来可能有两行同名（UI 允许手动添重），收成一行，
    // 否则导入后仍留着一条不知道哪条生效的僵尸。
    headers = headers.filter((h, i) => i === index || h.key.toLowerCase() !== expected)
    headers[index] = header
  }
  return { headers, added, overwritten }
}

export const CLI_IDENTITY_BUILTIN_VERSIONS: Record<string, string> = {
  claude_code: '2.1.71',
  codex: '0.72.0',
  grok: '0.2.110',
}

/**
 * CLI 身份预设的 User-Agent；与 Rust 侧 `provider_request::identity_pairs` 保持一致。
 *
 * 版本号要过头值校验后才用 —— Rust 的 `identity_version` 非法就退回内置版本。不校验的话
 * 面板上那块「实际发送的 User-Agent」在用户填了非 ASCII / 带换行的版本号时显示的是假值。
 */
export function identityUserAgent(identity: string, version: string): string | null {
  const configured = version.trim()
  const v = (isValidHeaderValue(configured) ? configured : '') || CLI_IDENTITY_BUILTIN_VERSIONS[identity]
  if (!v) return null
  if (identity === 'claude_code') return `claude-cli/${v} (external, cli)`
  if (identity === 'codex') return `codex_cli_rs/${v} (Ubuntu 24.4.0; x86_64) WindowsTerminal`
  if (identity === 'grok') return `grok-shell/${v} (linux; x86_64)`
  return null
}

/** 最终真正会发出去的 User-Agent，以及它是哪儿来的。自定义头优先于身份预设。 */
export function effectiveUserAgent(
  headers: ProviderCustomHeader[],
  identity: string,
  version: string,
): { value: string | null; source: 'custom' | 'preset' | 'none' } {
  // 从后往前找：同名多行时最后一行生效（与 Rust 侧的覆盖顺序一致）。
  for (let i = headers.length - 1; i >= 0; i -= 1) {
    const header = headers[i]
    if (header.key.toLowerCase() !== 'user-agent') continue
    if (!isValidHeaderKey(header.key) || !isValidHeaderValue(header.value)) continue
    return { value: header.value, source: 'custom' }
  }
  const preset = identityUserAgent(identity, version)
  return preset ? { value: preset, source: 'preset' } : { value: null, source: 'none' }
}
