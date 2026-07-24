import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ChevronDown, Check, Brain, RefreshCw, Loader2 } from 'lucide-react'
import { AgentIcon } from './AgentIcon'
import { chatApi, type DetectedExternalAgent } from './api'
import { chatTitlebarPillButtonClass } from './platform'
import { IconButton } from '../components/Button'
import { usePopoverMaxHeight } from './usePopoverMaxHeight'
import type { AgentRuntimeConfig } from './types'
import './runtimePicker.css'

const KIVIO_LOGO_SRC = '/logo-mark.png'

interface RuntimePickerProps {
  agentRuntime: AgentRuntimeConfig
  onRuntimeChange: (runtime: AgentRuntimeConfig) => void
  conversationId?: string | null
  // 会话-CLI 绑定（R3）：已产生消息的外部会话锁定运行时来源——不可换 CLI / 不可切回内置。
  // 锁定时 popover 仍可展开查看，但所有切换项 disabled 并显示提示行。
  locked?: boolean
}

const BUILTIN: AgentRuntimeConfig = {
  kind: 'builtin',
  externalAgentId: null,
  externalModel: null,
  externalReasoning: null,
}

function externalRuntime(agentId: string, model?: string | null): AgentRuntimeConfig {
  return {
    kind: 'external',
    externalAgentId: agentId,
    externalModel: model ?? 'default',
    externalReasoning: null,
  }
}

// 胶囊显示：把裸 "Default" 映射为「自动」（不再向用户暴露内部占位名）。
function mapDefaultLabel(label: string): string {
  return label === 'Default' ? 'Auto' : label
}

// 胶囊只显示模型名尾巴，去掉 provider 前缀（"foo/mimo-v2.5-pro" → "mimo-v2.5-pro"），
// 避免有意义的尾部被截断；下拉列表仍保留完整 id。
function stripProviderPrefix(label: string): string {
  const slash = label.lastIndexOf('/')
  return slash >= 0 ? label.slice(slash + 1) : label
}

// 胶囊隐藏模型名里的括号补充（"kimi (kimi-for-coding)" → "kimi"），下拉列表仍保留完整名。
function stripParenthetical(label: string): string {
  return label.replace(/\s*\([^)]*\)\s*$/, '').trim() || label
}

function RuntimePickerBase({ agentRuntime, onRuntimeChange, conversationId, locked = false }: RuntimePickerProps) {
  const [open, setOpen] = useState(false)
  const [agents, setAgents] = useState<DetectedExternalAgent[]>([])
  const [refreshing, setRefreshing] = useState(false)
  const menuRef = useRef<HTMLDivElement>(null)
  const menuMaxH = usePopoverMaxHeight(open, menuRef, 'down', 460)
  // 请求代际：conversationId 切换 / 手动刷新会并发发起检测，只让最新一次的结果落地
  // （也兜住卸载后 setState）。
  const agentsReqIdRef = useRef(0)

  const loadAgents = useCallback(
    (force: boolean) => {
      const reqId = ++agentsReqIdRef.current
      // 初次检测与手动刷新共用同一 in-flight 态：spinner + 「正在检测本机 CLI…」提示，
      // 避免首探未返回时误显示「PATH 中未发现可用 CLI」。
      setRefreshing(true)
      return chatApi
        .detectExternalAgents(force, conversationId)
        .then((list) => {
          if (agentsReqIdRef.current === reqId) setAgents(list)
          return list
        })
        .catch((err) => {
          console.error('Failed to detect external agents:', err)
          if (agentsReqIdRef.current === reqId && !force) setAgents([])
          return null
        })
        .finally(() => {
          if (agentsReqIdRef.current === reqId) setRefreshing(false)
        })
    },
    [conversationId],
  )

  useEffect(() => {
    // 每次 loadAgents 调用自身会先 ++reqId 使旧在途请求失效；卸载后 setState 在 React 18
    // 是安全 no-op，无需 cleanup 递增（避免 exhaustive-deps 对 cleanup 读 ref 的告警）。
    void loadAgents(false)
  }, [loadAgents])

  const usesExternal = agentRuntime.kind === 'external' && !!agentRuntime.externalAgentId
  const availableAgents = useMemo(
    () => agents.filter((agent) => agent.available),
    [agents],
  )
  const currentAgent = agents.find((item) => item.id === agentRuntime.externalAgentId)

  const label = useMemo(() => {
    if (!usesExternal) return '内置 Agent'
    return currentAgent?.name ?? agentRuntime.externalAgentId ?? '本地 CLI'
  }, [agentRuntime.externalAgentId, currentAgent?.name, usesExternal])

  const selectBuiltin = () => {
    if (locked) return
    onRuntimeChange(BUILTIN)
    setOpen(false)
  }

  const selectExternal = (agent: DetectedExternalAgent) => {
    if (locked) return
    if (!agent.available) return
    // 隐式契约（D3）：后端各探测路径都把合成的 "default" 占位放在 models[0]
    // （default_model_option / fallback_models 首项），因此这里取 [0] 即「让 CLI 用自己的默认模型」。
    const defaultModel = agent.models[0]?.id ?? 'default'
    onRuntimeChange(externalRuntime(agent.id, defaultModel))
    setOpen(false)
  }

  const selectLocalCliMode = () => {
    if (locked) return
    if (usesExternal && currentAgent?.available) return
    const firstAvailable = availableAgents[0]
    if (firstAvailable) {
      selectExternal(firstAvailable)
    }
  }

  return (
    <div className="kv-runtime-picker" data-tauri-drag-region="false">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className={`kv-runtime-picker__chip${open ? ' is-open' : ''}`}
        title={label}
        aria-label={label}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        {/* Icon keys off externalAgentId directly (not the detection result) so the agent
            icon shows immediately — detection is async and the list resets per conversation,
            which used to flash the Kivio logo until the first probe finished. */}
        {usesExternal && agentRuntime.externalAgentId ? (
          <AgentIcon id={agentRuntime.externalAgentId} size={18} />
        ) : (
          <img
            src={KIVIO_LOGO_SRC}
            alt=""
            aria-hidden="true"
            className="kv-runtime-picker__builtin-logo"
            width={18}
            height={18}
            draggable={false}
          />
        )}
      </button>

      {open && (
        <>
          <div className="fixed inset-0 z-10" onClick={() => setOpen(false)} aria-hidden />
          <div
            ref={menuRef}
            style={{ maxHeight: menuMaxH, overflowY: 'auto' }}
            className="kv-runtime-picker__popover chat-motion-popover"
            role="menu"
          >
            {locked && (
              <div className="kv-runtime-picker__locked-hint">
                会话已绑定当前 CLI，新建会话可切换
              </div>
            )}
            <div className="kv-runtime-picker__row">
              <span className="kv-runtime-picker__label">模式</span>
              <div className="kv-runtime-picker__seg" role="tablist">
                <button
                  type="button"
                  role="tab"
                  aria-selected={!usesExternal}
                  disabled={locked}
                  className={`kv-runtime-picker__seg-btn${!usesExternal ? ' is-active' : ''}`}
                  onClick={selectBuiltin}
                >
                  内置 Agent
                </button>
                <button
                  type="button"
                  role="tab"
                  aria-selected={usesExternal}
                  disabled={locked || availableAgents.length === 0}
                  className={`kv-runtime-picker__seg-btn${usesExternal ? ' is-active' : ''}`}
                  onClick={selectLocalCliMode}
                >
                  本地 CLI
                </button>
              </div>
            </div>

            <div className="kv-runtime-picker__row">
              <div className="kv-runtime-picker__agents-head">
                <span className="kv-runtime-picker__label">代理</span>
                <IconButton
                  size="xs"
                  variant="ghost"
                  label="刷新本机 CLI"
                  onClick={() => {
                    void loadAgents(true)
                  }}
                  disabled={refreshing}
                >
                  <RefreshCw size={13} className={refreshing ? 'animate-spin' : undefined} />
                </IconButton>
              </div>
              {agents.length === 0 ? (
                <span className="kv-runtime-picker__hint">
                  {refreshing ? '正在检测本机 CLI…' : 'PATH 中未发现可用 CLI'}
                </span>
              ) : availableAgents.length === 0 ? (
                <span className="kv-runtime-picker__hint">PATH 中未发现可用 CLI</span>
              ) : (
                <div className="kv-runtime-picker__agent-grid" role="radiogroup">
                  {availableAgents.map((agent) => {
                    const active = usesExternal && agentRuntime.externalAgentId === agent.id
                    return (
                      <button
                        key={agent.id}
                        type="button"
                        role="radio"
                        aria-checked={active}
                        disabled={locked && !active}
                        title={agent.version ?? undefined}
                        onClick={() => selectExternal(agent)}
                        className={`kv-runtime-picker__agent${active ? ' is-active' : ''}`}
                      >
                        <AgentIcon id={agent.id} size={20} />
                        <span className="kv-runtime-picker__agent-name">{agent.name}</span>
                      </button>
                    )
                  })}
                </div>
              )}
            </div>
          </div>
        </>
      )}
    </div>
  )
}

interface ExternalModelSelectorProps {
  agentRuntime: AgentRuntimeConfig
  onModelChange: (model: string, reasoning?: string | null) => void
  conversationId?: string | null
}

function ExternalModelSelectorBase({
  agentRuntime,
  onModelChange,
  conversationId,
}: ExternalModelSelectorProps) {
  const [open, setOpen] = useState(false)
  const [reasoningOpen, setReasoningOpen] = useState(false)
  const modelMenuRef = useRef<HTMLDivElement>(null)
  const modelMenuMaxH = usePopoverMaxHeight(open, modelMenuRef, 'down', 320)
  // 懒查：只探选中 agent 的模型（cwd-scoped），不再拉全量列表。保留上次结果，不清空闪。
  const [models, setModels] = useState<DetectedExternalAgent['models']>([])
  const [reasoningOptions, setReasoningOptions] = useState<
    NonNullable<DetectedExternalAgent['reasoningOptions']>
  >([])
  const [loading, setLoading] = useState(false)
  // source: probed=真实探测 / fallback=探测失败降级静态表（显示"默认列表"角标 + 重试）。
  const [source, setSource] = useState<'probed' | 'fallback'>('probed')
  // CLI 自己当前配置的模型（codex config.toml / ACP currentModelId / claude resolved）。用于胶囊
  // 显示真实名字并在用户未显式选择时自动同步；null = 该 CLI 无「当前」概念 → 显示「自动」。
  const [currentModel, setCurrentModel] = useState<string | null>(null)
  // 请求代际：agent 切换/卸载时使在途请求失效，防止旧结果覆盖新 agent 或卸载后 setState。
  const modelsReqIdRef = useRef(0)
  // 上次探测的 agent：仅在真正切换 agent 时清空 currentModel。模型列表刻意跨请求保留（防闪），
  // 但 currentModel 驱动胶囊主文案——若不清，新 agent 探测期间/失败后会一直显示上个 CLI 的模型名。
  const lastAgentIdRef = useRef<string | null>(null)
  // 用 ref 读最新 runtime / onModelChange，避免把它们塞进 loadModels 依赖导致每次选模型重探。
  const runtimeRef = useRef(agentRuntime)
  runtimeRef.current = agentRuntime
  const onModelChangeRef = useRef(onModelChange)
  onModelChangeRef.current = onModelChange

  const loadModels = useCallback(
    (agentId: string, force?: boolean) => {
      const reqId = ++modelsReqIdRef.current
      setLoading(true)
      return chatApi
        .detectExternalAgentModels(agentId, conversationId, force)
        .then((result) => {
          if (modelsReqIdRef.current !== reqId) return
          setModels(result.models)
          setReasoningOptions(result.reasoningOptions)
          setSource(result.source)
          setCurrentModel(result.currentModel ?? null)
          // 自动同步 CLI 当前配置：仅当用户未显式选择（externalModel 空 / 'default'）时。
          const rt = runtimeRef.current
          const explicitModel = !!rt.externalModel && rt.externalModel !== 'default'
          if (explicitModel) return
          const cur = result.currentModel
          // 当前模型是列表里可选的真实 id（grok / codex）才回填 externalModel；claude 的 resolved
          // model 不在别名目录里，不回填（发送仍走 CLI 默认），仅由显示层展示其名字。
          const selectable = !!cur && result.models.some((m) => m.id === cur)
          const nextModel = selectable ? (cur as string) : rt.externalModel ?? 'default'
          const explicitReasoning =
            !!rt.externalReasoning && rt.externalReasoning !== 'default'
          const nextReasoning =
            !explicitReasoning && result.currentReasoning
              ? result.currentReasoning
              : rt.externalReasoning ?? null
          if (nextModel !== (rt.externalModel ?? 'default') || nextReasoning !== (rt.externalReasoning ?? null)) {
            onModelChangeRef.current(nextModel, nextReasoning)
          }
        })
        .catch(() => {
          if (modelsReqIdRef.current !== reqId) return
          // 不再静默吞错：置为降级态，展示重试。保留上次模型列表不清空。
          setSource('fallback')
        })
        .finally(() => {
          if (modelsReqIdRef.current === reqId) setLoading(false)
        })
    },
    [conversationId],
  )

  useEffect(() => {
    const agentId = agentRuntime.externalAgentId ?? null
    if (lastAgentIdRef.current !== agentId) {
      lastAgentIdRef.current = agentId
      // 换 agent：旧 CLI 的 currentModel 立刻失效（探测中显示「获取中…」而非上个 CLI 的模型名）。
      setCurrentModel(null)
    }
    if (!agentId) {
      // 失效在途请求，防止旧结果落到已清空的状态上。
      modelsReqIdRef.current++
      setModels([])
      setReasoningOptions([])
      setSource('probed')
      return
    }
    // loadModels 自身先 ++reqId 使旧在途请求失效（agent/conversation 变更时 effect 重跑即覆盖）。
    void loadModels(agentId)
  }, [agentRuntime.externalAgentId, loadModels])

  const reasoningPillValue = agentRuntime.externalReasoning ?? 'default'
  const currentReasoningLabel = useMemo(() => {
    const opt = reasoningOptions.find((o) => o.id === reasoningPillValue)
    const raw = opt?.label ?? reasoningPillValue
    // 未显式选择推理等级（default）时显示「自动」，不再暴露裸 "Default"。
    return raw === 'Default' || reasoningPillValue === 'default' ? 'Auto' : raw
  }, [reasoningOptions, reasoningPillValue])
  const displayName = useMemo(() => {
    const currentId = agentRuntime.externalModel
    const explicit = !!currentId && currentId !== 'default'
    // 显式选择：显示所选模型 label（探测中列表未到时退回原始 id）。
    if (explicit) {
      const selected = models.find((item) => item.id === currentId)
      return stripProviderPrefix(stripParenthetical(mapDefaultLabel(selected?.label ?? currentId)))
    }
    // 未显式选择：优先显示 CLI 当前配置模型的真实名字；探测中显示「获取中…」；都没有则「自动」。
    if (currentModel) {
      const inList = models.find((item) => item.id === currentModel)
      return stripProviderPrefix(stripParenthetical(mapDefaultLabel(inList?.label ?? currentModel)))
    }
    if (loading) return '获取中…'
    return 'Auto'
  }, [agentRuntime.externalModel, models, currentModel, loading])

  if (agentRuntime.kind !== 'external' || !agentRuntime.externalAgentId) {
    return null
  }

  return (
    <div className="flex min-w-0 max-w-full items-center gap-1">
      <div className="relative min-w-0" data-tauri-drag-region="false">
        <button
          type="button"
          onClick={() => setOpen(!open)}
          className={`${chatTitlebarPillButtonClass} max-w-full min-w-0`}
        >
          <span className="max-w-[140px] truncate font-medium text-neutral-800 dark:text-neutral-200">
            {displayName}
          </span>
          {loading ? (
            <Loader2 size={14} className="shrink-0 animate-spin text-neutral-400" />
          ) : (
            <ChevronDown
              size={15}
              className={`shrink-0 text-neutral-400 transition-transform ${open ? 'rotate-180' : ''}`}
            />
          )}
        </button>
        {open && (
          <>
            <div className="fixed inset-0 z-10" onClick={() => setOpen(false)} aria-hidden />
            <div ref={modelMenuRef} style={{ maxHeight: modelMenuMaxH }} className="chat-model-selector-menu chat-motion-popover absolute left-0 top-full z-20 mt-2 min-w-[200px] overflow-y-auto rounded-2xl border border-neutral-200/90 bg-white py-1 shadow-lg dark:border-neutral-700 dark:bg-neutral-900">
              {source === 'fallback' && (
                <div className="kv-runtime-picker__fallback mx-1 my-1">
                  <span>探测失败，显示默认列表</span>
                  <button
                    type="button"
                    className="kv-runtime-picker__fallback-retry"
                    disabled={loading}
                    onClick={() => {
                      const agentId = agentRuntime.externalAgentId
                      if (agentId) void loadModels(agentId, true)
                    }}
                  >
                    重试
                  </button>
                </div>
              )}
              {models.length === 0 ? (
                <div className="px-3 py-2 text-sm text-neutral-500 dark:text-neutral-400">
                  {loading ? '正在探测模型…' : '该 CLI 未上报可用模型'}
                </div>
              ) : (
                models.map((model) => (
                  <button
                    key={model.id}
                    type="button"
                    onClick={() => {
                      onModelChange(model.id)
                      setOpen(false)
                    }}
                    className={`block w-full px-3 py-2 text-left text-sm text-neutral-700 hover:bg-neutral-100 dark:text-neutral-200 dark:hover:bg-neutral-800 ${
                      agentRuntime.externalModel === model.id ? 'font-semibold' : ''
                    }`}
                  >
                    {model.id === 'default' ? 'Auto（CLI 默认）' : model.label}
                  </button>
                ))
              )}
            </div>
          </>
        )}
      </div>

      {/* Standalone thinking-level pill, mirroring the builtin ThinkingLevelSelector. */}
      {reasoningOptions.length > 0 && (
        <div className="relative shrink-0" data-tauri-drag-region="false">
          <button
            type="button"
            onClick={() => setReasoningOpen(!reasoningOpen)}
            className={`${chatTitlebarPillButtonClass} max-w-full min-w-0`}
            title={`思考等级：${currentReasoningLabel}`}
            aria-label={`思考等级：${currentReasoningLabel}`}
          >
            <Brain size={15} className="shrink-0 text-neutral-500 dark:text-neutral-400" />
            <span className="chat-thinking-level-label max-w-[64px] truncate font-medium text-neutral-800 dark:text-neutral-200">
              {currentReasoningLabel}
            </span>
            <ChevronDown
              size={15}
              className={`shrink-0 text-neutral-400 transition-transform ${reasoningOpen ? 'rotate-180' : ''}`}
            />
          </button>
          {reasoningOpen && (
            <>
              <div
                className="fixed inset-0 z-10"
                onClick={() => setReasoningOpen(false)}
                aria-hidden
              />
              <div className="chat-model-selector-menu chat-motion-popover absolute left-0 top-full z-20 mt-2 min-w-[160px] overflow-y-auto rounded-2xl border border-neutral-200/90 bg-white py-1 shadow-lg dark:border-neutral-700 dark:bg-neutral-900">
                {reasoningOptions.map((option) => {
                  const active = option.id === reasoningPillValue
                  return (
                    <button
                      key={option.id}
                      type="button"
                      onClick={() => {
                        onModelChange(agentRuntime.externalModel ?? 'default', option.id)
                        setReasoningOpen(false)
                      }}
                      className={`flex w-full items-center justify-between gap-2 px-3 py-2 text-left text-[13px] transition-colors ${
                        active
                          ? 'bg-neutral-100 font-medium text-neutral-900 dark:bg-neutral-800 dark:text-neutral-100'
                          : 'text-neutral-700 hover:bg-neutral-50 dark:text-neutral-300 dark:hover:bg-neutral-800/80'
                      }`}
                    >
                      <span className="min-w-0 truncate">
                        {option.id === 'default' ? 'Auto（CLI 默认）' : option.label}
                      </span>
                      {active && <Check size={15} className="shrink-0 text-neutral-500" />}
                    </button>
                  )
                })}
              </div>
            </>
          )}
        </div>
      )}
    </div>
  )
}

// memo：顶栏选择器，仅在 props 变化时重渲。
export const RuntimePicker = memo(RuntimePickerBase)
export const ExternalModelSelector = memo(ExternalModelSelectorBase)
