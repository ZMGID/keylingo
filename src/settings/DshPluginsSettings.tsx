import { useCallback, useEffect, useMemo, useState, type ReactNode } from 'react'
import { ArrowLeft, ChevronDown, FileText, RefreshCw, Search, Terminal, Workflow, Globe } from 'lucide-react'
import {
  chatApi,
  type DshPluginEntry,
  type DshPluginSettingsPatch,
  type DshPluginSettingsSnapshot,
} from '../chat/api'
import { Button, IconButton } from '../components/Button'
import { Input } from './components'
import { dshPluginShortName } from './dshPluginNames'
import { i18n, type Lang } from './i18n'

type TabId = 'config' | 'list'

function errorMessage(err: unknown): string {
  if (typeof err === 'string' && err.trim()) return err
  if (err instanceof Error && err.message.trim()) return err.message
  if (err && typeof err === 'object' && 'message' in err) {
    const message = (err as { message: unknown }).message
    if (typeof message === 'string' && message.trim()) return message
  }
  return ''
}

function parseOptionalNumber(text: string): { value: number | null; invalid: boolean } {
  const trimmed = text.trim()
  if (!trimmed) return { value: null, invalid: false }
  if (!/^\d+$/.test(trimmed)) return { value: null, invalid: true }
  const value = Number(trimmed)
  if (!Number.isSafeInteger(value) || value <= 0) return { value: null, invalid: true }
  return { value, invalid: false }
}

export function DshPluginsSettings({
  lang,
  onBack,
}: {
  lang: Lang
  onBack?: () => void
}) {
  const t = i18n[lang]
  const [tab, setTab] = useState<TabId>('config')
  const [snapshot, setSnapshot] = useState<DshPluginSettingsSnapshot | null>(null)
  const [loadError, setLoadError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)

  const reload = useCallback(async () => {
    setLoading(true)
    setLoadError(null)
    try {
      setSnapshot(await chatApi.dshPluginSettingsGet())
    } catch (err) {
      setLoadError(errorMessage(err))
      setSnapshot(null)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void reload()
  }, [reload])

  return (
    <section className="kv-dsh-plugins">
      <div className="kv-dsh-plugins-toolbar">
        {onBack ? (
          <button
            type="button"
            onClick={onBack}
            className="kv-subpage-back"
            data-tauri-drag-region="false"
          >
            <ArrowLeft size={14} />
            <span>{lang === 'zh' ? '返回' : 'Back'}</span>
            <span className="kv-subpage-back-title">{t.externalAgentsDshPlugins}</span>
          </button>
        ) : (
          <p className="kv-row-desc kv-dsh-plugins-copy">{t.externalAgentsDshPluginsIntro}</p>
        )}
        <Button size="sm" onClick={() => void chatApi.dshOpenSettingsFile()}>
          <FileText size={12} />
          {t.externalAgentsDshPluginsOpenFile}
        </Button>
      </div>
      {onBack && <p className="kv-row-desc">{t.externalAgentsDshPluginsIntro}</p>}

      <div className="kv-seg kv-dsh-plugins-tabs" role="tablist" aria-label={t.externalAgentsDshPlugins}>
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'config'}
          className={tab === 'config' ? 'active' : ''}
          onClick={() => setTab('config')}
        >
          {t.externalAgentsDshPluginsConfigTab}
        </button>
        <button
          type="button"
          role="tab"
          aria-selected={tab === 'list'}
          className={tab === 'list' ? 'active' : ''}
          onClick={() => setTab('list')}
        >
          {t.externalAgentsDshPluginsListTab}
        </button>
      </div>

      {tab === 'config' ? (
        loading ? (
          <p className="kv-row-desc kv-dsh-plugins-status">{t.externalAgentsDshPluginsLoading}</p>
        ) : loadError || !snapshot ? (
          <div className="kv-dsh-plugins-failure">
            <div>
              <p role="alert">{t.externalAgentsDshPluginsEmpty}</p>
              {loadError && <p className="kv-row-desc">{loadError}</p>}
            </div>
            <Button size="sm" onClick={() => void reload()}>
              {t.externalAgentsDshPluginsListRetry}
            </Button>
          </div>
        ) : (
          <ConfigTab lang={lang} snapshot={snapshot} onSaved={setSnapshot} />
        )
      ) : (
        <InventoryTab lang={lang} />
      )}
    </section>
  )
}

function ConfigTab({
  lang,
  snapshot,
  onSaved,
}: {
  lang: Lang
  snapshot: DshPluginSettingsSnapshot
  onSaved: (next: DshPluginSettingsSnapshot) => void
}) {
  const t = i18n[lang]
  return (
    <div className="kv-dsh-config">
      <PluginConfigCard
        lang={lang}
        icon={<Terminal size={16} />}
        title={t.externalAgentsDshPluginsBashTitle}
        description={t.externalAgentsDshPluginsBashDesc}
        defaultOpen
      >
        <ShellCard lang={lang} snapshot={snapshot} onSaved={onSaved} />
      </PluginConfigCard>
      <PluginConfigCard
        lang={lang}
        icon={<Workflow size={16} />}
        title={t.externalAgentsDshPluginsLoopTitle}
        description={t.externalAgentsDshPluginsLoopDesc}
      >
        <AgentLoopCard lang={lang} snapshot={snapshot} onSaved={onSaved} />
      </PluginConfigCard>
      <PluginConfigCard
        lang={lang}
        icon={<Globe size={16} />}
        title={t.externalAgentsDshPluginsSearchTitle}
        description={t.externalAgentsDshPluginsSearchDesc}
      >
        <WebSearchCard lang={lang} snapshot={snapshot} onSaved={onSaved} />
      </PluginConfigCard>
    </div>
  )
}

function PluginConfigCard({
  lang,
  icon,
  title,
  description,
  defaultOpen = false,
  children,
}: {
  lang: Lang
  icon: ReactNode
  title: string
  description: string
  defaultOpen?: boolean
  children: ReactNode
}) {
  const t = i18n[lang]
  const [open, setOpen] = useState(defaultOpen)
  return (
    <article className={`kv-dsh-config-card${open ? ' is-open' : ''}`}>
      <button
        type="button"
        className="kv-dsh-config-card-head"
        aria-expanded={open}
        onClick={() => setOpen((value) => !value)}
      >
        <span className="kv-dsh-config-card-icon">{icon}</span>
        <span className="kv-dsh-config-card-copy">
          <span className="kv-dsh-config-card-title">{title}</span>
          <span className="kv-row-desc">{description}</span>
        </span>
        <ChevronDown
          size={16}
          className={`kv-dsh-chevron${open ? ' is-open' : ''}`}
          aria-label={open ? t.externalAgentsCollapse : t.externalAgentsDshPluginsExpand}
        />
      </button>
      {open && <div className="kv-dsh-config-card-body">{children}</div>}
    </article>
  )
}

function NumberField({
  id,
  label,
  hint,
  text,
  placeholder,
  overridden,
  invalid,
  invalidLabel,
  overriddenLabel,
  resetLabel,
  onEdit,
  onReset,
}: {
  id: string
  label: string
  hint: string
  text: string
  placeholder: string
  overridden: boolean
  invalid: boolean
  invalidLabel: string
  overriddenLabel: string
  resetLabel: string
  onEdit: (text: string) => void
  onReset: () => void
}) {
  return (
    <div className="kv-dsh-field">
      <div className="kv-dsh-field-label-row">
        <label htmlFor={id} className="kv-row-label">
          {label}
        </label>
        {overridden && <span className="kv-dsh-override">{overriddenLabel}</span>}
        {overridden && (
          <Button size="sm" variant="ghost" onClick={onReset}>
            {resetLabel}
          </Button>
        )}
      </div>
      <Input
        id={id}
        value={text}
        onChange={onEdit}
        placeholder={placeholder}
        mono
        aria-invalid={invalid}
      />
      <p className={`kv-row-desc${invalid ? ' kv-dsh-field-error' : ''}`}>
        {invalid ? invalidLabel : hint}
      </p>
    </div>
  )
}

function CardActions({
  lang,
  dirty,
  invalid,
  saving,
  onSave,
  onDiscard,
}: {
  lang: Lang
  dirty: boolean
  invalid: boolean
  saving: boolean
  onSave: () => void
  onDiscard: () => void
}) {
  const t = i18n[lang]
  if (!dirty) return null
  return (
    <div className="kv-dsh-card-actions">
      <span className="kv-dsh-unsaved">{t.externalAgentsDshPluginsUnsaved}</span>
      <Button size="sm" onClick={onDiscard} disabled={saving}>
        {t.externalAgentsDshPluginsDiscard}
      </Button>
      <Button size="sm" variant="primary" onClick={onSave} disabled={saving || invalid}>
        {saving ? t.externalAgentsDshPluginsSaving : t.externalAgentsDshPluginsSave}
      </Button>
    </div>
  )
}

function useCardSave(
  buildPatch: () => DshPluginSettingsPatch,
  onSaved: (next: DshPluginSettingsSnapshot) => void,
) {
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const save = async () => {
    setSaving(true)
    setError(null)
    try {
      onSaved(await chatApi.dshPluginSettingsSave(buildPatch()))
    } catch (err) {
      setError(String(err))
    } finally {
      setSaving(false)
    }
  }
  return { saving, error, save }
}

function ShellCard({
  lang,
  snapshot,
  onSaved,
}: {
  lang: Lang
  snapshot: DshPluginSettingsSnapshot
  onSaved: (next: DshPluginSettingsSnapshot) => void
}) {
  const t = i18n[lang]
  const [timeoutMs, setTimeoutMs] = useState(snapshot.shell.timeoutMs?.toString() ?? '')
  const [maxOutputBytes, setMaxOutputBytes] = useState(
    snapshot.shell.maxOutputBytes?.toString() ?? '',
  )
  useEffect(() => {
    setTimeoutMs(snapshot.shell.timeoutMs?.toString() ?? '')
    setMaxOutputBytes(snapshot.shell.maxOutputBytes?.toString() ?? '')
  }, [snapshot.shell.maxOutputBytes, snapshot.shell.timeoutMs])

  const parsedTimeout = parseOptionalNumber(timeoutMs)
  const parsedOutput = parseOptionalNumber(maxOutputBytes)
  const dirty =
    (parsedTimeout.value ?? null) !== snapshot.shell.timeoutMs ||
    (parsedOutput.value ?? null) !== snapshot.shell.maxOutputBytes
  const { saving, error, save } = useCardSave(
    () => ({
      shell: {
        timeoutMs: parsedTimeout.value,
        maxOutputBytes: parsedOutput.value,
      },
    }),
    onSaved,
  )

  return (
    <>
      <NumberField
        id="dsh-shell-timeout"
        label={t.externalAgentsDshPluginsBashTimeout}
        hint={t.externalAgentsDshPluginsBashTimeoutHint}
        text={timeoutMs}
        placeholder={String(snapshot.shell.timeoutMsDefault)}
        overridden={snapshot.shell.timeoutMs != null}
        invalid={parsedTimeout.invalid}
        invalidLabel={t.externalAgentsDshPluginsInvalidNumber}
        overriddenLabel={t.externalAgentsDshPluginsOverridden}
        resetLabel={t.externalAgentsDshPluginsReset}
        onEdit={setTimeoutMs}
        onReset={() => setTimeoutMs('')}
      />
      <NumberField
        id="dsh-shell-output"
        label={t.externalAgentsDshPluginsBashOutput}
        hint={t.externalAgentsDshPluginsBashOutputHint}
        text={maxOutputBytes}
        placeholder={String(snapshot.shell.maxOutputBytesDefault)}
        overridden={snapshot.shell.maxOutputBytes != null}
        invalid={parsedOutput.invalid}
        invalidLabel={t.externalAgentsDshPluginsInvalidNumber}
        overriddenLabel={t.externalAgentsDshPluginsOverridden}
        resetLabel={t.externalAgentsDshPluginsReset}
        onEdit={setMaxOutputBytes}
        onReset={() => setMaxOutputBytes('')}
      />
      <CardActions
        lang={lang}
        dirty={dirty}
        invalid={parsedTimeout.invalid || parsedOutput.invalid}
        saving={saving}
        onSave={() => void save()}
        onDiscard={() => {
          setTimeoutMs(snapshot.shell.timeoutMs?.toString() ?? '')
          setMaxOutputBytes(snapshot.shell.maxOutputBytes?.toString() ?? '')
        }}
      />
      {error && <p className="kv-row-desc kv-dsh-field-error">{error}</p>}
    </>
  )
}

function AgentLoopCard({
  lang,
  snapshot,
  onSaved,
}: {
  lang: Lang
  snapshot: DshPluginSettingsSnapshot
  onSaved: (next: DshPluginSettingsSnapshot) => void
}) {
  const t = i18n[lang]
  const [parallel, setParallel] = useState(
    snapshot.agentLoop.maxParallelToolCalls?.toString() ?? '',
  )
  useEffect(() => {
    setParallel(snapshot.agentLoop.maxParallelToolCalls?.toString() ?? '')
  }, [snapshot.agentLoop.maxParallelToolCalls])
  const parsed = parseOptionalNumber(parallel)
  const dirty = (parsed.value ?? null) !== snapshot.agentLoop.maxParallelToolCalls
  const { saving, error, save } = useCardSave(
    () => ({ agentLoop: { maxParallelToolCalls: parsed.value } }),
    onSaved,
  )
  return (
    <>
      <NumberField
        id="dsh-loop-parallel"
        label={t.externalAgentsDshPluginsLoopParallel}
        hint={t.externalAgentsDshPluginsLoopParallelHint}
        text={parallel}
        placeholder={String(snapshot.agentLoop.maxParallelToolCallsDefault)}
        overridden={snapshot.agentLoop.maxParallelToolCalls != null}
        invalid={parsed.invalid}
        invalidLabel={t.externalAgentsDshPluginsInvalidNumber}
        overriddenLabel={t.externalAgentsDshPluginsOverridden}
        resetLabel={t.externalAgentsDshPluginsReset}
        onEdit={setParallel}
        onReset={() => setParallel('')}
      />
      <CardActions
        lang={lang}
        dirty={dirty}
        invalid={parsed.invalid}
        saving={saving}
        onSave={() => void save()}
        onDiscard={() => setParallel(snapshot.agentLoop.maxParallelToolCalls?.toString() ?? '')}
      />
      {error && <p className="kv-row-desc kv-dsh-field-error">{error}</p>}
    </>
  )
}

function WebSearchCard({
  lang,
  snapshot,
  onSaved,
}: {
  lang: Lang
  snapshot: DshPluginSettingsSnapshot
  onSaved: (next: DshPluginSettingsSnapshot) => void
}) {
  const t = i18n[lang]
  const [baseUrl, setBaseUrl] = useState(snapshot.webSearch.baseUrl ?? '')
  const [maxUses, setMaxUses] = useState(snapshot.webSearch.maxUses?.toString() ?? '')
  const [apiKey, setApiKey] = useState('')
  useEffect(() => {
    setBaseUrl(snapshot.webSearch.baseUrl ?? '')
    setMaxUses(snapshot.webSearch.maxUses?.toString() ?? '')
    setApiKey('')
  }, [snapshot.webSearch.baseUrl, snapshot.webSearch.maxUses])
  const parsedUses = parseOptionalNumber(maxUses)
  const nextUrl = baseUrl.trim() || null
  const dirty =
    nextUrl !== snapshot.webSearch.baseUrl ||
    (parsedUses.value ?? null) !== snapshot.webSearch.maxUses ||
    apiKey.trim().length > 0
  const { saving, error, save } = useCardSave(
    () => ({
      webSearch: {
        baseUrl: nextUrl,
        maxUses: parsedUses.value,
        ...(apiKey.trim() ? { apiKey: apiKey.trim() } : {}),
      },
    }),
    onSaved,
  )
  return (
    <>
      <div className="kv-dsh-field">
        <label htmlFor="dsh-search-key" className="kv-row-label">
          {t.externalAgentsDshPluginsSearchKey}
        </label>
        <Input
          id="dsh-search-key"
          type="password"
          value={apiKey}
          onChange={setApiKey}
          placeholder={
            snapshot.webSearch.apiKeyConfigured
              ? t.externalAgentsDshPluginsSearchKeySet
              : t.externalAgentsDshPluginsSearchKeyUnset
          }
          mono
          disabled={!snapshot.webSearch.apiKeyWritable}
        />
        <p className="kv-row-desc">
          {snapshot.webSearch.apiKeyWritable
            ? t.externalAgentsDshPluginsSearchKeyHint
            : t.externalAgentsDshPluginsSearchKeyLocked}
        </p>
      </div>
      <div className="kv-dsh-field">
        <div className="kv-dsh-field-label-row">
          <label htmlFor="dsh-search-url" className="kv-row-label">
            {t.externalAgentsDshPluginsSearchUrl}
          </label>
          {snapshot.webSearch.baseUrl && (
            <span className="kv-dsh-override">{t.externalAgentsDshPluginsOverridden}</span>
          )}
        </div>
        <Input
          id="dsh-search-url"
          value={baseUrl}
          onChange={setBaseUrl}
          placeholder={snapshot.webSearch.baseUrlDefault}
          mono
        />
        <p className="kv-row-desc">{t.externalAgentsDshPluginsSearchUrlHint}</p>
      </div>
      <NumberField
        id="dsh-search-max"
        label={t.externalAgentsDshPluginsSearchMax}
        hint={t.externalAgentsDshPluginsSearchMaxHint}
        text={maxUses}
        placeholder={String(snapshot.webSearch.maxUsesDefault)}
        overridden={snapshot.webSearch.maxUses != null}
        invalid={parsedUses.invalid}
        invalidLabel={t.externalAgentsDshPluginsInvalidNumber}
        overriddenLabel={t.externalAgentsDshPluginsOverridden}
        resetLabel={t.externalAgentsDshPluginsReset}
        onEdit={setMaxUses}
        onReset={() => setMaxUses('')}
      />
      <CardActions
        lang={lang}
        dirty={dirty}
        invalid={parsedUses.invalid}
        saving={saving}
        onSave={() => void save()}
        onDiscard={() => {
          setBaseUrl(snapshot.webSearch.baseUrl ?? '')
          setMaxUses(snapshot.webSearch.maxUses?.toString() ?? '')
          setApiKey('')
        }}
      />
      {error && <p className="kv-row-desc kv-dsh-field-error">{error}</p>}
    </>
  )
}

function InventoryTab({ lang }: { lang: Lang }) {
  const t = i18n[lang]
  const [query, setQuery] = useState('')
  const [entries, setEntries] = useState<DshPluginEntry[] | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [loading, setLoading] = useState(true)
  const [expanded, setExpanded] = useState<string | null>(null)

  const reload = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      setEntries(await chatApi.dshPluginInventory())
    } catch (err) {
      setError(errorMessage(err))
      setEntries(null)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void reload()
  }, [reload])

  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase()
    if (!entries) return []
    if (!needle) return entries
    return entries.filter((entry) => {
      const title = dshPluginShortName(entry.moduleName).toLowerCase()
      return title.includes(needle) || entry.id.toLowerCase().includes(needle) || entry.moduleName.toLowerCase().includes(needle)
    })
  }, [entries, query])

  if (loading) {
    return <p className="kv-row-desc kv-dsh-plugins-status">{t.externalAgentsDshPluginsListLoading}</p>
  }
  if (entries === null) {
    return (
      <div className="kv-dsh-plugins-failure">
        <div>
          <p role="alert">{t.externalAgentsDshPluginsListError}</p>
          {error && <p className="kv-row-desc">{error}</p>}
        </div>
        <Button size="sm" onClick={() => void reload()}>
          {t.externalAgentsDshPluginsListRetry}
        </Button>
      </div>
    )
  }

  return (
    <div className="kv-dsh-inventory">
      <div className="kv-dsh-inventory-search">
        <Search size={13} aria-hidden />
        <Input
          value={query}
          onChange={setQuery}
          placeholder={t.externalAgentsDshPluginsListSearch}
          aria-label={t.externalAgentsDshPluginsListSearch}
        />
        <IconButton
          size="sm"
          label={t.externalAgentsDshPluginsListRetry}
          onClick={() => void reload()}
        >
          <RefreshCw size={13} />
        </IconButton>
      </div>
      <div className="kv-dsh-inventory-count">
        <span>{t.externalAgentsDshPluginsListCatalog}</span>
        <span>{filtered.length}</span>
      </div>
      {entries && entries.length === 0 ? (
        <p className="kv-row-desc">{t.externalAgentsDshPluginsListEmpty}</p>
      ) : filtered.length === 0 ? (
        <p className="kv-row-desc">{t.externalAgentsDshPluginsListEmptySearch}</p>
      ) : (
        <ul className="kv-dsh-inventory-grid">
          {filtered.map((entry) => {
            const title = dshPluginShortName(entry.moduleName)
            const open = expanded === entry.id
            return (
              <li key={entry.id} className={`kv-dsh-inventory-card${open ? ' is-open' : ''}`}>
                <button
                  type="button"
                  className="kv-dsh-inventory-card-head"
                  aria-expanded={open}
                  onClick={() => setExpanded((current) => (current === entry.id ? null : entry.id))}
                >
                  <strong className="kv-dsh-inventory-name" title={entry.moduleName}>
                    {title}
                  </strong>
                  <span className="kv-dsh-inventory-meta">
                    {entry.enabled && <span className="kv-dsh-status-dot" aria-hidden />}
                    <span className={`kv-dsh-status-tag${entry.enabled ? ' is-on' : ''}`}>
                      {entry.enabled
                        ? t.externalAgentsDshPluginsEnabled
                        : t.externalAgentsDshPluginsDisabled}
                    </span>
                    <ChevronDown size={13} className={`kv-dsh-chevron${open ? ' is-open' : ''}`} />
                  </span>
                </button>
                {open && (
                  <dl className="kv-dsh-inventory-details">
                    <div>
                      <dt>{t.externalAgentsDshPluginsEntryId}</dt>
                      <dd>
                        <code>{entry.id}</code>
                      </dd>
                    </div>
                    <div>
                      <dt>{t.externalAgentsDshPluginsModule}</dt>
                      <dd>{entry.moduleName}</dd>
                    </div>
                  </dl>
                )}
              </li>
            )
          })}
        </ul>
      )}
    </div>
  )
}
