import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import {
  ChevronDown,
  ChevronRight,
  Download,
  ExternalLink,
  Globe,
  Pencil,
  Plus,
  RefreshCw,
  Search,
  Terminal,
  Trash2,
  Workflow,
} from 'lucide-react'
import { open } from '@tauri-apps/plugin-dialog'
import { AgentIcon } from '../chat/AgentIcon'
import {
  chatApi,
  onExternalAgentsUpdated,
  onExternalCliInstallLog,
  type CcSwitchProvider,
  type DetectedExternalAgent,
  type DshOfficialCredential,
  type ExternalCliInstallInfo,
} from '../chat/api'
import type { NativeProviderSummary } from '../chat/types'
import { Input, Toggle } from './components'
import { i18n, type Lang } from './i18n'
import { Button, IconButton } from '../components/Button'
import { CliProviderModal } from './CliProviderModal'
import { DshPluginsSettings } from './DshPluginsSettings'
import { CcSwitchImportModal } from './CcSwitchImportModal'
import type {
  ExternalCliAgentConfig,
  ExternalCliProvider,
  Settings as SettingsData,
} from '../api/tauri'

const EMPTY_CONFIG: ExternalCliAgentConfig = {}
const DSH_OFFICIAL_PROVIDER_ID = 'deepseek-official'
const DSH_OFFICIAL_KEY_URL = 'https://platform.deepseek.com/api_keys'

function withOfficialDshProviders(
  agentId: string,
  natives: NativeProviderSummary[],
): NativeProviderSummary[] {
  if (agentId !== 'dsh') return natives
  if (natives.some((provider) => provider.id === DSH_OFFICIAL_PROVIDER_ID)) return natives
  return [
    {
      id: DSH_OFFICIAL_PROVIDER_ID,
      name: 'DeepSeek',
      modelCount: 2,
      isDefault: natives.every((provider) => !provider.isDefault),
    },
    ...natives,
  ]
}

function IconBox({ id, size }: { id: string; size: number }) {
  return (
    <span className="kv-cli-iconbox" style={{ width: size, height: size }}>
      <AgentIcon id={id} size={Math.round(size * 0.62)} />
    </span>
  )
}

/** label + 说明在左、操作在右的一行。就是 ccgui 那张卡片里的行。 */
function Row({
  label,
  desc,
  children,
}: {
  label: string
  desc?: string | null
  children: React.ReactNode
}) {
  return (
    <div className="kv-row">
      <div className="kv-row-text">
        <div className="kv-row-label">{label}</div>
        {desc && <p className="kv-row-desc">{desc}</p>}
      </div>
      <div className="kv-row-control">{children}</div>
    </div>
  )
}

/** 比较 CLI 常见的数字版本号；缺段按 0 处理，解析失败时交回后端结果兜底。 */
function isVersionNewer(localVersion: string, latestVersion: string): boolean | null {
  const parse = (value: string) => {
    const match = value.match(/\d+(?:\.\d+){2,}/)?.[0]
    if (!match) return null
    const parts = match.split('.').map(Number)
    return parts.every(Number.isSafeInteger) ? parts : null
  }
  const local = parse(localVersion)
  const latest = parse(latestVersion)
  if (!local || !latest) return null
  const length = Math.max(local.length, latest.length)
  for (let index = 0; index < length; index += 1) {
    const currentPart = local[index] ?? 0
    const latestPart = latest[index] ?? 0
    if (latestPart !== currentPart) return latestPart > currentPart
  }
  return false
}

interface ExternalAgentsSettingsProps {
  lang: Lang
  settings: SettingsData
  updateChat: (updates: Partial<NonNullable<SettingsData['chat']>>) => void
}

export function ExternalAgentsSettings({ lang, settings, updateChat }: ExternalAgentsSettingsProps) {
  const t = i18n[lang]
  const [agents, setAgents] = useState<DetectedExternalAgent[]>([])
  const [scanning, setScanning] = useState(false)
  const [selectedId, setSelectedId] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [collapsed, setCollapsed] = useState<Record<string, boolean>>({})

  const overrides = useMemo(
    () => settings.chat?.externalCliAgents ?? {},
    [settings.chat?.externalCliAgents],
  )

  const loadAgents = useCallback(async (force = false) => {
    setScanning(true)
    try {
      const list = await chatApi.detectExternalAgents(force)
      setAgents(list)
      setSelectedId((prev) => prev ?? list.find((a) => a.available)?.id ?? list[0]?.id ?? null)
    } catch (err) {
      console.error('[ExternalAgentsSettings] detect failed:', err)
      setAgents([])
    } finally {
      setScanning(false)
    }
  }, [])

  useEffect(() => {
    void loadAgents()
  }, [loadAgents])

  // 首屏拿的是落盘快照，后台重探完会推一条新列表过来。
  useEffect(() => {
    let unlisten: (() => void) | null = null
    let cancelled = false
    void onExternalAgentsUpdated((list) => setAgents(list)).then((un) => {
      if (cancelled) un()
      else unlisten = un
    })
    return () => {
      cancelled = true
      unlisten?.()
    }
  }, [])

  // 三组：已安装 / 已停用 / 未安装。停用单独成组，免得用户在已安装里找不到自己关掉的那个。
  const groups = useMemo(() => {
    const needle = query.trim().toLowerCase()
    const installed: DetectedExternalAgent[] = []
    const disabled: DetectedExternalAgent[] = []
    const missing: DetectedExternalAgent[] = []
    for (const agent of agents) {
      if (needle && !agent.name.toLowerCase().includes(needle) && !agent.id.includes(needle)) continue
      const off = overrides[agent.id]?.disabled ?? agent.disabled ?? false
      if (off) disabled.push(agent)
      else if (agent.available) installed.push(agent)
      else missing.push(agent)
    }
    return [
      { key: 'installed', label: t.externalAgentsInstalled, items: installed },
      { key: 'disabled', label: t.externalAgentsDisabledGroup, items: disabled },
      { key: 'missing', label: t.externalAgentsNotInstalled, items: missing },
    ].filter((group) => group.items.length > 0)
  }, [agents, overrides, query, t])

  const selected = agents.find((agent) => agent.id === selectedId) ?? null

  const patchOverride = useCallback(
    (agentId: string, patch: Partial<ExternalCliAgentConfig>) => {
      const current = settings.chat?.externalCliAgents ?? {}
      updateChat({
        externalCliAgents: {
          ...current,
          [agentId]: { ...(current[agentId] ?? EMPTY_CONFIG), ...patch },
        },
      })
    },
    [settings.chat?.externalCliAgents, updateChat],
  )

  return (
    <div className="kv-providers-root">
      <div className="kv-providers">
        <div className="kv-provider-list kv-split-list">
          {/* 搜索框 + 重新扫描。扫描按钮做成搜索行右边的一个图标：它原来是列表下方一个整
              宽按钮，自带边框和分隔线，在左栏里显得像另一块面板。 */}
          <div className="kv-cli-search">
            <div className="relative min-w-0 flex-1">
              <Search
                size={12}
                className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-[var(--text-faint)]"
              />
              <Input
                value={query}
                onChange={setQuery}
                placeholder={t.externalAgentsSearch}
                className="!pl-6 !text-[11.5px]"
              />
            </div>
            <IconButton
              size="sm"
              label={t.externalAgentsRescan}
              onClick={() => void loadAgents(true)}
              disabled={scanning}
            >
              <RefreshCw size={13} className={scanning ? 'animate-spin' : ''} />
            </IconButton>
          </div>
          <div className="kv-provider-list-items custom-scrollbar !flex-none">
            {groups.map((group) => {
              const isCollapsed = collapsed[group.key] ?? false
              return (
                <div key={group.key}>
                  <button
                    type="button"
                    className="kv-cli-group-head"
                    onClick={() => setCollapsed((prev) => ({ ...prev, [group.key]: !isCollapsed }))}
                    data-tauri-drag-region="false"
                  >
                    {isCollapsed ? <ChevronRight /> : <ChevronDown />}
                    {group.label}
                  </button>
                  {!isCollapsed &&
                    group.items.map((agent) => {
                      const off = overrides[agent.id]?.disabled ?? agent.disabled ?? false
                      return (
                        <button
                          key={agent.id}
                          type="button"
                          onClick={() => setSelectedId(agent.id)}
                          className={`kv-provider-item ${agent.id === selectedId ? 'active' : ''}`}
                          data-tauri-drag-region="false"
                        >
                          <span className="kv-provider-item-select">
                            <IconBox id={agent.id} size={22} />
                            <span className="kv-provider-name">{agent.name}</span>
                          </span>
                          <span
                            className={`kv-provider-dot ${
                              off || !agent.available ? 'off' : 'on'
                            }`}
                          />
                        </button>
                      )
                    })}
                </div>
              )
            })}
            {groups.length === 0 && (
              <p className="kv-row-desc px-1 py-2">
                {scanning ? t.externalAgentsRescanning : t.externalAgentsNoMatch}
              </p>
            )}
          </div>
        </div>

        <div className="kv-provider-detail">
          {selected ? (
            <AgentDetail
              key={selected.id}
              agent={selected}
              lang={lang}
              config={overrides[selected.id] ?? EMPTY_CONFIG}
              onPatch={(patch) => patchOverride(selected.id, patch)}
              reloadAgents={loadAgents}
            />
          ) : (
            <p className="kv-row-desc py-8 text-center">
              {scanning ? t.externalAgentsRescanning : t.externalAgentsSelectHint}
            </p>
          )}
        </div>
      </div>
    </div>
  )
}

function AgentDetail({
  agent,
  lang,
  config,
  onPatch,
  reloadAgents,
}: {
  agent: DetectedExternalAgent
  lang: Lang
  config: ExternalCliAgentConfig
  onPatch: (patch: Partial<ExternalCliAgentConfig>) => void
  reloadAgents: (force?: boolean) => Promise<void>
}) {
  const t = i18n[lang]
  const disabled = config.disabled ?? agent.disabled ?? false
  const customModels = config.customModels ?? []
  const [modelsExpanded, setModelsExpanded] = useState(false)
  const install = useInstall(agent.id, reloadAgents)

  const [probedModels, setProbedModels] = useState<DetectedExternalAgent['models']>([])
  const [showPlugins, setShowPlugins] = useState(false)
  useEffect(() => setShowPlugins(false), [agent.id])
  useEffect(() => {
    if (!agent.available) {
      setProbedModels([])
      return
    }
    let cancelled = false
    void chatApi
      .detectExternalAgentModels(agent.id, null, false)
      .then(({ models }) => {
        if (!cancelled) setProbedModels(models)
      })
      .catch(() => {})
    return () => {
      cancelled = true
    }
  }, [agent.id, agent.available])

  const pickBinary = async () => {
    const picked = await open({ multiple: false, directory: false })
    if (typeof picked === 'string') onPatch({ path: picked })
  }

  const { info, checking, running, log, result, logRef, refresh, runInstall } = install
  const localVersion = info?.localVersion ?? agent.version ?? null
  const versionComparison =
    localVersion && info?.latestVersion
      ? isVersionNewer(localVersion, info.latestVersion)
      : null
  const updateAvailable = versionComparison ?? info?.updateAvailable ?? false
  const versionStatus = !agent.available
    ? null
    : checking
      ? { label: t.externalAgentsCheckingVersion, tone: '' }
      : !info
        ? { label: t.externalAgentsVersionCheckFailed, tone: 'warn' }
        : !localVersion || !info.latestVersion
          ? { label: t.externalAgentsLatestVersionUnknown, tone: 'warn' }
          : updateAvailable
            ? {
                label: (info.command
                  ? t.externalAgentsUpdateAvailable
                  : t.externalAgentsManualUpdateAvailable
                ).replace('{version}', info.latestVersion),
                tone: 'warn',
              }
            : { label: t.externalAgentsUpToDate, tone: 'ok' }
  const showInstallAction = Boolean(
    info?.command && !checking && (!agent.available || updateAvailable),
  )

  if (showPlugins && agent.id === 'dsh') {
    return <DshPluginsSettings lang={lang} onBack={() => setShowPlugins(false)} />
  }

  return (
    <>
      <div className="kv-cli-head">
        <IconBox id={agent.id} size={32} />
        <span className="kv-cli-head-main">
          <span className="kv-cli-head-title">{agent.name}</span>
          {info && (
            <a
              className="kv-cli-docs"
              href={info.docsUrl}
              target="_blank"
              rel="noreferrer"
              title={t.externalAgentsDocs}
              data-tauri-drag-region="false"
            >
              <ExternalLink size={12} />
              {t.externalAgentsDocs}
            </a>
          )}
        </span>
        <span className="kv-cli-pill">{localVersion ?? t.externalAgentsNotInstalled}</span>
        {versionStatus && (
          <span className={`kv-tag ${versionStatus.tone}`}>{versionStatus.label}</span>
        )}
        {showInstallAction && (
          <Button size="sm" variant="primary" onClick={() => void runInstall()} disabled={running}>
            {running
              ? t.externalAgentsInstalling
              : agent.available
                ? t.externalAgentsUpdate
                : t.externalAgentsInstall}
          </Button>
        )}
        <IconButton
          size="sm"
          label={t.externalAgentsCheckUpdate}
          onClick={() => void refresh()}
          disabled={checking}
        >
          <RefreshCw size={13} className={checking ? 'animate-spin' : ''} />
        </IconButton>
      </div>
      {agent.id === 'dsh' && !agent.available && (
        <p className="kv-row-desc">{t.externalAgentsDshInstallHint}</p>
      )}

      {agent.id === 'dsh' && <DshOfficialKeyCard lang={lang} />}

      {agent.id === 'dsh' && (
        <button
          type="button"
          onClick={() => setShowPlugins(true)}
          className="kv-cli-card kv-dsh-plugins-entry"
          data-tauri-drag-region="false"
        >
          <span className="kv-dsh-plugins-entry-icons" aria-hidden="true">
            <span><Terminal size={13} /></span>
            <span><Workflow size={13} /></span>
            <span><Globe size={13} /></span>
          </span>
          <span className="kv-row-text">
            <span className="kv-row-label">{t.externalAgentsDshPlugins}</span>
            <span className="kv-row-desc">{t.externalAgentsDshPluginsHint}</span>
          </span>
          <span className="kv-subpage-entry-go">
            {lang === 'zh' ? '配置' : 'Edit'}
            <ChevronRight size={14} aria-hidden="true" />
          </span>
        </button>
      )}

      {(log.length > 0 || result) && (
        <div className="kv-cli-card">
          <div className="kv-row-stack">
            <div className="kv-row-text">
              <div className="kv-row-label">{t.externalAgentsInstallLog}</div>
              {result && (
                <p className="kv-row-desc">
                  {result === 'ok' ? t.externalAgentsInstallDone : t.externalAgentsInstallFailed}
                </p>
              )}
            </div>
            {log.length > 0 && (
              <pre ref={logRef} className="kv-cli-log">
                {log.join('\n')}
              </pre>
            )}
          </div>
        </div>
      )}

      <div className="kv-cli-card">
        <Row label={t.externalAgentsEnable} desc={t.externalAgentsEnableDesc}>
          <Toggle checked={!disabled} onChange={(on) => onPatch({ disabled: !on })} />
        </Row>
        <Row
          label={t.externalAgentsBinaryPath}
          desc={config.path || agent.path || t.externalAgentsUsePath}
        >
          {config.path && (
            <Button size="sm" variant="ghost" onClick={() => onPatch({ path: '' })}>
              {t.externalAgentsClear}
            </Button>
          )}
          <Button size="sm" onClick={() => void pickBinary()}>
            {t.externalAgentsConfigPath}
          </Button>
        </Row>
        <Row
          label={t.externalAgentsModelsSection}
          desc={t.externalAgentsModelsSummary
            .replace('{probed}', String(probedModels.length))
            .replace('{custom}', String(customModels.length))}
        >
          <Button size="sm" onClick={() => setModelsExpanded((value) => !value)}>
            {modelsExpanded ? t.externalAgentsCollapse : t.externalAgentsManageModels}
          </Button>
        </Row>
        {modelsExpanded && (
          <div className="kv-cli-sub">
            <p className="kv-row-desc">{t.externalAgentsModelsDesc}</p>
            {probedModels.length > 0 && (
              <p className="kv-row-desc break-all">
                {t.externalAgentsModelsProbed.replace('{count}', String(probedModels.length))}:{' '}
                {probedModels.map((model) => model.id).join(', ')}
              </p>
            )}
            {customModels.map((model, idx) => (
              <div key={idx} className="flex items-center gap-2">
                <Input
                  value={model.id}
                  onChange={(value) =>
                    onPatch({
                      customModels: customModels.map((m, i) =>
                        i === idx ? { ...m, id: value } : m,
                      ),
                    })
                  }
                  placeholder={t.externalAgentsModelId}
                  mono
                />
                <Input
                  value={model.label}
                  onChange={(value) =>
                    onPatch({
                      customModels: customModels.map((m, i) =>
                        i === idx ? { ...m, label: value } : m,
                      ),
                    })
                  }
                  placeholder={t.externalAgentsModelLabel}
                />
                <IconButton
                  size="sm"
                  label={t.externalAgentsRemove}
                  onClick={() => onPatch({ customModels: customModels.filter((_, i) => i !== idx) })}
                >
                  <Trash2 size={13} />
                </IconButton>
              </div>
            ))}
            <div>
              <Button
                size="sm"
                onClick={() => onPatch({ customModels: [...customModels, { id: '', label: '' }] })}
              >
                <Plus size={12} />
                {t.externalAgentsModelAdd}
              </Button>
            </div>
          </div>
        )}
      </div>

      {agent.id === 'cursor-agent' && agent.available && (
        <p className="kv-row-desc mb-3">{t.externalAgentsCursorToolLimit}</p>
      )}

      <ProviderSection
        lang={lang}
        agentId={agent.id}
        agentName={agent.name}
        providers={config.providers ?? []}
        nativeProviders={agent.nativeProviders ?? []}
        current={config.currentProvider ?? ''}
        onPatch={onPatch}
      />
    </>
  )
}

function DshOfficialKeyCard({ lang }: { lang: Lang }) {
  const t = i18n[lang]
  const [status, setStatus] = useState<DshOfficialCredential | null>(null)
  const [apiKey, setApiKey] = useState('')
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    void chatApi
      .dshOfficialCredentialStatus()
      .then((next) => {
        if (!cancelled) setStatus(next)
      })
      .catch(() => {
        if (!cancelled) setStatus({ configured: false, writable: true })
      })
    return () => {
      cancelled = true
    }
  }, [])

  const writable = status?.writable ?? true
  const save = async () => {
    const key = apiKey.trim()
    if (!key || !writable) return
    setSaving(true)
    setError(null)
    try {
      setStatus(await chatApi.dshOfficialCredentialSave(key))
      setApiKey('')
    } catch (err) {
      setError(String(err))
    } finally {
      setSaving(false)
    }
  }

  return (
    <div className="kv-cli-card kv-dsh-official-key">
      <div className="kv-dsh-official-key-head">
        <label htmlFor="dsh-official-key" className="kv-row-label">
          {t.externalAgentsDshOfficialKey}
        </label>
        {status && !status.configured && writable && (
          <span className="kv-tag warn">{t.externalAgentsDshOfficialKeyUnsetTag}</span>
        )}
        {status?.configured && (
          <span className="kv-tag ok">{t.externalAgentsDshOfficialKeySetTag}</span>
        )}
        <a
          className="kv-cli-docs"
          href={DSH_OFFICIAL_KEY_URL}
          target="_blank"
          rel="noreferrer"
          data-tauri-drag-region="false"
        >
          <ExternalLink size={12} />
          {t.externalAgentsDshOfficialKeyGet}
        </a>
      </div>
      <Input
        id="dsh-official-key"
        type="password"
        value={apiKey}
        onChange={setApiKey}
        placeholder={
          status?.configured
            ? t.externalAgentsDshOfficialKeySet
            : t.externalAgentsDshOfficialKeyUnset
        }
        mono
        disabled={!writable}
        onKeyDown={(event) => {
          if (event.key === 'Enter') void save()
        }}
      />
      <p className="kv-row-desc">
        {writable ? t.externalAgentsDshOfficialKeyHint : t.externalAgentsDshOfficialKeyLocked}
      </p>
      {error && <p className="kv-row-desc kv-dsh-field-error">{error}</p>}
      {writable && (
        <div className="kv-dsh-official-key-actions">
          <Button
            size="sm"
            variant="primary"
            disabled={saving || !apiKey.trim()}
            onClick={() => void save()}
          >
            {saving ? t.externalAgentsDshOfficialKeySaving : t.externalAgentsDshOfficialKeySave}
          </Button>
        </div>
      )}
    </div>
  )
}

function NativeProviderRow({
  lang,
  provider,
  current,
  onUseCliConfig,
}: {
  lang: Lang
  provider: NativeProviderSummary
  current: string
  onUseCliConfig: () => void
}) {
  const t = i18n[lang]
  const official = provider.id === DSH_OFFICIAL_PROVIDER_ID
  const usingCliConfig = !current
  const inUse = usingCliConfig && provider.isDefault
  const models =
    provider.modelCount > 0
      ? t.externalAgentsNativeProviderModels.replace('{count}', String(provider.modelCount))
      : null
  const subtitle = official
    ? [t.externalAgentsDshOfficialProvider, models].filter(Boolean).join(' · ')
    : [provider.id, provider.baseUrl, models].filter(Boolean).join(' · ')
  return (
    <div className="kv-row kv-cli-provider">
      <span className="kv-cli-monogram">{monogram(provider.name)}</span>
      <div className="kv-row-text">
        <div className="kv-row-label">{provider.name}</div>
        <p className="kv-row-desc truncate">{subtitle}</p>
      </div>
      <div className="kv-row-control">
        {inUse ? (
          <span className="kv-tag ok">{t.externalAgentsProviderInUse}</span>
        ) : current && provider.isDefault ? (
          <Button size="sm" onClick={onUseCliConfig}>
            {t.externalAgentsProviderActivate}
          </Button>
        ) : (
          <span className={`kv-tag ${provider.isDefault ? 'ok' : ''}`}>
            {provider.isDefault
              ? t.externalAgentsNativeProviderDefault
              : official
                ? t.externalAgentsDshOfficialProvider
                : t.externalAgentsNativeProviderBadge}
          </span>
        )}
      </div>
    </div>
  )
}

/** 一个供应商的展示副标题：备注 > base_url > codex 的 model_provider 行。 */
function providerSubtitle(provider: ExternalCliProvider): string {
  if (provider.remark) return provider.remark
  const baseUrl = provider.env?.find((pair) => /BASE_URL$/i.test(pair.key))?.value
  if (baseUrl) return baseUrl
  const fromToml = provider.configToml?.match(/base_url\s*=\s*"([^"]+)"/)?.[1]
  if (fromToml) return fromToml
  try {
    const config = JSON.parse(provider.configJson ?? '{}') as Record<string, unknown>
    if (typeof config.baseUrl === 'string') return config.baseUrl
    if (typeof config.baseURL === 'string') return config.baseURL
    const options = config.options as Record<string, unknown> | undefined
    if (typeof options?.baseURL === 'string') return options.baseURL
  } catch {
    // The edit modal will surface malformed Kivio-owned JSON; keep the list usable meanwhile.
  }
  return ''
}

/**
 * 行首那个小方块里的字。
 *
 * ponytail: 不学 ccgui 按 base_url 猜品牌图标（那要一整个图标包 + 一张域名映射表，
 * 而中转站域名千奇百怪，猜错比不猜更糟）。取首个字母/汉字就够把几十条区分开。
 */
function monogram(name: string): string {
  const first = name.trim().match(/[\p{L}\p{N}]/u)?.[0] ?? '·'
  return first.toUpperCase()
}

/**
 * 「所有供应商」区块（仿 ccgui）：列表 + 导入 + 添加。
 *
 * 只改设置草稿，落地由后端在保存设置时统一做（`persist_settings` → `materialize_all`），
 * 所以这里不需要在每个操作后再调一次 apply 命令。
 */
function ProviderSection({
  lang,
  agentId,
  agentName,
  providers,
  nativeProviders,
  current,
  onPatch,
}: {
  lang: Lang
  agentId: string
  agentName: string
  providers: ExternalCliProvider[]
  nativeProviders: NonNullable<DetectedExternalAgent['nativeProviders']>
  current: string
  onPatch: (patch: Partial<ExternalCliAgentConfig>) => void
}) {
  const t = i18n[lang]
  const [editing, setEditing] = useState<ExternalCliProvider | null | undefined>(undefined)
  const [importing, setImporting] = useState(false)
  const listedNatives = withOfficialDshProviders(agentId, nativeProviders)
  const hideGenericLocal = agentId === 'dsh'

  const save = (provider: ExternalCliProvider) => {
    const exists = providers.some((p) => p.id === provider.id)
    onPatch({
      providers: exists
        ? providers.map((p) => (p.id === provider.id ? provider : p))
        : [...providers, provider],
    })
    setEditing(undefined)
  }

  const remove = (provider: ExternalCliProvider) => {
    if (!window.confirm(t.externalAgentsProviderDeleteConfirm.replace('{name}', provider.name))) return
    onPatch({
      providers: providers.filter((p) => p.id !== provider.id),
      ...(current === provider.id ? { currentProvider: '' } : {}),
    })
    void chatApi.externalCliProviderCleanup(
      agentId,
      provider.id,
      provider.nativeProviderId,
      provider.name,
    )
  }

  const importFromCcSwitch = (items: CcSwitchProvider[]) => {
    // 保留 cc-switch 的原 id：同一条再导一次是更新，不会堆出重复项。
    const merged = [...providers]
    for (const item of items) {
      const next: ExternalCliProvider = {
        id: item.id,
        name: item.name,
        remark: item.remark,
        env: item.env,
        configToml: item.configToml,
        authJson: item.authJson,
      }
      const idx = merged.findIndex((p) => p.id === item.id)
      if (idx >= 0) merged[idx] = next
      else merged.push(next)
    }
    onPatch({ providers: merged })
    setImporting(false)
  }

  // opencode / pi / grok / kimi 写原生配置；dsh 写 Kivio 私有 profile；claude / codex 用 Kivio 私有文件。
  const nativeOnDisk = agentId === 'opencode' || agentId === 'pi' || agentId === 'grok' || agentId === 'kimi'
  const envOnly = agentId !== 'claude' && agentId !== 'codex' && agentId !== 'dsh' && !nativeOnDisk

  return (
    <div className="kv-cli-providers">
      <div className="kv-cli-providers-head">
        <span className="kv-row-label">{t.externalAgentsProviderAll}</span>
        <Button size="sm" onClick={() => setImporting(true)}>
          <Download size={12} />
          {t.externalAgentsProviderImport}
        </Button>
        <Button size="sm" variant="primary" onClick={() => setEditing(null)}>
          <Plus size={12} />
          {t.externalAgentsProviderAdd}
        </Button>
      </div>
      <div className="kv-cli-card">
        {providers.length === 0 && listedNatives.length === 0 ? (
          // 一条供应商都没有时只留这句话（同 ccgui）：此时「使用 CLI 自身配置」是唯一可能的
          // 状态，把它渲染成一行可选项纯属噪音。
          <p className="kv-row-desc px-3 py-5 text-center">{t.externalAgentsProviderEmpty}</p>
        ) : (
          <>
            {!hideGenericLocal && (
              <div className="kv-row kv-cli-provider">
                <span className="kv-cli-monogram local">
                  <Terminal size={13} />
                </span>
                <div className="kv-row-text">
                  <div className="kv-row-label">{t.externalAgentsProviderNone}</div>
                  <p className="kv-row-desc">{t.externalAgentsProviderNoneDesc}</p>
                </div>
                <div className="kv-row-control">
                  {!current ? (
                    <span className="kv-tag ok">{t.externalAgentsProviderInUse}</span>
                  ) : (
                    <Button size="sm" onClick={() => onPatch({ currentProvider: '' })}>
                      {t.externalAgentsProviderActivate}
                    </Button>
                  )}
                  <span className="kv-cli-provider-spacer" />
                </div>
              </div>
            )}
            {hideGenericLocal && listedNatives.map((provider) => (
              <NativeProviderRow
                key={`native-${provider.id}`}
                lang={lang}
                provider={provider}
                current={current}
                onUseCliConfig={() => onPatch({ currentProvider: '' })}
              />
            ))}
            {providers.map((provider) => (
              <div className="kv-row kv-cli-provider" key={provider.id}>
                <span className="kv-cli-monogram">{monogram(provider.name)}</span>
                <div className="kv-row-text">
                  <div className="kv-row-label">{provider.name}</div>
                  <p className="kv-row-desc truncate">{providerSubtitle(provider)}</p>
                </div>
                <div className="kv-row-control">
                  {current === provider.id ? (
                    <span className="kv-tag ok">{t.externalAgentsProviderInUse}</span>
                  ) : (
                    <Button size="sm" onClick={() => onPatch({ currentProvider: provider.id })}>
                      {t.externalAgentsProviderActivate}
                    </Button>
                  )}
                  <IconButton size="sm" label={t.externalAgentsProviderEdit} onClick={() => setEditing(provider)}>
                    <Pencil size={13} />
                  </IconButton>
                  <IconButton size="sm" label={t.externalAgentsRemove} onClick={() => remove(provider)}>
                    <Trash2 size={13} />
                  </IconButton>
                </div>
              </div>
            ))}
            {!hideGenericLocal && listedNatives.map((provider) => (
              <NativeProviderRow
                key={`native-${provider.id}`}
                lang={lang}
                provider={provider}
                current={current}
                onUseCliConfig={() => onPatch({ currentProvider: '' })}
              />
            ))}
          </>
        )}
      </div>
      <p className="kv-row-desc mt-2">
        {nativeOnDisk
          ? t.externalAgentsNativeScope
          : envOnly
            ? t.externalAgentsProviderEnvOnly
            : t.externalAgentsProviderScope}
      </p>

      {editing !== undefined && (
        <CliProviderModal
          lang={lang}
          agentId={agentId}
          agentName={agentName}
          initial={editing}
          onSave={save}
          onClose={() => setEditing(undefined)}
        />
      )}
      {importing && (
        <CcSwitchImportModal
          lang={lang}
          agentId={agentId}
          existingIds={providers.map((p) => p.id)}
          onImport={importFromCcSwitch}
          onClose={() => setImporting(false)}
        />
      )}
    </div>
  )
}

/** 版本检查 + 一键安装/更新的状态机。只被 AgentDetail 用，抽出来纯粹是别让它涨到 200 行。 */
function useInstall(agentId: string, reloadAgents: (force?: boolean) => Promise<void>) {
  const [info, setInfo] = useState<ExternalCliInstallInfo | null>(null)
  const [checking, setChecking] = useState(true)
  const [running, setRunning] = useState(false)
  const [log, setLog] = useState<string[]>([])
  const [result, setResult] = useState<'ok' | 'fail' | null>(null)
  const logRef = useRef<HTMLPreElement | null>(null)

  const refresh = useCallback(async () => {
    setChecking(true)
    try {
      setInfo(await chatApi.externalCliInstallInfo(agentId))
    } catch {
      setInfo(null)
    } finally {
      setChecking(false)
    }
  }, [agentId])

  useEffect(() => {
    void refresh()
  }, [refresh])

  useEffect(() => {
    logRef.current?.scrollTo({ top: logRef.current.scrollHeight })
  }, [log])

  const runInstall = async () => {
    setRunning(true)
    setResult(null)
    setLog([])
    // 监听要在 invoke 之前挂上：安装命令的头几行（`$ …`）是同步发出来的。
    const unlisten = await onExternalCliInstallLog((event) => {
      if (event.agentId !== agentId) return
      if (event.done) {
        setRunning(false)
        setResult(event.success ? 'ok' : 'fail')
        return
      }
      if (event.line !== null) setLog((prev) => [...prev, event.line as string])
    })
    try {
      await chatApi.externalCliInstall(agentId)
      setResult('ok')
    } catch (err) {
      setLog((prev) => [...prev, String(err)])
      setResult('fail')
    } finally {
      setRunning(false)
      unlisten()
      await refresh()
      await reloadAgents(true)
    }
  }

  return { info, checking, running, log, result, logRef, refresh, runInstall }
}
