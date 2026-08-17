import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  ArrowLeft,
  BookOpen,
  FolderOpen,
  FolderPlus,
  RefreshCw,
  Search,
  Trash2,
} from 'lucide-react'
import { open } from '@tauri-apps/plugin-dialog'
import { chatApi, type PiSkillEntry, type PiSkillInventory } from '../chat/api'
import { Button, IconButton } from '../components/Button'
import { Input, Toggle } from './components'
import { i18n, type Lang } from './i18n'

function errorMessage(error: unknown): string {
  if (error instanceof Error && error.message.trim()) return error.message
  if (typeof error === 'string' && error.trim()) return error
  return String(error)
}

const SOURCE_ORDER: PiSkillEntry['sourceKind'][] = ['pi', 'agents', 'package', 'configured']

export function PiSkillsSettings({ lang, onBack }: { lang: Lang; onBack: () => void }) {
  const t = i18n[lang]
  const [inventory, setInventory] = useState<PiSkillInventory | null>(null)
  const [loading, setLoading] = useState(true)
  const [error, setError] = useState<string | null>(null)
  const [query, setQuery] = useState('')
  const [busy, setBusy] = useState<string | null>(null)
  const [result, setResult] = useState<string | null>(null)

  const reload = useCallback(async () => {
    setLoading(true)
    setError(null)
    try {
      setInventory(await chatApi.piSkillsInventory())
    } catch (nextError) {
      setError(errorMessage(nextError))
    } finally {
      setLoading(false)
    }
  }, [])

  useEffect(() => {
    void reload()
  }, [reload])

  const runAction = async (key: string, action: () => Promise<void>, success?: string) => {
    setBusy(key)
    setError(null)
    setResult(null)
    try {
      await action()
      if (success) setResult(success)
      await reload()
    } catch (nextError) {
      setError(errorMessage(nextError))
    } finally {
      setBusy(null)
    }
  }

  const pickScanPath = async () => {
    const picked = await open({
      multiple: false,
      directory: true,
      defaultPath: inventory?.agentDir,
    })
    if (typeof picked !== 'string') return
    await runAction(
      'add-path',
      () => chatApi.piSkillAddPath(picked),
      t.externalAgentsPiSkillsPathAdded,
    )
  }

  const needle = query.trim().toLowerCase()
  const groups = useMemo(
    () =>
      SOURCE_ORDER.map((sourceKind) => ({
        sourceKind,
        items: (inventory?.skills ?? []).filter((skill) => {
          if (skill.sourceKind !== sourceKind) return false
          if (!needle) return true
          return [skill.name, skill.description ?? '', skill.path, skill.packageSource ?? ''].some(
            (value) => value.toLowerCase().includes(needle),
          )
        }),
      })).filter((group) => group.items.length > 0),
    [inventory?.skills, needle],
  )

  const groupTitle = (sourceKind: PiSkillEntry['sourceKind']) => {
    if (sourceKind === 'pi') return t.externalAgentsPiSkillsPiGlobal
    if (sourceKind === 'agents') return t.externalAgentsPiSkillsAgentsGlobal
    if (sourceKind === 'package') return t.externalAgentsPiSkillsPackages
    return t.externalAgentsPiSkillsConfigured
  }

  return (
    <section className="kv-pi-skills">
      <div className="kv-pi-skills-toolbar">
        <button
          type="button"
          onClick={onBack}
          className="kv-subpage-back"
          data-tauri-drag-region="false"
        >
          <ArrowLeft size={14} />
          <span>{lang === 'zh' ? '返回' : 'Back'}</span>
          <span className="kv-subpage-back-title">{t.externalAgentsPiSkills}</span>
        </button>
        <span className="kv-pi-skills-toolbar-actions">
          <IconButton
            size="sm"
            label={t.externalAgentsPiSkillsOpenPiDir}
            disabled={busy !== null}
            onClick={() => void chatApi.piSkillsOpenDir('pi')}
          >
            <FolderOpen size={13} />
          </IconButton>
          <IconButton
            size="sm"
            label={t.externalAgentsPiSkillsOpenAgentsDir}
            disabled={busy !== null}
            onClick={() => void chatApi.piSkillsOpenDir('agents')}
          >
            <BookOpen size={13} />
          </IconButton>
          <IconButton
            size="sm"
            label={t.externalAgentsPiSkillsRefresh}
            disabled={loading || busy !== null}
            onClick={() => void reload()}
          >
            <RefreshCw size={13} className={loading ? 'animate-spin' : ''} />
          </IconButton>
        </span>
      </div>
      <p className="kv-row-desc">{t.externalAgentsPiSkillsIntro}</p>

      <div className="kv-cli-card kv-pi-skills-settings">
        <div className="kv-row">
          <div className="kv-row-text">
            <div className="kv-row-label">{t.externalAgentsPiSkillsCommands}</div>
            <p className="kv-row-desc">{t.externalAgentsPiSkillsCommandsHint}</p>
          </div>
          <div className="kv-row-control">
            <Toggle
              checked={inventory?.skillCommandsEnabled ?? true}
              disabled={loading || busy !== null || !inventory}
              onChange={(enabled) =>
                void runAction('commands', () => chatApi.piSkillCommandsSetEnabled(enabled))
              }
            />
          </div>
        </div>
        <div className="kv-row kv-pi-skills-path-head">
          <div className="kv-row-text">
            <div className="kv-row-label">{t.externalAgentsPiSkillsPaths}</div>
            <p className="kv-row-desc">{t.externalAgentsPiSkillsPathsHint}</p>
          </div>
          <div className="kv-row-control">
            <Button
              size="sm"
              disabled={loading || busy !== null || !inventory}
              onClick={() => void pickScanPath()}
            >
              <FolderPlus size={13} />
              {t.externalAgentsPiSkillsAddPath}
            </Button>
          </div>
        </div>
        {(inventory?.configuredPaths ?? []).map((entry) => (
          <div className="kv-row kv-pi-skill-path-row" key={entry.path}>
            <div className="kv-row-text min-w-0">
              <code className="kv-pi-skill-path" title={entry.path}>{entry.path}</code>
              {!entry.exists && (
                <span className="kv-tag warn">{t.externalAgentsPiSkillsMissingPath}</span>
              )}
            </div>
            <div className="kv-row-control">
              <IconButton
                size="xs"
                label={t.externalAgentsPiSkillsRemovePath}
                disabled={busy !== null}
                onClick={() =>
                  void runAction('remove-path', () => chatApi.piSkillRemovePath(entry.path))
                }
              >
                <Trash2 size={12} />
              </IconButton>
            </div>
          </div>
        ))}
      </div>

      <div className="kv-cli-search kv-pi-skill-search">
        <div className="relative min-w-0 flex-1">
          <Search
            size={12}
            className="pointer-events-none absolute left-2 top-1/2 -translate-y-1/2 text-[var(--text-faint)]"
          />
          <Input
            value={query}
            onChange={setQuery}
            placeholder={t.externalAgentsPiSkillsSearch}
            className="!pl-6 !text-[11.5px]"
          />
        </div>
      </div>

      {error && <div className="kv-pi-extension-message error" role="alert">{error}</div>}
      {result && <div className="kv-pi-extension-message success" role="status">{result}</div>}

      {loading && !inventory ? (
        <div className="kv-pi-extension-empty">
          <RefreshCw size={16} className="animate-spin" />
          <span>{t.externalAgentsPiSkillsLoading}</span>
        </div>
      ) : !inventory ? (
        <div className="kv-pi-extension-empty">
          <span>{t.externalAgentsPiSkillsEmpty}</span>
          <Button size="sm" onClick={() => void reload()}>
            {t.externalAgentsPiSkillsRetry}
          </Button>
        </div>
      ) : groups.length === 0 ? (
        <div className="kv-pi-extension-empty">{t.externalAgentsPiSkillsNoMatch}</div>
      ) : (
        <div className="kv-pi-skill-groups">
          {groups.map((group) => (
            <section className="kv-pi-extension-group" key={group.sourceKind}>
              <div className="kv-pi-extension-group-head">
                <span>{groupTitle(group.sourceKind)}</span>
                <span className="kv-tag">{group.items.length}</span>
              </div>
              <div className="kv-pi-extension-list">
                {group.items.map((skill) => (
                  <SkillRow
                    key={`${skill.packageSource ?? skill.sourceKind}:${skill.path}`}
                    skill={skill}
                    lang={lang}
                    busy={busy}
                    onToggle={(enabled) =>
                      runAction(`toggle:${skill.path}`, () => chatApi.piSkillSetEnabled(skill, enabled))
                    }
                    onOpen={() => chatApi.piSkillOpen(skill)}
                    onRemove={() =>
                      runAction(`remove:${skill.path}`, () => chatApi.piSkillRemove(skill))
                    }
                  />
                ))}
              </div>
            </section>
          ))}
        </div>
      )}
    </section>
  )
}

function SkillRow({
  skill,
  lang,
  busy,
  onToggle,
  onOpen,
  onRemove,
}: {
  skill: PiSkillEntry
  lang: Lang
  busy: string | null
  onToggle: (enabled: boolean) => Promise<void>
  onOpen: () => Promise<void>
  onRemove: () => Promise<void>
}) {
  const t = i18n[lang]
  const remove = () => {
    const message = t.externalAgentsPiSkillsDeleteConfirm.replace('{name}', skill.name)
    if (window.confirm(message)) void onRemove()
  }

  return (
    <div className="kv-row kv-pi-skill-row">
      <span className="kv-pi-skill-icon" aria-hidden="true">
        <BookOpen size={14} />
      </span>
      <div className="kv-row-text min-w-0">
        <div className="kv-pi-extension-title-line">
          <span className="kv-row-label">{skill.name}</span>
          {skill.packageSource && <span className="kv-tag mono">{skill.packageSource}</span>}
        </div>
        {skill.description && <p className="kv-row-desc">{skill.description}</p>}
        <code className="kv-pi-skill-path" title={skill.path}>{skill.path}</code>
      </div>
      <div className="kv-row-control kv-pi-extension-actions">
        <IconButton
          size="xs"
          label={t.externalAgentsPiSkillsOpen}
          disabled={busy !== null}
          onClick={() => void onOpen()}
        >
          <FolderOpen size={12} />
        </IconButton>
        {skill.canRemove && (
          <IconButton
            size="xs"
            label={t.externalAgentsPiSkillsDelete}
            disabled={busy !== null}
            onClick={remove}
          >
            <Trash2 size={12} />
          </IconButton>
        )}
        {skill.canToggle ? (
          <Toggle
            checked={skill.enabled}
            disabled={busy !== null}
            onChange={(enabled) => void onToggle(enabled)}
          />
        ) : (
          <span className="kv-tag warn">pi config</span>
        )}
      </div>
    </div>
  )
}
