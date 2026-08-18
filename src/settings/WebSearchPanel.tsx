import { useCallback, useMemo, useState } from 'react'
import { AlertCircle, CheckCircle2, Eye, EyeOff, ExternalLink, Info, Loader2, Play } from 'lucide-react'
import { api, type Settings, type WebSearchMcpAuth, type WebSearchProviderId } from '../api/tauri'
import type { I18n, Lang } from './i18n'
import { Input, Select, SettingRow, SettingsGroup, TextArea, Toggle } from './components'
import { Button } from '../components/Button'

type WebSearchConfig = NonNullable<Settings['lens']['webSearch']>
/** 后端已接入的搜索源（settings 的 provider 枚举值）。 */
type ProviderId = WebSearchProviderId

type WebSearchPanelProps = {
  t: I18n
  lang: Lang
  webSearch: WebSearchConfig | undefined
  onChange: (updates: Partial<WebSearchConfig>) => void
}

const DEFAULT_WEB_SEARCH: WebSearchConfig = {
  enabled: false,
  provider: 'tavily',
  tavilyApiKey: '',
  exaApiKey: '',
  exaMcpUrl: 'https://mcp.exa.ai/mcp',
  ollamaApiKey: '',
  grokApiKey: '',
  grokModel: 'grok-4-1-fast-non-reasoning',
  grokBaseUrl: 'https://api.x.ai/v1',
  grokSystemPrompt:
    "You are a helpful search assistant. Search the web to find accurate and up-to-date information for the user's query. Provide a comprehensive answer with citations.",
  braveApiKey: '',
  braveBaseUrl: 'https://api.search.brave.com',
  serperApiKey: '',
  serperBaseUrl: 'https://google.serper.dev',
  bochaApiKey: '',
  bochaBaseUrl: 'https://api.bochaai.com',
  zhipuApiKey: '',
  zhipuBaseUrl: 'https://open.bigmodel.cn/api/paas/v4',
  tinyfishApiKey: '',
  tinyfishBaseUrl: 'https://api.search.tinyfish.ai',
  tinyfishMcpUrl: 'https://agent.tinyfish.ai/mcp',
  tinyfishMcpAuth: null,
  searxngBaseUrl: '',
  maxResults: 5,
  searchDepth: 'basic',
}

type ProviderDef = {
  id: ProviderId
  name: string
  site: string
  apiKeyUrl?: string
  /** 使用哪个密钥字段；ExaMCP / SearXNG 无需密钥则留空。 */
  keyField?:
    | 'tavilyApiKey'
    | 'exaApiKey'
    | 'ollamaApiKey'
    | 'grokApiKey'
    | 'braveApiKey'
    | 'serperApiKey'
    | 'bochaApiKey'
    | 'zhipuApiKey'
    | 'tinyfishApiKey'
  /** API Key 输入框占位符。 */
  keyPlaceholder?: string
  /** 可编辑的 API base 字段（代理/自建网关可改）；写入对应 *BaseUrl 设置。 */
  baseUrlField?:
    | 'tavilyBaseUrl'
    | 'exaBaseUrl'
    | 'ollamaBaseUrl'
    | 'braveBaseUrl'
    | 'serperBaseUrl'
    | 'bochaBaseUrl'
    | 'zhipuBaseUrl'
    | 'tinyfishBaseUrl'
    | 'searxngBaseUrl'
  /** base 输入框占位符（也是官方默认值）。 */
  baseUrlPlaceholder?: string
  /** 可编辑 MCP endpoint（写入 exaMcpUrl / tinyfishMcpUrl）。 */
  endpointField?: 'exaMcpUrl' | 'tinyfishMcpUrl'
  endpointPlaceholder?: string
  /** 地址栏下方说明：keyless MCP / 自建实例。 */
  urlHint?: 'exaMcp' | 'tinyfishMcp' | 'searxng'
  supportsDepth?: boolean
  /** 模型驱动搜索（Grok）：额外显示 型号 / 自定义网址 / 系统提示。 */
  modelBased?: boolean
  /** 本地品牌图标路径（public/），无则回退到文字标。 */
  icon?: string
}

const PROVIDERS: ProviderDef[] = [
  {
    id: 'tavily',
    name: 'Tavily',
    site: 'https://tavily.com',
    apiKeyUrl: 'https://app.tavily.com/home',
    keyField: 'tavilyApiKey',
    keyPlaceholder: 'tvly-...',
    baseUrlField: 'tavilyBaseUrl',
    baseUrlPlaceholder: 'https://api.tavily.com',
    supportsDepth: true,
    icon: '/search-icons/tavily.png',
  },
  {
    id: 'exa',
    name: 'Exa',
    site: 'https://exa.ai',
    apiKeyUrl: 'https://dashboard.exa.ai/api-keys',
    keyField: 'exaApiKey',
    keyPlaceholder: 'exa-...',
    baseUrlField: 'exaBaseUrl',
    baseUrlPlaceholder: 'https://api.exa.ai',
    icon: '/search-icons/exa.png',
  },
  {
    id: 'exa_mcp',
    name: 'ExaMCP',
    site: 'https://docs.exa.ai/reference/exa-mcp',
    // Exa MCP 无需 API Key，只需 endpoint。
    endpointField: 'exaMcpUrl',
    endpointPlaceholder: 'https://mcp.exa.ai/mcp',
    urlHint: 'exaMcp',
    icon: '/search-icons/exa.png',
  },
  {
    id: 'brave',
    name: 'Brave',
    site: 'https://brave.com/search/api/',
    apiKeyUrl: 'https://api-dashboard.search.brave.com/app/keys',
    keyField: 'braveApiKey',
    keyPlaceholder: 'BSA...',
    baseUrlField: 'braveBaseUrl',
    baseUrlPlaceholder: 'https://api.search.brave.com',
    icon: '/search-icons/brave.svg',
  },
  {
    id: 'serper',
    name: 'Serper',
    site: 'https://serper.dev',
    apiKeyUrl: 'https://serper.dev/api-key',
    keyField: 'serperApiKey',
    keyPlaceholder: 'serper key',
    baseUrlField: 'serperBaseUrl',
    baseUrlPlaceholder: 'https://google.serper.dev',
    icon: '/search-icons/serper.png',
  },
  {
    id: 'tinyfish',
    name: 'TinyFish',
    site: 'https://www.tinyfish.ai',
    apiKeyUrl: 'https://agent.tinyfish.ai/api-keys',
    keyField: 'tinyfishApiKey',
    keyPlaceholder: 'tinyfish key',
    baseUrlField: 'tinyfishBaseUrl',
    baseUrlPlaceholder: 'https://api.search.tinyfish.ai',
    icon: '/search-icons/tinyfish.png',
  },
  {
    id: 'tinyfish_mcp',
    name: 'TinyFish MCP',
    site: 'https://docs.tinyfish.ai/mcp-integration',
    endpointField: 'tinyfishMcpUrl',
    endpointPlaceholder: 'https://agent.tinyfish.ai/mcp',
    urlHint: 'tinyfishMcp',
    icon: '/search-icons/tinyfish.png',
  },
  {
    id: 'bocha',
    name: 'Bocha',
    site: 'https://open.bochaai.com',
    apiKeyUrl: 'https://open.bochaai.com',
    keyField: 'bochaApiKey',
    keyPlaceholder: 'bocha key',
    baseUrlField: 'bochaBaseUrl',
    baseUrlPlaceholder: 'https://api.bochaai.com',
    icon: '/search-icons/bocha.png',
  },
  {
    id: 'zhipu',
    name: 'Zhipu',
    site: 'https://open.bigmodel.cn',
    apiKeyUrl: 'https://open.bigmodel.cn/usercenter/apikeys',
    keyField: 'zhipuApiKey',
    keyPlaceholder: 'zhipu key',
    baseUrlField: 'zhipuBaseUrl',
    baseUrlPlaceholder: 'https://open.bigmodel.cn/api/paas/v4',
    icon: '/search-icons/zhipu.png',
  },
  {
    id: 'searxng',
    name: 'SearXNG',
    site: 'https://docs.searxng.org/dev/search_api.html',
    baseUrlField: 'searxngBaseUrl',
    baseUrlPlaceholder: 'https://searx.example',
    urlHint: 'searxng',
    icon: '/search-icons/searxng.svg',
  },
  {
    id: 'ollama',
    name: 'Ollama',
    site: 'https://ollama.com',
    apiKeyUrl: 'https://ollama.com/settings/keys',
    keyField: 'ollamaApiKey',
    keyPlaceholder: 'ollama key',
    baseUrlField: 'ollamaBaseUrl',
    baseUrlPlaceholder: 'https://ollama.com',
    icon: '/search-icons/ollama.png',
  },
  {
    id: 'grok',
    name: 'Grok',
    site: 'https://x.ai',
    apiKeyUrl: 'https://console.x.ai',
    keyField: 'grokApiKey',
    keyPlaceholder: 'xai-...',
    modelBased: true,
    icon: '/search-icons/grok.png',
  },
]

/** 品牌图标：优先本地真实 logo（public/search-icons），加载失败回退到灰底首字母。 */
function ProviderMark({ name, icon, size = 20 }: { name: string; icon?: string; size?: number }) {
  const [failed, setFailed] = useState(false)
  if (icon && !failed) {
    return (
      <img
        src={icon}
        alt=""
        width={size}
        height={size}
        draggable={false}
        onError={() => setFailed(true)}
        className="shrink-0 rounded-[6px] object-contain"
        style={{ width: size, height: size }}
      />
    )
  }
  return (
    <span
      className="grid shrink-0 place-items-center rounded-[6px] bg-zinc-100 font-semibold uppercase text-zinc-500 dark:bg-zinc-800 dark:text-zinc-300"
      style={{ width: size, height: size, fontSize: size * 0.46, lineHeight: 1 }}
    >
      {name.slice(0, 1)}
    </span>
  )
}

function hostname(url: string): string {
  return url.replace(/^https?:\/\//, '').replace(/\/.*$/, '')
}

function ApiKeyField({
  value,
  placeholder,
  onChange,
}: {
  value: string
  placeholder: string
  onChange: (value: string) => void
}) {
  const [reveal, setReveal] = useState(false)
  return (
    <div className="relative w-full">
      <Input
        type={reveal ? 'text' : 'password'}
        value={value}
        onChange={onChange}
        placeholder={placeholder}
        mono
        className="w-full pr-9"
      />
      <button
        type="button"
        onClick={() => setReveal((v) => !v)}
        className="absolute right-2 top-1/2 -translate-y-1/2 text-zinc-400 hover:text-zinc-600 dark:hover:text-zinc-200"
        data-tauri-drag-region="false"
        aria-label={reveal ? 'Hide' : 'Show'}
      >
        {reveal ? <EyeOff size={15} /> : <Eye size={15} />}
      </button>
    </div>
  )
}

function TinyfishMcpAuthRow({
  t,
  url,
  auth,
  onChange,
}: {
  t: I18n
  url: string
  auth: WebSearchMcpAuth | null | undefined
  onChange: (auth: WebSearchMcpAuth | null) => void
}) {
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const authorized = (auth?.accessToken ?? '').trim() !== ''
  const account = auth?.account?.trim()

  const authorize = useCallback(async () => {
    const endpoint = url.trim() || 'https://agent.tinyfish.ai/mcp'
    setError(null)
    setBusy(true)
    try {
      const server = await api.connectorOauthConnect({
        url: endpoint,
        name: 'TinyFish MCP',
      })
      const next = server.auth?.accessToken?.trim()
        ? server.auth
        : authFromOauthHeader(server.headers)
      if (!next) {
        setError(t.webSearchTinyfishMcpAuthFailed)
        return
      }
      onChange(next)
    } catch (err) {
      setError(String(err))
    } finally {
      setBusy(false)
    }
  }, [onChange, t.webSearchTinyfishMcpAuthFailed, url])

  return (
    <SettingRow
      label={t.connectorsAuthOauth}
      description={
        authorized
          ? (account
            ? t.webSearchTinyfishMcpAuthorizedAs.replace('{account}', account)
            : t.webSearchTinyfishMcpAuthorized)
          : t.webSearchTinyfishMcpHint
      }
    >
      <div className="flex flex-col items-end gap-1">
        <div className="flex items-center gap-2">
          {authorized ? (
            <Button
              onClick={() => {
                setError(null)
                onChange(null)
              }}
              data-tauri-drag-region="false"
            >
              {t.webSearchTinyfishMcpDisconnect}
            </Button>
          ) : (
            <Button
              variant="primary"
              disabled={busy}
              onClick={() => void authorize()}
              data-tauri-drag-region="false"
            >
              {busy ? t.webSearchTinyfishMcpAuthorizing : t.webSearchTinyfishMcpAuthorize}
            </Button>
          )}
        </div>
        {error && (
          <p className="max-w-[280px] text-right text-[12px] text-red-500">{error}</p>
        )}
      </div>
    </SettingRow>
  )
}

function authFromOauthHeader(headers: Record<string, string>): WebSearchMcpAuth | null {
  const header = Object.entries(headers).find(([key]) => key.toLowerCase() === 'authorization')?.[1] ?? ''
  const token = header.replace(/^Bearer\s+/i, '').trim()
  if (!token) return null
  return { kind: 'oauth', accessToken: token }
}

type TestState =
  | { status: 'idle' }
  | { status: 'running' }
  | { status: 'ok'; count: number; results: { title: string; url: string }[] }
  | { status: 'error'; message: string }

/** 测试搜索：用当前（可能未保存的）配置真实跑一次搜索，展示结果或错误。 */
function TestSearch({ t, config }: { t: I18n; config: WebSearchConfig }) {
  const [query, setQuery] = useState('')
  const [state, setState] = useState<TestState>({ status: 'idle' })

  const run = async () => {
    const q = query.trim()
    if (!q || state.status === 'running') return
    setState({ status: 'running' })
    try {
      const res = await api.testWebSearch(config, q)
      if (res.success) {
        const results = (res.results ?? []).map((r) => ({ title: r.title, url: r.url }))
        setState({ status: 'ok', count: results.length, results: results.slice(0, 5) })
      } else {
        setState({ status: 'error', message: res.error || 'Unknown error' })
      }
    } catch (err) {
      setState({ status: 'error', message: err instanceof Error ? err.message : String(err) })
    }
  }

  return (
    <SettingsGroup title={t.webSearchTestSection}>
      <div className="flex items-center gap-2">
        <Input
          value={query}
          onChange={setQuery}
          placeholder={t.webSearchTestPlaceholder}
          className="flex-1"
        />
        <button
          type="button"
          onClick={() => void run()}
          disabled={!query.trim() || state.status === 'running'}
          className="grid size-9 shrink-0 place-items-center rounded-lg border border-zinc-200 text-zinc-500 transition hover:bg-zinc-100 hover:text-indigo-600 disabled:opacity-40 dark:border-zinc-700 dark:hover:bg-zinc-800"
          data-tauri-drag-region="false"
          aria-label={t.webSearchTestSection}
        >
          {state.status === 'running'
            ? <Loader2 size={16} className="animate-spin" />
            : <Play size={16} />}
        </button>
      </div>

      {state.status === 'running' && (
        <p className="mt-2 flex items-center gap-1.5 text-[12px] text-zinc-400">
          <Loader2 size={12} className="animate-spin" /> {t.webSearchTesting}
        </p>
      )}
      {state.status === 'error' && (
        <p className="mt-2 flex items-start gap-1.5 text-[12px] text-red-500 dark:text-red-400">
          <AlertCircle size={13} className="mt-px shrink-0" /> <span className="break-all">{state.message}</span>
        </p>
      )}
      {state.status === 'ok' && (
        <div className="mt-2 space-y-1.5">
          <p className="flex items-center gap-1.5 text-[12px] text-emerald-600 dark:text-emerald-400">
            <CheckCircle2 size={13} className="shrink-0" />
            {state.count > 0
              ? t.webSearchTestResults.replace('{n}', String(state.count))
              : t.webSearchTestEmpty}
          </p>
          {state.results.length > 0 && (
            <ul className="space-y-1 rounded-lg border border-zinc-200 bg-zinc-50/60 p-2 dark:border-zinc-800 dark:bg-zinc-800/30">
              {state.results.map((r, i) => (
                <li key={`${r.url}-${i}`} className="min-w-0 truncate text-[12px] text-zinc-600 dark:text-zinc-300">
                  <span className="text-zinc-400">{i + 1}.</span> {r.title || r.url}
                </li>
              ))}
            </ul>
          )}
        </div>
      )}
    </SettingsGroup>
  )
}

export function WebSearchPanel({ t, lang, webSearch, onChange }: WebSearchPanelProps) {
  const config = { ...DEFAULT_WEB_SEARCH, ...(webSearch ?? {}) }
  const [selectedId, setSelectedId] = useState<ProviderId>(config.provider)

  const selected = useMemo(
    () => PROVIDERS.find((p) => p.id === selectedId) ?? PROVIDERS[0],
    [selectedId],
  )

  const isDefault = selected.id === config.provider
  const keyValue = selected.keyField ? config[selected.keyField] || '' : ''

  return (
    <div className="websearch-panel-root flex min-h-full items-stretch gap-0">
      {/* 左侧二级侧边栏：搜索服务商 */}
      <nav className="relative flex h-full min-h-full w-44 shrink-0 flex-col self-stretch pr-3">
        <div
          className="pointer-events-none absolute inset-y-0 right-0 w-px bg-zinc-200/80 dark:bg-zinc-800"
          aria-hidden
        />
        <div className="px-2 pb-1 pt-1 text-[11px] font-medium uppercase tracking-wide text-zinc-400">
          {t.webSearchApiSection}
        </div>
        <div className="space-y-0.5">
          {PROVIDERS.map((provider) => {
            const active = provider.id === selectedId
            const marked = provider.id === config.provider
            return (
              <button
                key={provider.id}
                type="button"
                onClick={() => setSelectedId(provider.id)}
                className={`flex w-full items-center gap-2.5 rounded-lg px-2 py-1.5 text-left text-sm transition ${
                  active
                    ? 'bg-indigo-50 font-medium text-indigo-700 dark:bg-indigo-950/40 dark:text-indigo-300'
                    : 'text-zinc-700 hover:bg-zinc-100 dark:text-zinc-200 dark:hover:bg-zinc-800'
                }`}
                data-tauri-drag-region="false"
              >
                <ProviderMark name={provider.name} icon={provider.icon} size={20} />
                <span className="min-w-0 flex-1 truncate">{provider.name}</span>
                {marked && (
                  <span className="shrink-0 rounded-full border border-emerald-400/60 px-1.5 py-px text-[10px] font-medium text-emerald-600 dark:text-emerald-400">
                    {t.webSearchDefaultBadge}
                  </span>
                )}
              </button>
            )
          })}
        </div>
      </nav>

      {/* 右侧内容 */}
      <div className="min-w-0 flex-1 pl-5">
        {/* Hero 头部：名称 + 外链 + 默认控制 */}
        <div className="mb-5 flex items-center gap-3 border-b border-zinc-200/70 pb-4 dark:border-zinc-800">
          <ProviderMark name={selected.name} icon={selected.icon} size={32} />
          <div className="min-w-0">
            <div className="flex items-center gap-1.5">
              <span className="text-[16px] font-semibold text-zinc-800 dark:text-zinc-100">
                {selected.name}
              </span>
              <button
                type="button"
                onClick={() => void api.openExternal(selected.site)}
                className="text-zinc-400 hover:text-indigo-500"
                data-tauri-drag-region="false"
                aria-label="Open site"
              >
                <ExternalLink size={13} />
              </button>
            </div>
            <div className="truncate text-xs text-zinc-400">{hostname(selected.site)}</div>
          </div>
          <div className="ml-auto shrink-0">
            {isDefault ? (
              <span className="inline-flex items-center gap-1 rounded-full border border-emerald-400/50 bg-emerald-50 px-3 py-1 text-xs font-medium text-emerald-600 dark:bg-emerald-950/30 dark:text-emerald-400">
                {t.webSearchDefaultBadge}
              </span>
            ) : (
              <Button
                variant="primary"
                onClick={() => onChange({ provider: selected.id })}
                data-tauri-drag-region="false"
              >
                {t.webSearchSetDefault}
              </Button>
            )}
          </div>
        </div>

        <div className="space-y-6">
          <SettingsGroup title={t.webSearchProviderSection}>
            {selected.keyField && (
              <SettingRow label={t.lensWebSearchApiKey} stack>
                <div className="w-full space-y-1">
                  <ApiKeyField
                    value={keyValue}
                    placeholder={selected.keyPlaceholder || 'API key'}
                    onChange={(value) => onChange({ [selected.keyField!]: value } as Partial<WebSearchConfig>)}
                  />
                  {selected.apiKeyUrl && (
                    <button
                      type="button"
                      onClick={() => void api.openExternal(selected.apiKeyUrl!)}
                      className="inline-flex items-center text-[12px] text-indigo-500 hover:underline dark:text-indigo-300"
                      data-tauri-drag-region="false"
                    >
                      {t.webSearchGetKey} ↗
                    </button>
                  )}
                </div>
              </SettingRow>
            )}

            {!selected.modelBased && (
              <SettingRow
                label={t.webSearchApiUrl}
                description={
                  selected.urlHint === 'searxng'
                    ? t.webSearchSearxngHint
                    : selected.urlHint === 'exaMcp'
                      ? t.webSearchExaMcpKeyless
                      : selected.urlHint === 'tinyfishMcp'
                        ? t.webSearchTinyfishMcpHint
                        : undefined
                }
                stack
              >
                {selected.endpointField ? (
                  <Input
                    value={config[selected.endpointField] || ''}
                    onChange={(value) => onChange({ [selected.endpointField!]: value } as Partial<WebSearchConfig>)}
                    placeholder={selected.endpointPlaceholder}
                    mono
                    className="w-full"
                  />
                ) : selected.baseUrlField ? (
                  <Input
                    value={config[selected.baseUrlField] ?? ''}
                    onChange={(value) => onChange({ [selected.baseUrlField!]: value } as Partial<WebSearchConfig>)}
                    placeholder={selected.baseUrlPlaceholder}
                    mono
                    className="w-full"
                  />
                ) : null}
              </SettingRow>
            )}

            {selected.urlHint === 'tinyfishMcp' && (
              <TinyfishMcpAuthRow
                t={t}
                url={config.tinyfishMcpUrl || ''}
                auth={config.tinyfishMcpAuth}
                onChange={(tinyfishMcpAuth) => onChange({ tinyfishMcpAuth })}
              />
            )}

            {selected.modelBased && (
              <>
                <SettingRow label={t.webSearchModel} stack>
                  <Input
                    value={config.grokModel || ''}
                    onChange={(value) => onChange({ grokModel: value })}
                    placeholder="grok-4-1-fast-non-reasoning"
                    mono
                    className="w-full"
                  />
                </SettingRow>
                <SettingRow label={t.webSearchCustomUrl} stack>
                  <Input
                    value={config.grokBaseUrl || ''}
                    onChange={(value) => onChange({ grokBaseUrl: value })}
                    placeholder="https://api.x.ai/v1"
                    mono
                    className="w-full"
                  />
                </SettingRow>
                <SettingRow label={t.webSearchSystemPrompt} stack>
                  <TextArea
                    value={config.grokSystemPrompt || ''}
                    onChange={(value) => onChange({ grokSystemPrompt: value })}
                    rows={4}
                  />
                </SettingRow>
              </>
            )}

            {selected.supportsDepth && (
              <SettingRow label={t.lensWebSearchDepth}>
                <Select
                  className="w-44"
                  value={config.searchDepth || 'basic'}
                  onChange={(v) => onChange({ searchDepth: v as WebSearchConfig['searchDepth'] })}
                  options={[
                    { value: 'ultra-fast', label: 'Ultra fast' },
                    { value: 'fast', label: 'Fast' },
                    { value: 'basic', label: 'Basic' },
                    { value: 'advanced', label: 'Advanced' },
                  ]}
                />
              </SettingRow>
            )}

            {selected.keyField && !isDefault && (
              <p className="flex items-center gap-1.5 px-1 pt-1 text-[12px] text-zinc-400">
                <Info size={12} /> {t.webSearchApiKeyRequired}
              </p>
            )}
          </SettingsGroup>

          {/* key 绑定当前查看的服务商：切换时重置测试状态；provider 覆盖为 selected.id，
              确保测试的是"正在查看"的服务商，而非已保存的默认服务商。 */}
          <TestSearch key={selected.id} t={t} config={{ ...config, provider: selected.id }} />

          {/* 结果数量对模型驱动搜索（Grok，返回合成答案）无意义，故隐藏。 */}
          {!selected.modelBased && (
            <SettingsGroup title={t.webSearchGeneralSection}>
              <SettingRow label={t.lensWebSearchMaxResults}>
                <Input
                  type="number"
                  min={1}
                  max={10}
                  className="w-24"
                  value={String(config.maxResults ?? 5)}
                  onChange={(value) => onChange({
                    maxResults: Math.min(10, Math.max(1, Number.parseInt(value, 10) || 1)),
                  })}
                />
              </SettingRow>
            </SettingsGroup>
          )}

          <SettingsGroup title={t.webSearchLensSection}>
            <SettingRow label={t.enabled} description={t.lensWebSearchHint}>
              <Toggle
                checked={config.enabled === true}
                onChange={(v) => onChange({ enabled: v })}
              />
            </SettingRow>
          </SettingsGroup>

          <p className="kv-row-desc px-1">
            {lang === 'zh'
              ? 'Chat 联网开关在 MCP → 内置工具。'
              : 'Chat web toggles: MCP → Built-in tools.'}
          </p>
        </div>
      </div>
    </div>
  )
}
