import { useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import { api, isTauriRuntime, type ChatToolDefinition, type ModelProvider } from '../../api/tauri'
import { getSettingsCached } from '../../api/settingsCache'
import { FieldBlock, Select } from '../../settings/components'
import { isProviderEnabled, type SelectOption } from '../../settings/utils'
import { useT } from '../../settings/i18n'
import { chatApi, type DetectedExternalAgent } from '../api'
import { AgentIcon } from '../AgentIcon'
import { normalizeAgent, toAgentData, collapseToolIds, withRuntimeKind, type NormalizedAgent } from './agentModel'
import type { AgentSlot, FlowNode } from './types'

function Section({
  id,
  title,
  required,
  focused,
  children,
}: {
  id: AgentSlot
  title: string
  required?: boolean
  focused: boolean
  children: ReactNode
}) {
  const ref = useRef<HTMLElement>(null)
  useEffect(() => {
    if (focused) ref.current?.scrollIntoView({ block: 'nearest' })
  }, [focused])
  return (
    <section
      ref={ref}
      id={`kv-agent-slot-${id}`}
      className={`kv-automation-inspector-section${focused ? ' is-focused' : ''}`}
    >
      <h3>
        {title}
        {required ? <span className="kv-automation-slot-req">*</span> : null}
      </h3>
      {children}
    </section>
  )
}

function modelOptions(providers: ModelProvider[]): SelectOption[] {
  const options: SelectOption[] = []
  for (const provider of providers.filter(isProviderEnabled)) {
    const models = provider.enabledModels.length > 0 ? provider.enabledModels : provider.availableModels
    for (const model of models) {
      options.push({
        value: `${provider.id}:${model}`,
        label: model,
        title: `${provider.name} / ${model}`,
      })
    }
  }
  return options
}

function withOrphanOption(options: SelectOption[], value: string): SelectOption[] {
  if (!value || options.some((option) => option.value === value)) return options
  const orphan = { value, label: value }
  if (options[0]?.value === '') return [options[0], orphan, ...options.slice(1)]
  return [orphan, ...options]
}

export function AgentInspector({
  node,
  onChange,
  slot,
}: {
  node: FlowNode
  onChange: (next: FlowNode) => void
  slot: AgentSlot
}) {
  const t = useT()
  const agent = normalizeAgent(node.data.agent)
  const patch = (next: NormalizedAgent, label?: string) =>
    onChange({
      ...node,
      data: {
        ...node.data,
        ...(label ? { label } : {}),
        agent: toAgentData(next),
      },
    })
  const [providers, setProviders] = useState<ModelProvider[]>([])
  const [cliAgents, setCliAgents] = useState<DetectedExternalAgent[]>([])
  const [cliModels, setCliModels] = useState<Array<{ id: string, label: string }>>([])
  const [cliCurrentModel, setCliCurrentModel] = useState<string | null>(null)
  const [tools, setTools] = useState<ChatToolDefinition[]>([])
  const [skills, setSkills] = useState<Array<{ id: string, name: string, description?: string }>>([])

  useEffect(() => {
    let cancelled = false
    void getSettingsCached()
      .then((settings) => {
        if (!cancelled) setProviders(settings.providers || [])
      })
      .catch(() => {})
    if (!isTauriRuntime()) return () => { cancelled = true }
    void chatApi.detectExternalAgents(false).then((list) => {
      if (!cancelled) setCliAgents(list.filter((agent) => agent.available && !agent.disabled))
    }).catch(() => {})
    void api.chatMcpListTools().then((result) => {
      if (!cancelled) setTools(result.tools ?? [])
    }).catch(() => {})
    void api.chatSkillsList().then((result) => {
      if (!cancelled) {
        setSkills((result.skills ?? []).map((skill) => ({
          id: skill.id,
          name: skill.name,
          description: skill.description,
        })))
      }
    }).catch(() => {})
    return () => { cancelled = true }
  }, [])

  useEffect(() => {
    if (agent.runtimeKind !== 'external' || !agent.externalAgentId || !isTauriRuntime()) {
      setCliModels([])
      setCliCurrentModel(null)
      return
    }
    let cancelled = false
    void chatApi.detectExternalAgentModels(agent.externalAgentId).then((result) => {
      if (cancelled) return
      setCliModels(result.models ?? [])
      setCliCurrentModel(result.currentModel ?? null)
    }).catch(() => {})
    return () => { cancelled = true }
  }, [agent.runtimeKind, agent.externalAgentId])

  const kivioModelValue = agent.providerId && agent.model
    ? `${agent.providerId}:${agent.model}`
    : ''
  const kivioModels = useMemo(
    () => withOrphanOption(
      [{ value: '', label: t.chatAutomationAgentModelDefault }, ...modelOptions(providers)],
      kivioModelValue,
    ),
    [providers, kivioModelValue, t.chatAutomationAgentModelDefault],
  )
  const cliModelOptions = useMemo(
    () => withOrphanOption(
      [
        {
          value: '',
          label: t.chatRuntimeAutoCliDefault,
          title: cliCurrentModel
            ? `${t.chatRuntimeAutoCliDefault} · ${cliCurrentModel}`
            : t.chatRuntimeAutoCliDefault,
        },
        ...cliModels.map((model) => ({ value: model.id, label: model.label || model.id })),
      ],
      agent.externalModel ?? '',
    ),
    [agent.externalModel, cliCurrentModel, cliModels, t.chatRuntimeAutoCliDefault],
  )

  const allToolIds = useMemo(
    () => tools.map((tool) => tool.id || tool.name).filter(Boolean),
    [tools],
  )
  const toggleTool = (id: string) => {
    const current = agent.toolIds.length === 0 ? allToolIds : agent.toolIds
    const next = current.includes(id)
      ? current.filter((item) => item !== id)
      : [...current, id]
    patch({ ...agent, toolIds: collapseToolIds(next, allToolIds) })
  }
  const toggleSkill = (id: string) => {
    const next = agent.skillIds.includes(id)
      ? agent.skillIds.filter((item) => item !== id)
      : [...agent.skillIds, id]
    patch({ ...agent, skillIds: next })
  }

  return (
    <>
      {slot === 'runtime' ? (
      <Section
        id="runtime"
        title={t.chatAutomationAgentSlotRuntime}
        required
        focused={false}
      >
        <div className="kv-automation-runtime-grid" role="radiogroup">
          <button
            type="button"
            role="radio"
            aria-checked={agent.runtimeKind === 'builtin'}
            className={`kv-automation-runtime-chip${agent.runtimeKind === 'builtin' ? ' is-active' : ''}`}
            onClick={() => patch(withRuntimeKind(agent, 'builtin'), t.chatAutomationKivioAgent)}
          >
            {t.chatAutomationKivioAgent}
          </button>
          <button
            type="button"
            role="radio"
            aria-checked={agent.runtimeKind === 'chat'}
            className={`kv-automation-runtime-chip${agent.runtimeKind === 'chat' ? ' is-active' : ''}`}
            onClick={() => patch(withRuntimeKind(agent, 'chat'), t.chatAutomationKivioChat)}
          >
            {t.chatAutomationKivioChat}
          </button>
          {cliAgents.map((cli) => (
            <button
              key={cli.id}
              type="button"
              role="radio"
              aria-checked={agent.runtimeKind === 'external' && agent.externalAgentId === cli.id}
              className={`kv-automation-runtime-chip${agent.runtimeKind === 'external' && agent.externalAgentId === cli.id ? ' is-active' : ''}`}
              onClick={() => patch(
                withRuntimeKind(agent, 'external', {
                  externalAgentId: cli.id,
                  externalModel: null,
                }),
                cli.name,
              )}
            >
              <AgentIcon id={cli.id} size={14} />
              {cli.name}
            </button>
          ))}
        </div>
        {agent.runtimeKind !== 'external' ? (
          <FieldBlock label={t.chatAutomationAgentModel}>
            <Select
              value={kivioModelValue}
              onChange={(value) => {
                if (!value) {
                  patch(withRuntimeKind({ ...agent, providerId: null, model: null }, agent.runtimeKind))
                  return
                }
                const idx = value.indexOf(':')
                patch(withRuntimeKind({
                  ...agent,
                  providerId: value.slice(0, idx),
                  model: value.slice(idx + 1),
                }, agent.runtimeKind))
              }}
              options={kivioModels}
            />
          </FieldBlock>
        ) : (
          <FieldBlock label={t.chatAutomationAgentModel}>
            <Select
              value={agent.externalModel ?? ''}
              onChange={(value) => patch(withRuntimeKind(
                { ...agent, externalModel: value || null },
                'external',
              ))}
              options={cliModelOptions}
            />
          </FieldBlock>
        )}
      </Section>
      ) : null}

      {slot === 'context' ? (
      <Section
        id="context"
        title={t.chatAutomationAgentSlotContext}
        required
        focused={false}
      >
        <textarea
          className="kv-textarea"
          rows={7}
          value={agent.prompt}
          placeholder={t.chatAutomationNodePromptPlaceholder}
          onChange={(event) => patch({ ...agent, prompt: event.target.value })}
        />
        <p className="kv-automation-inspector-note">{t.chatAutomationVarsHint}</p>
      </Section>
      ) : null}

      {slot === 'tool' ? (
      <Section
        id="tool"
        title={t.chatAutomationAgentSlotTool}
        focused={false}
      >
        <p className="kv-automation-inspector-note">
          {agent.runtimeKind === 'external'
            ? t.chatAutomationAgentToolsCliHint
            : t.chatAutomationAgentToolsHint}
        </p>
        {agent.runtimeKind !== 'external' ? (
          <div className="kv-automation-check-list">
            {tools.length === 0 ? (
              <p className="kv-automation-inspector-note">{t.chatAutomationAgentToolsEmpty}</p>
            ) : tools.map((tool) => {
              const id = tool.id || tool.name
              return (
                <label key={id} className="kv-automation-check">
                  <input
                    type="checkbox"
                    checked={agent.toolIds.length === 0 || agent.toolIds.includes(id)}
                    onChange={() => toggleTool(id)}
                  />
                  <span>
                    <span className="kv-automation-check-title">{tool.name}</span>
                    {tool.serverName ? (
                      <span className="kv-automation-check-hint">{tool.serverName}</span>
                    ) : null}
                  </span>
                </label>
              )
            })}
          </div>
        ) : null}
      </Section>
      ) : null}

      {slot === 'skill' ? (
      <Section
        id="skill"
        title={t.chatAutomationAgentSlotSkill}
        focused={false}
      >
        <p className="kv-automation-inspector-note">{t.chatAutomationAgentSkillsHint}</p>
        <div className="kv-automation-check-list">
          {skills.length === 0 ? (
            <p className="kv-automation-inspector-note">{t.chatAutomationAgentSkillsEmpty}</p>
          ) : skills.map((skill) => (
            <label key={skill.id} className="kv-automation-check">
              <input
                type="checkbox"
                checked={agent.skillIds.includes(skill.id)}
                onChange={() => toggleSkill(skill.id)}
              />
              <span>
                <span className="kv-automation-check-title">{skill.name}</span>
                {skill.description ? (
                  <span className="kv-automation-check-hint">{skill.description}</span>
                ) : null}
              </span>
            </label>
          ))}
        </div>
      </Section>
      ) : null}
    </>
  )
}
