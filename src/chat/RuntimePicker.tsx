import { memo, useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ChevronDown, Check, Brain, RefreshCw } from 'lucide-react'
import { AgentIcon } from './AgentIcon'
import { useT } from '../settings/i18n'
import { chatApi, type DetectedExternalAgent } from './api'
import { chatTitlebarPillButtonClass } from './platform'
import { IconButton } from '../components/Button'
import { usePopoverMaxHeight } from './usePopoverMaxHeight'
import type { AgentRuntimeConfig } from './types'
import { rememberedExternalRuntime } from './lastAgentRuntime'
import './runtimePicker.css'

const KIVIO_LOGO_SRC = '/logo-mark.png'

/** Same brand mark as Agent; `variant` only changes color treatment so the shape stays identical. */
function KivioMark({
  size = 20,
  variant = 'agent',
}: {
  size?: number
  variant?: 'agent' | 'chat'
}) {
  return (
    <img
      src={KIVIO_LOGO_SRC}
      alt=""
      aria-hidden="true"
      className={
        variant === 'chat'
          ? 'kv-runtime-picker__builtin-logo kv-runtime-picker__builtin-logo--chat'
          : 'kv-runtime-picker__builtin-logo'
      }
      width={size}
      height={size}
      draggable={false}
    />
  )
}

interface RuntimePickerProps {
  agentRuntime: AgentRuntimeConfig
  onRuntimeChange: (runtime: AgentRuntimeConfig) => void
  conversationId?: string | null
  // 一 agent 一对话：已有消息的会话锁定运行时来源（内置 / 本地 CLI 均不可换）。
  // 锁定时 popover 仍可展开查看，但所有切换项 disabled 并显示提示行。
  locked?: boolean
}

const BUILTIN: AgentRuntimeConfig = {
  kind: 'builtin',
  externalAgentId: null,
  externalModel: null,
  externalReasoning: null,
}

const CHAT: AgentRuntimeConfig = {
  kind: 'chat',
  externalAgentId: null,
  externalModel: null,
  externalReasoning: null,
}

// 胶囊显示：把裸 "Default" 映射为「自动」（不再向用户暴露内部占位名）。
function mapDefaultLabel(label: string): string {
  return label === 'Default' ? 'Auto' : label
}

/** ACP 探测常把思考开关报成 on/off，那不是档位。dsh 的 `off` 是真档位，会跟 high/max 一起出现。 */
const ACP_SWITCH_IDS = ['on', 'off', 'true', 'false', 'enabled', 'disabled']

function isAcpSwitchId(id: string): boolean {
  return ACP_SWITCH_IDS.includes(id.toLowerCase())
}

function isAcpSwitchOnlyList(options: { id: string }[]): boolean {
  return options.length > 0 && options.every((option) => isAcpSwitchId(option.id))
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
  const t = useT()
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
  const usesChat = agentRuntime.kind === 'chat'
  const usesBuiltinAgent = agentRuntime.kind === 'builtin' || (!usesExternal && !usesChat)
  const availableAgents = useMemo(
    // 设置页停用的不出现在这里；已经绑定它的旧会话照常（currentAgent 走的是全量 agents）。
    () => agents.filter((agent) => agent.available && !agent.disabled),
    [agents],
  )
  const currentAgent = agents.find((item) => item.id === agentRuntime.externalAgentId)

  const label = useMemo(() => {
    if (usesExternal) return currentAgent?.name ?? agentRuntime.externalAgentId ?? t.chatRuntimeLocalCli
    if (usesChat) return 'Kivio Chat'
    return 'Kivio Agent'
  }, [agentRuntime.externalAgentId, currentAgent?.name, t, usesChat, usesExternal])

  const selectBuiltin = () => {
    if (locked) return
    onRuntimeChange(BUILTIN)
    setOpen(false)
  }

  const selectChat = () => {
    if (locked) return
    onRuntimeChange(CHAT)
    setOpen(false)
  }

  const selectExternal = (agent: DetectedExternalAgent) => {
    if (locked) return
    if (!agent.available) return
    // 已选中的代理再点一次只关菜单。重发 default 会把用户刚选的模型和思考档清成 Auto。
    if (agentRuntime.kind === 'external' && agentRuntime.externalAgentId === agent.id) {
      setOpen(false)
      return
    }
    // 换代理时带回该 CLI 上次的模型/思考档。没有记录才走 default（胶囊显示 Auto）。
    onRuntimeChange(rememberedExternalRuntime(agent.id))
    setOpen(false)
  }

  return (
    <div className="kv-runtime-picker" data-tauri-drag-region="false">
      <button
        type="button"
        onClick={() => setOpen(!open)}
        className={`kv-runtime-picker__chip${open ? ' is-open' : ''}`}
        title={locked ? `${label} · ${t.chatRuntimeBoundHint}` : label}
        aria-label={locked ? `${label} · ${t.chatRuntimeBoundHint}` : label}
        aria-haspopup="menu"
        aria-expanded={open}
      >
        {/* Icon keys off externalAgentId directly (not the detection result) so the agent
            icon shows immediately — detection is async and the list resets per conversation,
            which used to flash the Kivio logo until the first probe finished. */}
        {usesExternal && agentRuntime.externalAgentId ? (
          <AgentIcon id={agentRuntime.externalAgentId} size={18} />
        ) : (
          <KivioMark size={18} variant={usesChat ? 'chat' : 'agent'} />
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
            <div className="kv-runtime-picker__row">
              <div className="kv-runtime-picker__agents-head">
                <span className="kv-runtime-picker__label">{t.chatRuntimeAgent}</span>
                {locked && (
                  <span className="kv-runtime-picker__bound-note" title={t.chatRuntimeBoundHint}>
                    {t.chatRuntimeBoundShort}
                  </span>
                )}
                <IconButton
                  size="xs"
                  variant="ghost"
                  label={t.chatRuntimeRefresh}
                  onClick={() => {
                    void loadAgents(true)
                  }}
                  disabled={refreshing}
                >
                  <RefreshCw size={13} className={refreshing ? 'animate-spin' : undefined} />
                </IconButton>
              </div>
              {/* Kivio 自己就是一个代理，和本机 CLI 同列平铺（原先上面还有一行「模式」分段器，
                  内置/本地 CLI 二选一 —— 两级选择表达的是同一件事，去掉一级）。 */}
              <div className="kv-runtime-picker__agent-grid" role="radiogroup">
                <button
                  type="button"
                  role="radio"
                  aria-checked={usesBuiltinAgent}
                  disabled={locked && !usesBuiltinAgent}
                  onClick={selectBuiltin}
                  className={`kv-runtime-picker__agent${usesBuiltinAgent ? ' is-active' : ''}`}
                >
                  <KivioMark size={20} variant="agent" />
                  <span className="kv-runtime-picker__agent-name">Kivio Agent</span>
                </button>
                <button
                  type="button"
                  role="radio"
                  aria-checked={usesChat}
                  disabled={locked && !usesChat}
                  onClick={selectChat}
                  className={`kv-runtime-picker__agent${usesChat ? ' is-active' : ''}`}
                >
                  <KivioMark size={20} variant="chat" />
                  <span className="kv-runtime-picker__agent-name">Kivio Chat</span>
                </button>
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
              {availableAgents.length === 0 && (
                <span className="kv-runtime-picker__hint">
                  {refreshing ? t.chatRuntimeDetecting : t.chatRuntimeNoneFound}
                </span>
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
  const t = useT()
  const [open, setOpen] = useState(false)
  const [reasoningOpen, setReasoningOpen] = useState(false)
  const modelMenuRef = useRef<HTMLDivElement>(null)
  const modelMenuMaxH = usePopoverMaxHeight(open, modelMenuRef, 'down', 320)
  // 懒查：只探选中 agent 的模型（cwd-scoped），不再拉全量列表。保留上次结果，不清空闪。
  const [models, setModels] = useState<DetectedExternalAgent['models']>([])
  const [reasoningOptions, setReasoningOptions] = useState<
    NonNullable<DetectedExternalAgent['reasoningOptions']>
  >([])
  // kimi 等：档位随模型变（K3=low/high/max，K2.7 always_thinking=无）。按所选模型取。
  const [reasoningByModel, setReasoningByModel] = useState<
    Record<string, NonNullable<DetectedExternalAgent['reasoningOptions']>>
  >({})
  const [loading, setLoading] = useState(false)
  // source: probed=真实探测 / fallback=探测失败降级静态表（显示"默认列表"角标 + 重试）。
  const [source, setSource] = useState<'probed' | 'fallback'>('probed')
  // CLI 自己当前配置的模型（codex config.toml / ACP currentModelId / claude resolved）。用于胶囊
  // 显示真实名字并在用户未显式选择时自动同步；null = 该 CLI 无「当前」概念 → 显示「自动」。
  const [currentModel, setCurrentModel] = useState<string | null>(null)
  const [currentReasoning, setCurrentReasoning] = useState<string | null>(null)
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
          setReasoningByModel(result.reasoningByModel ?? {})
          setSource(result.source)
          setCurrentModel(result.currentModel ?? null)
          setCurrentReasoning(result.currentReasoning ?? null)
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
          // 回填当前档位：只认「确实在该模型档位列表里」的值。改前是硬编码 on/off 黑名单——
          // 判据和真相源分了岔（真相源是 reasoningByModel / reasoningOptions），
          // 将来哪个 CLI 真把 On/Off 当合法档位就会被误杀成「自动」。按列表判即可。
          const probedReasoning = result.currentReasoning
          const activeOpts =
            (cur && result.reasoningByModel?.[cur]) || result.reasoningOptions
          const saneReasoning =
            probedReasoning && activeOpts.some((o) => o.id === probedReasoning)
              ? probedReasoning
              : null
          const nextReasoning = !explicitReasoning && saneReasoning
            ? saneReasoning
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
      setCurrentReasoning(null)
      setReasoningByModel({})
    }
    if (!agentId) {
      // 失效在途请求，防止旧结果落到已清空的状态上。
      modelsReqIdRef.current++
      setModels([])
      setReasoningOptions([])
      setReasoningByModel({})
      setSource('probed')
      return
    }
    // loadModels 自身先 ++reqId 使旧在途请求失效（agent/conversation 变更时 effect 重跑即覆盖）。
    void loadModels(agentId)
  }, [agentRuntime.externalAgentId, loadModels])

  // 当前展示用的 effort 列表：优先按所选/当前模型从 reasoningByModel 取（kimi 按模型变档）。
  const activeReasoningOptions = useMemo(() => {
    const modelId =
      agentRuntime.externalModel && agentRuntime.externalModel !== 'default'
        ? agentRuntime.externalModel
        : currentModel
    if (modelId && Object.prototype.hasOwnProperty.call(reasoningByModel, modelId)) {
      return reasoningByModel[modelId] ?? []
    }
    // 整表都是 ACP 开关才藏档位。不要从混有 high/max 的表里单独抠掉 off——dsh 的 off 是真档位。
    if (isAcpSwitchOnlyList(reasoningOptions)) {
      return []
    }
    return reasoningOptions
  }, [agentRuntime.externalModel, currentModel, reasoningByModel, reasoningOptions])

  const reasoningPillValue = agentRuntime.externalReasoning ?? 'default'
  const currentReasoningLabel = useMemo(() => {
    const explicit =
      !!agentRuntime.externalReasoning && agentRuntime.externalReasoning !== 'default'
    // 未显式选择时跟模型胶囊同一口径：优先展示 CLI 实际在用的档位，而不是「Auto」。
    const displayId = explicit
      ? reasoningPillValue
      : currentReasoning && currentReasoning !== 'default'
        ? currentReasoning
        : reasoningPillValue
    const opt = activeReasoningOptions.find((o) => o.id === displayId)
    if (opt) {
      return mapDefaultLabel(opt.label || displayId)
    }
    const raw = displayId
    // 未显式选择且探测也没有档位时显示「自动」，不再暴露裸 "Default"。
    // 残留的 ACP on/off 开关也当自动（不是档位名）。列表里的 off（dsh）走上面的 opt 分支。
    if (
      raw === 'Default' ||
      displayId === 'default' ||
      isAcpSwitchId(String(displayId))
    ) {
      return 'Auto'
    }
    return raw
  }, [activeReasoningOptions, agentRuntime.externalReasoning, currentReasoning, reasoningPillValue])
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
    if (loading) return t.chatRuntimeFetching
    return 'Auto'
  }, [agentRuntime.externalModel, models, currentModel, loading, t])

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
          {/* ponytail: 探测中复用已有的 shimmer 文字动画，不再转圈；chevron 常驻避免宽度跳动 */}
          <span
            className={`max-w-[140px] truncate font-medium ${loading ? 'reasoning-shimmer-text' : 'text-neutral-800 dark:text-neutral-200'}`}
          >
            {displayName}
          </span>
          <ChevronDown
            size={15}
            className={`shrink-0 transition-transform ${loading ? 'text-neutral-300 dark:text-neutral-600' : 'text-neutral-400'} ${open ? 'rotate-180' : ''}`}
          />
        </button>
        {open && (
          <>
            <div className="fixed inset-0 z-10" onClick={() => setOpen(false)} aria-hidden />
            <div ref={modelMenuRef} style={{ maxHeight: modelMenuMaxH }} className="chat-model-selector-menu chat-motion-popover absolute left-0 top-full z-20 mt-2 min-w-[200px] overflow-y-auto kv-menu">
              {source === 'fallback' && (
                <div className="kv-runtime-picker__fallback mx-1 my-1">
                  <span>{t.chatRuntimeProbeFailed}</span>
                  <button
                    type="button"
                    className="kv-runtime-picker__fallback-retry"
                    disabled={loading}
                    onClick={() => {
                      const agentId = agentRuntime.externalAgentId
                      if (agentId) void loadModels(agentId, true)
                    }}
                  >
                    {t.chatRetry}
                  </button>
                </div>
              )}
              {models.length === 0 ? (
                <div
                  className={`kv-menu-row ${loading ? 'reasoning-shimmer-text' : 'text-neutral-500 dark:text-neutral-400'}`}
                >
                  {loading ? t.chatRuntimeProbingModels : t.chatRuntimeNoModels}
                </div>
              ) : (
                models.map((model) => (
                  <button
                    key={model.id}
                    type="button"
                    onClick={() => {
                      // 换模型时 effort 可能变（kimi K3 有 low/high/max，K2.7 无）。
                      // 旧值是 on/off 或不在新列表里 → 清掉或落到 high。
                      const efforts = reasoningByModel[model.id]
                      const cur = agentRuntime.externalReasoning
                      // 同上：只看「在不在新模型的档位列表里」，不再额外拉黑 on/off。
                      const curOk =
                        !!cur &&
                        cur !== 'default' &&
                        (!efforts || efforts.some((o) => o.id === cur))
                      let nextReasoning: string | null = curOk ? (cur as string) : null
                      if (!curOk && efforts && efforts.length > 0) {
                        nextReasoning =
                          efforts.find((o) => o.id === 'high')?.id ?? efforts[0]?.id ?? null
                      }
                      if (efforts && efforts.length === 0) nextReasoning = null
                      onModelChange(model.id, nextReasoning)
                      setOpen(false)
                    }}
                    className={`kv-menu-row text-neutral-700 hover:bg-black/[0.05] dark:text-neutral-200 dark:hover:bg-white/[0.07] ${
                      agentRuntime.externalModel === model.id ? 'font-semibold' : ''
                    }`}
                  >
                    {model.id === 'default' ? t.chatRuntimeAutoCliDefault : model.label}
                  </button>
                ))
              )}
            </div>
          </>
        )}
      </div>

      {/* Standalone thinking-level pill, mirroring the builtin ThinkingLevelSelector. */}
      {activeReasoningOptions.length > 0 && (
        <div className="relative shrink-0" data-tauri-drag-region="false">
          <button
            type="button"
            onClick={() => setReasoningOpen(!reasoningOpen)}
            className={`${chatTitlebarPillButtonClass} max-w-full min-w-0`}
            title={t.chatThinkingLevel.replace('{level}', currentReasoningLabel)}
            aria-label={t.chatThinkingLevel.replace('{level}', currentReasoningLabel)}
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
              <div className="chat-model-selector-menu chat-motion-popover absolute left-0 top-full z-20 mt-2 min-w-[160px] overflow-y-auto kv-menu">
                {activeReasoningOptions.map((option) => {
                  const active = option.id === reasoningPillValue
                  return (
                    <button
                      key={option.id}
                      type="button"
                      onClick={() => {
                        onModelChange(agentRuntime.externalModel ?? 'default', option.id)
                        setReasoningOpen(false)
                      }}
                      className={`kv-menu-row justify-between transition-colors ${
                        active
                          ? 'bg-neutral-100 font-medium text-neutral-900 dark:bg-neutral-800 dark:text-neutral-100'
                          : 'text-neutral-700 hover:bg-neutral-50 dark:text-neutral-300 dark:hover:bg-neutral-800/80'
                      }`}
                    >
                      <span className="min-w-0 truncate">
                        {option.id === 'default' ? t.chatRuntimeAutoCliDefault : option.label}
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
