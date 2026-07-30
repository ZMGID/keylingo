import { useCallback, useEffect, useState } from 'react'
import { RefreshCw } from 'lucide-react'
import { AgentIcon } from '../chat/AgentIcon'
import { chatApi, type DetectedExternalAgent } from '../chat/api'
import { SettingsGroup } from './components'
import { i18n, type Lang } from './i18n'
import { Button } from '../components/Button'

function authLabel(agent: DetectedExternalAgent, lang: Lang): string {
  const t = i18n[lang]
  const status = agent.authStatus ?? agent.auth_status
  if (status === 'ok') return t.externalAgentsAuthOk
  if (status === 'auth_required') return t.externalAgentsAuthRequired
  return t.externalAgentsAuthUnknown
}

interface ExternalAgentsSettingsProps {
  lang: Lang
}

export function ExternalAgentsSettings({ lang }: ExternalAgentsSettingsProps) {
  const t = i18n[lang]
  const [agents, setAgents] = useState<DetectedExternalAgent[]>([])
  const [scanning, setScanning] = useState(false)
  const [expandedId, setExpandedId] = useState<string | null>(null)

  const loadAgents = useCallback(async (force = false) => {
    setScanning(true)
    try {
      const list = await chatApi.detectExternalAgents(force)
      setAgents(list) // 先展示可用性（秒回），下面再懒查每个可用 agent 的真实模型并合并。
      // 设置页需展示每个 agent 的真实模型列表/数量：对 available agent 并行懒查（走缓存+single-flight）。
      const enriched = await Promise.all(
        list.map(async (agent) => {
          if (!agent.available) return agent
          try {
            const { models, reasoningOptions } = await chatApi.detectExternalAgentModels(
              agent.id,
              null,
              force,
            )
            return { ...agent, models, reasoningOptions }
          } catch {
            return agent
          }
        }),
      )
      setAgents(enriched)
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

  return (
    <>
      <SettingsGroup title={t.externalAgentsDetectSection}>
        <div className="mb-3 flex flex-wrap items-center justify-between gap-2 px-1">
          <Button
            size="sm"
            onClick={() => void loadAgents(true)}
            disabled={scanning}
            data-tauri-drag-region="false"
          >
            <RefreshCw size={12} className={scanning ? 'animate-spin' : ''} />
            {scanning ? t.externalAgentsRescanning : t.externalAgentsRescan}
          </Button>
        </div>

        {agents.length === 0 && !scanning ? (
          <div className="rounded-xl border border-dashed border-[var(--border)] px-4 py-6 text-center">
            <p className="text-[13px] font-medium text-neutral-800 dark:text-neutral-100">
              {t.externalAgentsNoAvailable}
            </p>
            <p className="kv-row-desc mt-1">{t.externalAgentsNoAvailableHint}</p>
          </div>
        ) : (
          <div className="flex flex-col gap-2">
            {agents.map((agent) => {
              const expanded = expandedId === agent.id
              const modelPreview = agent.models
                .slice(0, 6)
                .map((model) => model.id)
                .join(', ')
              return (
                <div
                  key={agent.id}
                  className="rounded-xl border border-[var(--border)] bg-[var(--bg-input)] px-3 py-3"
                >
                  <button
                    type="button"
                    className="flex w-full items-start gap-3 text-left"
                    onClick={() => setExpandedId(expanded ? null : agent.id)}
                  >
                    <AgentIcon id={agent.id} size={28} />
                    <div className="min-w-0 flex-1">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="text-[14px] font-medium text-neutral-900 dark:text-neutral-50">
                          {agent.name}
                        </span>
                        <span className={`kv-tag ${agent.available ? 'ok' : ''}`}>
                          {agent.available
                            ? t.externalAgentsInstalled
                            : t.externalAgentsNotInstalled}
                        </span>
                        {agent.available && (
                          <span className="kv-row-desc text-[11px]">
                            {t.externalAgentsModelsCount.replace(
                              '{count}',
                              String(agent.models.length),
                            )}
                          </span>
                        )}
                      </div>
                      {agent.available && (
                        <p className="kv-row-desc mt-1 truncate">
                          {authLabel(agent, lang)}
                          {agent.version ? ` · ${agent.version}` : ''}
                        </p>
                      )}
                      {agent.id === 'cursor' && agent.available && (
                        <p className="kv-row-desc mt-1">{t.externalAgentsCursorToolLimit}</p>
                      )}
                    </div>
                  </button>
                  {expanded && (
                    <div className="mt-3 border-t border-[var(--divider)] pt-3 text-[12px] text-[var(--text-muted)]">
                      {agent.path && (
                        <p className="break-all">
                          <span className="font-medium">{t.externalAgentsPath}: </span>
                          {agent.path}
                        </p>
                      )}
                      {agent.version && (
                        <p className="mt-1">
                          <span className="font-medium">{t.externalAgentsVersion}: </span>
                          {agent.version}
                        </p>
                      )}
                      {modelPreview && (
                        <p className="mt-1 break-all">
                          <span className="font-medium">{t.externalAgentsDefaultModel}: </span>
                          {modelPreview}
                          {agent.models.length > 6 ? '…' : ''}
                        </p>
                      )}
                    </div>
                  )}
                </div>
              )
            })}
          </div>
        )}
      </SettingsGroup>
    </>
  )
}
