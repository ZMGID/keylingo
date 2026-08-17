import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  ArrowLeft,
  Download,
  FileCode2,
  FolderOpen,
  Package,
  RefreshCw,
  Search,
  Trash2,
} from 'lucide-react'
import { open } from '@tauri-apps/plugin-dialog'
import {
  chatApi,
  type PiExtensionInventory,
  type PiExtensionPackage,
  type PiLocalExtension,
} from '../chat/api'
import { Button, IconButton } from '../components/Button'
import { Input, Toggle } from './components'
import { i18n, type Lang } from './i18n'

function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim()) return error.message
  if (typeof error === 'string' && error.trim()) return error
  return String(error)
}

export function PiExtensionsSettings({ lang, onBack }: { lang: Lang; onBack: () => void }) {
  const t = i18n[lang]
  const [inventory, setInventory] = useState<PiExtensionInventory | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [source, setSource] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const [result, setResult] = useState<string | null>(null)

  const reload = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      setInventory(await chatApi.piExtensionsInventory())
    } catch (nextError) {
      setError(errorMessage(nextError))
      setInventory(null)
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void reload()
  }, [reload])

  const runAction = async (key: string, action: () => Promise<{ output?: string } | void>) => {
    setBusy(key)
    setResult(null)
    setError(null)
    try {
      const next = await action()
      const output =
        next && 'output' in next && typeof next.output === 'string' ? next.output.trim() : ''
      setResult(output || t.externalAgentsPiExtensionsCommandDone)
      await reload()
    } catch (nextError) {
      setError(errorMessage(nextError))
    } finally {
      setBusy(null)
    }
  }

  const install = async () => {
    const value = source.trim()
    if (!value) return
    await runAction('install', async () => {
      const response = await chatApi.piExtensionInstall(value)
      setSource('')
      return response
    })
  }

  const pickLocalPackage = async () => {
    const picked = await open({
      multiple: false,
      directory: true,
      defaultPath: inventory?.agentDir,
    })
    if (typeof picked === 'string') setSource(picked)
  }

  const needle = query.trim().toLowerCase()
  const packages = useMemo(
    () =>
      (inventory?.packages ?? []).filter((item) => {
        if (!needle) return true
        return [item.name, item.source, item.description ?? ''].some((value) =>
          value.toLowerCase().includes(needle),
        )
      }),
    [inventory?.packages, needle],
  )
  const localExtensions = useMemo(
    () =>
      (inventory?.localExtensions ?? []).filter((item) => {
        if (!needle) return true
        return [item.name, item.relativePath].some((value) => value.toLowerCase().includes(needle))
      }),
    [inventory?.localExtensions, needle],
  )

  return (
    <section className="kv-pi-extensions">
      <div className="kv-pi-extensions-toolbar">
        <button
          type="button"
          onClick={onBack}
          className="kv-subpage-back"
          data-tauri-drag-region="false"
        >
          <ArrowLeft size={14} />
          <span>{lang === 'zh' ? '返回' : 'Back'}</span>
          <span className="kv-subpage-back-title">{t.externalAgentsPiExtensions}</span>
        </button>
        <IconButton
          size="sm"
          label={t.externalAgentsPiExtensionsOpenDir}
          onClick={() => void chatApi.piExtensionsOpenDir()}
        >
          <FolderOpen size={13} />
        </IconButton>
        <Button
          size="sm"
          disabled={busy !== null || !inventory?.packages.length}
          onClick={() => void runAction('update-all', () => chatApi.piExtensionUpdate())}
        >
          <RefreshCw size={12} className={busy === 'update-all' ? 'animate-spin' : ''} />
          {busy === 'update-all'
            ? t.externalAgentsPiExtensionsUpdating
            : t.externalAgentsPiExtensionsUpdateAll}
        </Button>
      </div>
      <p className="kv-row-desc">{t.externalAgentsPiExtensionsIntro}</p>

      <div className="kv-pi-install">
        <div className="kv-pi-install-copy">
          <div className="kv-row-label">{t.externalAgentsPiExtensionsInstallTitle}</div>
          <p className="kv-row-desc">{t.externalAgentsPiExtensionsSecurity}</p>
        </div>
        <div className="kv-pi-install-form">
          <Input
            value={source}
            onChange={setSource}
            placeholder={t.externalAgentsPiExtensionsSourcePlaceholder}
            mono
            onKeyDown={(event) => {
              if (event.key === 'Enter') void install()
            }}
          />
          <IconButton
            size="sm"
            label={t.externalAgentsPiExtensionsPickLocal}
            disabled={busy !== null}
            onClick={() => void pickLocalPackage()}
          >
            <FolderOpen size={13} />
          </IconButton>
          <Button
            size="sm"
            variant="primary"
            disabled={busy !== null || !source.trim()}
            onClick={() => void install()}
          >
            <Download size={12} />
            {busy === 'install'
              ? t.externalAgentsPiExtensionsInstalling
              : t.externalAgentsPiExtensionsInstall}
          </Button>
        </div>
      </div>

      <div className="kv-cli-search kv-pi-extension-search">
        <div className="relative min-w-0 flex-1">
          <Search
            size={12}
            className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-[var(--text-faint)]"
          />
          <Input
            value={query}
            onChange={setQuery}
            placeholder={t.externalAgentsPiExtensionsSearch}
            className="!pl-6 !text-[11.5px]"
          />
        </div>
        <IconButton
          size="sm"
          label={t.externalAgentsRescan}
          disabled={loading || busy !== null}
          onClick={() => void reload()}
        >
          <RefreshCw size={13} className={loading ? 'animate-spin' : ''} />
        </IconButton>
      </div>

      {error && (
        <div className="kv-pi-extension-message error" role="alert">
          {error}
        </div>
      )}
      {result && <pre className="kv-pi-extension-message">{result}</pre>}

      {loading && !inventory ? (
        <p className="kv-row-desc kv-pi-extension-state">{t.externalAgentsPiExtensionsLoading}</p>
      ) : !inventory ? (
        <div className="kv-pi-extension-state">
          <p className="kv-row-desc">{t.externalAgentsPiExtensionsEmpty}</p>
          <Button size="sm" onClick={() => void reload()}>
            {t.externalAgentsPiExtensionsRetry}
          </Button>
        </div>
      ) : (
        <>
          <ExtensionSection
            title={t.externalAgentsPiExtensionsPackages}
            count={packages.length}
            empty={t.externalAgentsPiExtensionsNoPackages}
          >
            {packages.map((item) => (
              <PackageRow key={item.source} lang={lang} item={item} busy={busy} onRun={runAction} />
            ))}
          </ExtensionSection>

          <ExtensionSection
            title={t.externalAgentsPiExtensionsLocal}
            count={localExtensions.length}
            empty={t.externalAgentsPiExtensionsNoLocal}
          >
            {localExtensions.map((item) => (
              <LocalExtensionRow
                key={item.relativePath}
                lang={lang}
                item={item}
                busy={busy}
                onRun={runAction}
              />
            ))}
          </ExtensionSection>
        </>
      )}
    </section>
  )
}

function ExtensionSection({
  title,
  count,
  empty,
  children,
}: {
  title: string
  count: number
  empty: string
  children: React.ReactNode
}) {
  return (
    <div className="kv-pi-extension-section">
      <div className="kv-pi-extension-section-head">
        <span className="kv-row-label">{title}</span>
        <span className="kv-tag">{count}</span>
      </div>
      <div className="kv-cli-card">
        {count > 0 ? children : <p className="kv-row-desc px-3 py-5 text-center">{empty}</p>}
      </div>
    </div>
  )
}

function PackageRow({
  lang,
  item,
  busy,
  onRun,
}: {
  lang: Lang
  item: PiExtensionPackage
  busy: string | null
  onRun: (key: string, action: () => Promise<{ output?: string } | void>) => Promise<void>
}) {
  const t = i18n[lang]
  const actionKey = `package:${item.source}`
  const working = busy === actionKey
  const resourceLabel =
    item.resources.length > 0
      ? item.resources.join(' · ')
      : t.externalAgentsPiExtensionsResourcePackage
  const remove = () => {
    if (!window.confirm(t.externalAgentsPiExtensionsRemoveConfirm.replace('{name}', item.name)))
      return
    void onRun(actionKey, () => chatApi.piExtensionRemove(item.source))
  }
  return (
    <div className="kv-row kv-pi-extension-row">
      <span className="kv-pi-extension-icon">
        <Package size={14} />
      </span>
      <div className="kv-row-text">
        <div className="kv-pi-extension-title-line">
          <span className="kv-row-label">{item.name}</span>
          {item.version && <span className="kv-pi-extension-version">v{item.version}</span>}
        </div>
        <p className="kv-row-desc truncate">{item.description || item.source}</p>
        <p className="kv-pi-extension-meta">
          {resourceLabel}
          {item.hasExtensions && item.extensionEntries > 0
            ? ` · ${t.externalAgentsPiExtensionsCount.replace('{count}', String(item.extensionEntries))}`
            : ''}
        </p>
      </div>
      <div className="kv-row-control kv-pi-extension-actions">
        {item.hasExtensions ? (
          item.canToggle ? (
            <Toggle
              checked={item.enabled}
              disabled={busy !== null}
              onChange={(enabled) =>
                void onRun(actionKey, () =>
                  chatApi.piExtensionSetEnabled('package', item.source, enabled),
                )
              }
              ariaLabel={`${item.name} ${t.externalAgentsEnable}`}
            />
          ) : (
            <span className="kv-tag">{t.externalAgentsPiExtensionsCustomConfig}</span>
          )
        ) : (
          <span className="kv-tag">{t.externalAgentsPiExtensionsResourcePackage}</span>
        )}
        <IconButton
          size="sm"
          label={t.externalAgentsPiExtensionsOpen}
          disabled={!item.path || busy !== null}
          onClick={() => void chatApi.piExtensionOpen('package', item.source)}
        >
          <FolderOpen size={13} />
        </IconButton>
        <IconButton
          size="sm"
          label={t.externalAgentsPiExtensionsUpdate}
          disabled={busy !== null}
          onClick={() => void onRun(actionKey, () => chatApi.piExtensionUpdate(item.source))}
        >
          <RefreshCw size={13} className={working ? 'animate-spin' : ''} />
        </IconButton>
        <IconButton
          size="sm"
          label={t.externalAgentsPiExtensionsRemove}
          disabled={busy !== null}
          onClick={remove}
        >
          <Trash2 size={13} />
        </IconButton>
      </div>
    </div>
  )
}

function LocalExtensionRow({
  lang,
  item,
  busy,
  onRun,
}: {
  lang: Lang
  item: PiLocalExtension
  busy: string | null
  onRun: (key: string, action: () => Promise<{ output?: string } | void>) => Promise<void>
}) {
  const t = i18n[lang]
  const actionKey = `local:${item.relativePath}`
  return (
    <div className="kv-row kv-pi-extension-row">
      <span className="kv-pi-extension-icon">
        <FileCode2 size={14} />
      </span>
      <div className="kv-row-text">
        <div className="kv-row-label">{item.name}</div>
        <p className="kv-row-desc truncate">{item.relativePath}</p>
        <p className="kv-pi-extension-meta">
          {item.kind === 'directory'
            ? t.externalAgentsPiExtensionsLocalDirectory
            : t.externalAgentsPiExtensionsLocalFile}
        </p>
      </div>
      <div className="kv-row-control kv-pi-extension-actions">
        <Toggle
          checked={item.enabled}
          disabled={busy !== null}
          onChange={(enabled) =>
            void onRun(actionKey, () =>
              chatApi.piExtensionSetEnabled('local', item.relativePath, enabled),
            )
          }
          ariaLabel={`${item.name} ${t.externalAgentsEnable}`}
        />
        <IconButton
          size="sm"
          label={t.externalAgentsPiExtensionsOpen}
          disabled={busy !== null}
          onClick={() => void chatApi.piExtensionOpen('local', item.relativePath)}
        >
          <FolderOpen size={13} />
        </IconButton>
      </div>
    </div>
  )
}
