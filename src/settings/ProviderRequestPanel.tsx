import { useRef, useState } from 'react'
import { Plus, Trash2, ClipboardPaste, Info } from 'lucide-react'
import { Toggle, Select, Input, TextArea, SettingRow, FieldBlock } from './components'
import { Button, IconButton } from '../components/Button'
import {
  CLI_IDENTITY_BUILTIN_VERSIONS,
  effectiveUserAgent,
  HeaderImportError,
  headerIssue,
  mergeImportedHeaders,
  parseHeaderImport,
  isValidHeaderValue,
  suggestHeaderKeys,
  type HeaderIssue,
  type ProviderCustomHeader,
} from './providerRequest'
import {
  normalizeProviderApiFormat,
  promptCachingDefault,
  promptCachingSupported,
} from '../api/tauri'
import type { I18n, Lang } from './i18n'
import type { ModelProvider, ProviderRequestConfig } from '../api/tauri'

const RETENTION_OPTIONS = ['short', 'long'] as const

// 行的稳定标识。只在本组件内部用于 React key 与「动过没有」的记账，不落库。
let uidCounter = 0
const nextUid = () => (uidCounter += 1)

function issueMessage(issue: HeaderIssue, t: I18n): string {
  if (issue === 'reserved') return t.headerIssueReserved
  if (issue === 'invalid-value') return t.headerIssueInvalidValue
  return t.headerIssueInvalidKey
}

function importErrorMessage(code: string, t: I18n): string {
  if (code === 'invalid-json') return t.headerImportErrorInvalidJson
  if (code === 'unsupported-json') return t.headerImportErrorUnsupportedJson
  if (code === 'unterminated-quote') return t.headerImportErrorUnterminatedQuote
  return t.headerImportErrorEmpty
}

/**
 * 「请求配置」二级页的内容：自定义请求头 / 系统代理 / prompt 缓存 / CLI 身份。
 * 由 `ProviderDetail` 在子页面里渲染（返回按钮与标题在那边）。
 * 纯展示 + 本地编辑态，落库全走 `onUpdateProvider`。
 */
export function ProviderRequestPanel({
  provider,
  t,
  lang,
  gzipInfoOpen,
  onToggleGzipInfo,
  onUpdateProvider,
}: {
  provider: ModelProvider
  t: I18n
  lang: Lang
  gzipInfoOpen: Set<string>
  onToggleGzipInfo: (id: string) => void
  onUpdateProvider: (id: string, updates: Partial<ModelProvider>) => void
}) {
  const [importOpen, setImportOpen] = useState(false)
  const [importText, setImportText] = useState('')
  const [importError, setImportError] = useState<string | null>(null)
  const [importSummary, setImportSummary] = useState<string | null>(null)
  const config = provider.request ?? {}
  const headers = config.customHeaders ?? []

  // 行的身份用稳定 uid，不用 index。删掉中间一行、或导入时折叠了重复行，index 全会重排——
  // 「这行动过没有」「这行是不是存过盘的」跟着挪到别人身上，表现就是刚点出来的空行立刻飘红。
  const uids = useRef<string[]>([])
  if (uids.current.length !== headers.length) {
    // 行数变了：保留前缀的 uid，新增的补新 uid。删除是 filter 出新数组，前缀不变，够用。
    uids.current = headers.map((_, i) => uids.current[i] ?? `h${nextUid()}`)
  }
  const rowUid = (index: number) => uids.current[index] ?? `h${index}`

  // 联想框只在正在编辑的那一行展开。
  const [suggestRow, setSuggestRow] = useState<string | null>(null)
  const [suggestActive, setSuggestActive] = useState(0)
  // 刚点「添加」出来的空行不该立刻飘红，动过之后才校验。
  const [touchedRows, setTouchedRows] = useState<Set<string>>(new Set())
  const markTouched = (uid: string) =>
    setTouchedRows((prev) => (prev.has(uid) ? prev : new Set(prev).add(uid)))
  // 进入本页时就存在的那批行是存过盘的，一进来就该校验。
  const [initialUids] = useState(() => new Set(uids.current))

  const apiFormat = normalizeProviderApiFormat(provider.apiFormat)
  const isAnthropic = apiFormat === 'anthropic_messages'
  // Anthropic 打 cache_control 断点，OpenAI Chat / Responses 发 prompt_cache_key 路由提示。
  // Gemini 服务端隐式缓存、xAI 直接拒收 prompt_cache_key —— 两者都没有可发的字段。
  const cachingSupported = promptCachingSupported(provider.apiFormat)
  // 未显式拨过开关时按协议给默认，与 Rust 的 prompt_caching_enabled 一致。
  const cachingOn = config.promptCaching ?? promptCachingDefault(provider.apiFormat)

  const patch = (updates: Partial<ProviderRequestConfig>) =>
    onUpdateProvider(provider.id, { request: { ...config, ...updates } })

  const setHeaders = (next: ProviderCustomHeader[]) => patch({ customHeaders: next })

  const handleImport = () => {
    try {
      const result = parseHeaderImport(importText)
      const merged = mergeImportedHeaders(headers, result.headers)
      setHeaders(merged.headers)
      setImportSummary(
        t.headerImportSummary
          .replace('{added}', String(merged.added))
          .replace('{overwritten}', String(merged.overwritten))
          .replace('{skipped}', String(result.issues.length)),
      )
      setImportText('')
      setImportOpen(false)
      setImportError(null)
      // 导入会重排行 —— uid 交给下一次渲染按新长度重建。
      uids.current = []
    } catch (error) {
      // 解析失败整批不落库，用户现有的列表原样保留。
      setImportError(
        importErrorMessage(error instanceof HeaderImportError ? error.code : 'empty', t),
      )
      setImportSummary(null)
    }
  }

  // 身份预设与自定义头可能都写了 User-Agent，用户得看得见最后哪条赢。
  const ua = effectiveUserAgent(headers, config.cliIdentity ?? '', config.cliIdentityVersion ?? '')
  const rawVersion = (config.cliIdentityVersion ?? '').trim()
  const versionIssue = rawVersion !== '' && !isValidHeaderValue(rawVersion)

  const identityOptions = [
    { value: '', label: t.cliIdentityOff },
    { value: 'claude_code', label: 'Claude Code' },
    { value: 'codex', label: 'Codex' },
    { value: 'grok', label: 'Grok CLI' },
  ]

  return (
    <section className="kv-group">
      <SettingRow
        label={
          <span className="flex flex-col gap-1">
            <span className="flex items-center gap-1">
              <span>{lang === 'zh' ? '压缩请求体 (gzip)' : 'Compress request body (gzip)'}</span>
              <IconButton
                size="xs"
                label={lang === 'zh' ? '显示说明' : 'Show details'}
                onClick={() => onToggleGzipInfo(provider.id)}
              >
                <Info size={12} />
              </IconButton>
            </span>
            {gzipInfoOpen.has(provider.id) && (
              <span className="kv-row-desc mt-1 block">
                {lang === 'zh'
                  ? '个别供应商前置的 WAF 会扫描明文请求体，把工具/系统提示里的 shell 命令、文件路径等文本误判为攻击而返回 403。开启后请求体用 gzip 压缩发送（多数网关可正常解压）。若该供应商不接受 gzip 请求（如官方 DeepSeek）会返回 400，请保持关闭。'
                  : 'Some providers sit behind a WAF that scans the plaintext request body and returns 403 for shell/path text inside tool or system-prompt content. Enable to gzip the request body (most gateways accept it). Keep off for providers that reject gzip requests (e.g. official DeepSeek), which would return 400.'}
              </span>
            )}
          </span>
        }
      >
        <Toggle
          ariaLabel={lang === 'zh' ? '压缩请求体 (gzip)' : 'Compress request body (gzip)'}
          checked={provider.compressRequestBody === true}
          onChange={(v) => onUpdateProvider(provider.id, { compressRequestBody: v })}
        />
      </SettingRow>

      <SettingRow label={t.useSystemProxy} description={t.useSystemProxyHint}>
        <Toggle
          ariaLabel={t.useSystemProxy}
          checked={config.useSystemProxy !== false}
          onChange={(useSystemProxy) => patch({ useSystemProxy })}
        />
      </SettingRow>

      {/* 一直显示：藏起来会让人以为没做这个功能。不支持的协议置灰并说明原因，
              和联网搜索「内置」选项在 Chat Completions 上置灰的处理一致。 */}
      <SettingRow
        label={t.promptCaching}
        description={
          !cachingSupported
            ? t.promptCachingUnsupported
            : isAnthropic
              ? t.promptCachingHintAnthropic
              : t.promptCachingHintOpenAI
        }
      >
        <Toggle
          ariaLabel={t.promptCaching}
          checked={cachingSupported && cachingOn}
          disabled={!cachingSupported}
          onChange={(promptCaching) => patch({ promptCaching })}
        />
      </SettingRow>
      {isAnthropic && cachingOn && (
        <SettingRow label={t.promptCacheRetention}>
          <Select
            className="w-44"
            value={config.promptCacheRetention === 'long' ? 'long' : 'short'}
            onChange={(promptCacheRetention) => patch({ promptCacheRetention })}
            options={RETENTION_OPTIONS.map((value) => ({
              value,
              label: value === 'long' ? t.promptCacheLong : t.promptCacheShort,
            }))}
          />
        </SettingRow>
      )}

      {ua.value && (
        <div className="mt-1 rounded-lg bg-black/[0.03] px-3 py-2 dark:bg-white/[0.04]">
          <div className="flex items-center justify-between gap-3">
            <span className="kv-row-desc">{t.cliIdentityEffective}</span>
            <span className="kv-row-desc">
              {ua.source === 'custom' ? t.cliIdentitySourceCustom : t.cliIdentitySourcePreset}
            </span>
          </div>
          <div className="mt-0.5 break-all font-mono text-[11px] leading-relaxed">{ua.value}</div>
        </div>
      )}

      <SettingRow label={t.cliIdentity} description={t.cliIdentityHint}>
        <Select
          className="w-40"
          value={config.cliIdentity ?? ''}
          onChange={(cliIdentity) => patch({ cliIdentity })}
          options={identityOptions}
        />
      </SettingRow>
      {config.cliIdentity ? (
        <FieldBlock label={t.cliIdentityVersion}>
          <Input
            value={config.cliIdentityVersion ?? ''}
            onChange={(cliIdentityVersion) => patch({ cliIdentityVersion })}
            placeholder={CLI_IDENTITY_BUILTIN_VERSIONS[config.cliIdentity] ?? ''}
            className={versionIssue ? '!border-red-500' : ''}
            title={versionIssue ? t.headerIssueInvalidValue : undefined}
            mono
          />
          {/* 后端会把非法版本号清空并退回内置版本，不给提示的话用户只看到自己填的东西消失。 */}
          {versionIssue && (
            <p className="mt-0.5 text-[11px] text-red-500">{t.headerIssueInvalidValue}</p>
          )}
        </FieldBlock>
      ) : null}

      <div className="mt-3 flex items-center justify-between gap-2">
        <span className="kv-row-label">{t.customHeaders}</span>
        <div className="flex shrink-0 gap-1.5">
          <Button
            size="sm"
            onClick={() => {
              setImportOpen((v) => !v)
              setImportError(null)
              setImportSummary(null)
            }}
            data-tauri-drag-region="false"
          >
            <ClipboardPaste size={11} />
            {t.importCustomHeaders}
          </Button>
          <Button
            size="sm"
            disabled={importOpen}
            onClick={() => setHeaders([...headers, { key: '', value: '' }])}
            data-tauri-drag-region="false"
          >
            <Plus size={11} />
            {t.addCustomHeader}
          </Button>
        </div>
      </div>

      {importOpen ? (
        /* 导入视图与列表互斥：解析成功后回到列表，直接看到增量导入的结果。 */
        <div className="mt-2 space-y-2">
          <TextArea
            value={importText}
            onChange={(v) => {
              setImportText(v)
              setImportError(null)
            }}
            placeholder={t.customHeaderImportPlaceholder}
            rows={5}
            mono
          />
          {importError && <p className="text-[12px] text-red-500">{importError}</p>}
          <div className="flex justify-end gap-1.5">
            <Button
              size="sm"
              onClick={() => {
                setImportOpen(false)
                setImportText('')
                setImportError(null)
              }}
              data-tauri-drag-region="false"
            >
              {t.cancelCustomHeaderImport}
            </Button>
            <Button
              size="sm"
              variant="primary"
              onClick={handleImport}
              data-tauri-drag-region="false"
            >
              {t.parseAndImportCustomHeaders}
            </Button>
          </div>
        </div>
      ) : headers.length === 0 ? (
        <p className="kv-row-desc mt-2">
          {t.noCustomHeaders} — {t.noCustomHeadersHint}
        </p>
      ) : (
        <div className="mt-2 space-y-1.5">
          {headers.map((header, index) => {
            const uid = rowUid(index)
            // 已存盘的行一进来就校验；刚点「添加」出来的空行等用户动过再说。
            const issue = headerIssue(header, touchedRows.has(uid) || initialUids.has(uid))
            const suggestions = suggestRow === uid ? suggestHeaderKeys(header.key) : []
            const listboxId = `kv-header-suggest-${uid}`
            const pickSuggestion = (suggestion: string) => {
              const next = headers.slice()
              next[index] = { ...header, key: suggestion }
              setHeaders(next)
              markTouched(uid)
              setSuggestRow(null)
            }
            return (
              <div key={uid} className="relative">
                <div className="flex items-start gap-1.5">
                  <div className="relative w-[42%] min-w-0">
                    <Input
                      value={header.key}
                      onChange={(key) => {
                        markTouched(uid)
                        const next = headers.slice()
                        next[index] = { ...header, key }
                        setHeaders(next)
                        setSuggestActive(0)
                      }}
                      onFocus={() => {
                        setSuggestRow(uid)
                        setSuggestActive(0)
                      }}
                      // 用 relatedTarget 判断焦点去哪，而不是定时关闭：定时关会让键盘用户
                      // Tab 到选项上之后下拉被卸载，Enter 落空，等于联想框键盘不可用。
                      onBlur={(e) => {
                        const next = e.relatedTarget as HTMLElement | null
                        if (next?.closest(`#${listboxId}`)) return
                        setSuggestRow(null)
                      }}
                      onKeyDown={(e) => {
                        if (suggestions.length === 0) return
                        if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
                          e.preventDefault()
                          const delta = e.key === 'ArrowDown' ? 1 : suggestions.length - 1
                          setSuggestActive((i) => (i + delta) % suggestions.length)
                        } else if (e.key === 'Enter') {
                          e.preventDefault()
                          pickSuggestion(suggestions[suggestActive] ?? suggestions[0])
                        } else if (e.key === 'Escape') {
                          setSuggestRow(null)
                        }
                      }}
                      placeholder={t.customHeaderKeyPlaceholder}
                      className={issue && issue !== 'invalid-value' ? '!border-red-500' : ''}
                      title={issue ? issueMessage(issue, t) : undefined}
                      role="combobox"
                      aria-expanded={suggestions.length > 0}
                      aria-controls={suggestions.length > 0 ? listboxId : undefined}
                      aria-autocomplete="list"
                      autoComplete="off"
                      spellCheck={false}
                      mono
                    />
                    {suggestions.length > 0 && (
                      <div
                        id={listboxId}
                        role="listbox"
                        className="absolute left-0 right-0 top-full z-10 mt-1 overflow-hidden rounded-lg bg-white shadow-lg ring-1 ring-black/10 dark:bg-neutral-800 dark:ring-white/10"
                      >
                        {suggestions.map((suggestion, i) => (
                          <button
                            key={suggestion}
                            type="button"
                            role="option"
                            aria-selected={i === suggestActive}
                            className={`block w-full px-2.5 py-1.5 text-left font-mono text-[12px] hover:bg-black/[0.05] dark:hover:bg-white/[0.07] ${
                              i === suggestActive ? 'bg-black/[0.05] dark:bg-white/[0.07]' : ''
                            }`}
                            // onBlur 先于 onClick，鼠标要用 mousedown 才点得中。
                            onMouseDown={(e) => {
                              e.preventDefault()
                              pickSuggestion(suggestion)
                            }}
                            onClick={() => pickSuggestion(suggestion)}
                            data-tauri-drag-region="false"
                          >
                            {suggestion}
                          </button>
                        ))}
                      </div>
                    )}
                  </div>
                  <Input
                    value={header.value}
                    onChange={(value) => {
                      markTouched(uid)
                      const next = headers.slice()
                      next[index] = { ...header, value }
                      setHeaders(next)
                    }}
                    placeholder={t.customHeaderValuePlaceholder}
                    className={`min-w-0 flex-1 ${issue === 'invalid-value' ? '!border-red-500' : ''}`}
                    title={issue ? issueMessage(issue, t) : undefined}
                    spellCheck={false}
                    mono
                  />
                  <IconButton
                    variant="danger"
                    size="sm"
                    label={t.removeCustomHeader}
                    title={t.removeCustomHeader}
                    onClick={() => {
                      uids.current = uids.current.filter((_, i) => i !== index)
                      setHeaders(headers.filter((_, i) => i !== index))
                    }}
                    data-tauri-drag-region="false"
                  >
                    <Trash2 size={12} />
                  </IconButton>
                </div>
                {issue && (
                  <p className="mt-0.5 text-[11px] text-red-500">{issueMessage(issue, t)}</p>
                )}
              </div>
            )
          })}
        </div>
      )}

      {importSummary && !importOpen && (
        <p className="kv-row-desc mt-2" role="status">
          {importSummary}
        </p>
      )}
    </section>
  )
}
