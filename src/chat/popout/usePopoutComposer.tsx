import { useCallback, useEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction } from 'react'
import {
  api,
  builtinWebSearchSupported,
  isTauriRuntime,
  type ChatMcpServer,
  type ChatToolDefinition,
} from '../../api/tauri'
import { getSettingsCached, refreshSettings, saveSettingsCached, subscribeSettings } from '../../api/settingsCache'
import { isPluginManagedServer, preservePluginManagedServers } from '../../settings/connectorCatalog'
import { i18n, type Lang } from '../../settings/i18n'
import { hasEnabledNativeBuiltinTool, hasEnabledSkillRuntime } from '../../utils/chatTools'
import { chatApi, type AgentRuntimeConfig } from '../api'
import { mergeCompactionContextState } from '../compactionBoundary'
import { applyLiveContextUsage } from '../contextPanel'
import { mergeClearContextState } from '../contextClearBoundary'
import { ContextIndicator } from '../ContextIndicator'
import { dockApi } from '../dock/api'
import { useTauriEvent } from '../hooks/useTauriEvent'
import type { InputBarProps } from '../InputBar'
import {
  deriveDshPresetModes,
  derivePermissionModes,
  useDetectedExternalAgents,
  useDshCustomPresets,
} from '../permissionModes'
import { SessionUsageStrip } from '../SessionUsageStrip'
import type {
  AdditionalDirectory,
  AgentPlanMode,
  ChatAssistant,
  ChatProject,
  ChatSet,
  Conversation,
  ConversationContextState,
  ModelRef,
  SkillMeta,
  WebSearchMode,
} from '../types'

const LAST_WEB_SEARCH_MODE_KEY = 'kivio.chat.lastWebSearchMode'
const VALID_WEB_SEARCH_MODES: ReadonlySet<string> = new Set(['off', 'builtin', 'third_party'])

function loadLastWebSearchMode(): WebSearchMode | undefined {
  try {
    const raw = window.localStorage.getItem(LAST_WEB_SEARCH_MODE_KEY)
    return raw && VALID_WEB_SEARCH_MODES.has(raw) ? (raw as WebSearchMode) : undefined
  } catch {
    return undefined
  }
}

function saveLastWebSearchMode(mode: WebSearchMode): void {
  try {
    window.localStorage.setItem(LAST_WEB_SEARCH_MODE_KEY, mode)
  } catch {
    /* ignore */
  }
}

function additionalDirectoriesOf(conversation: Conversation | null | undefined): AdditionalDirectory[] {
  return conversation?.additional_directories ?? conversation?.additionalDirectories ?? []
}

function skillRecommendedTools(skill?: SkillMeta | null): string[] {
  return skill?.recommended_tools ?? skill?.recommendedTools ?? []
}

function normalizeSkill(skill: import('../../api/tauri').SkillMeta): SkillMeta {
  return {
    id: skill.id,
    name: skill.name,
    description: skill.description,
    source: skill.source,
    path: skill.path ?? undefined,
    recommendedTools: skill.recommendedTools,
    disableModelInvocation: skill.disableModelInvocation,
    files: skill.files,
  }
}

function toolMatchesRecommendation(tool: ChatToolDefinition, recommended: string): boolean {
  const name = recommended.trim()
  if (!name) return false
  return (
    tool.name === name
    || tool.id === name
    || `${tool.serverId ?? ''}:${tool.name}` === name
  )
}

function isBlankConversation(conversation: Conversation | null): boolean {
  return Boolean(
    conversation
    && conversation.messages.length === 0
    && !(conversation.assistant_id ?? conversation.assistantId),
  )
}

function applyConversationMeta(
  setConversation: Dispatch<SetStateAction<Conversation | null>>,
  updated: Conversation,
) {
  setConversation((prev) => {
    if (!prev || prev.id !== updated.id) return updated
    if (updated.revision < prev.revision) return prev
    return { ...updated, messages: prev.messages }
  })
}

type UsePopoutComposerArgs = {
  conversation: Conversation | null
  setConversation: Dispatch<SetStateAction<Conversation | null>>
  conversationId: string
  lang: Lang
  displayMessages: Conversation['messages']
  streaming: boolean
  usesChatRuntime: boolean
  usesExternalRuntime: boolean
  runtime: AgentRuntimeConfig
  onSend: InputBarProps['onSend']
  onCancel: () => void
  cancelVisible: boolean
  cancelling: boolean
  disabled: boolean
}

export function usePopoutComposer({
  conversation,
  setConversation,
  conversationId,
  lang,
  displayMessages,
  streaming,
  usesChatRuntime,
  usesExternalRuntime,
  runtime,
  onSend,
  onCancel,
  cancelVisible,
  cancelling,
  disabled,
}: UsePopoutComposerArgs): InputBarProps {
  const conversationIdRef = useRef(conversationId)
  conversationIdRef.current = conversationId

  const [contextState, setContextState] = useState<ConversationContextState | null>(
    () => conversation?.context_state ?? conversation?.contextState ?? null,
  )
  const [contextLoading, setContextLoading] = useState(false)
  const [contextError, setContextError] = useState('')
  const [contextCompressing, setContextCompressing] = useState(false)
  const [enabledTools, setEnabledTools] = useState<ChatToolDefinition[]>([])
  const [enabledToolCount, setEnabledToolCount] = useState<number | null>(null)
  const [toolsDisabledReason, setToolsDisabledReason] = useState('')
  const [toolsRequested, setToolsRequested] = useState(false)
  const [mcpServers, setMcpServers] = useState<ChatMcpServer[]>([])
  const [webSearchEnabled, setWebSearchEnabled] = useState(true)
  const [providerApiFormats, setProviderApiFormats] = useState<Record<string, string>>({})
  const [providerBaseUrls, setProviderBaseUrls] = useState<Record<string, string>>({})
  const [skills, setSkills] = useState<SkillMeta[]>([])
  const [disabledSkillIds, setDisabledSkillIds] = useState<string[]>([])
  const [boundProject, setBoundProject] = useState<ChatProject | null>(null)
  const [boundSet, setBoundSet] = useState<ChatSet | null>(null)
  const [gitWorkdir, setGitWorkdir] = useState('')

  const detectedExternalAgents = useDetectedExternalAgents(conversationId)
  const dshCustomPresets = useDshCustomPresets(runtime)
  const activeAgentPlanMode = conversation?.agent_plan_state?.mode
    ?? conversation?.agentPlanState?.mode
    ?? 'act'
  const composerModes = useMemo(
    () => derivePermissionModes({
      target: 'composer',
      agentRuntime: runtime,
      agents: detectedExternalAgents,
      agentPlanMode: activeAgentPlanMode,
    }),
    [runtime, detectedExternalAgents, activeAgentPlanMode],
  )
  const composerPresets = useMemo(
    () => deriveDshPresetModes(runtime, dshCustomPresets),
    [runtime, dshCustomPresets],
  )

  const projectId = conversation?.project_id ?? conversation?.projectId ?? null
  const setId = conversation?.set_id ?? conversation?.setId ?? null
  const storedActiveSkillId = conversation?.active_skill_id ?? conversation?.activeSkillId ?? null

  const patchContextState = useCallback((nextState: ConversationContextState) => {
    setContextState((prev) => {
      const merged = mergeClearContextState(prev, mergeCompactionContextState(prev, nextState))
      setConversation((current) => current
        ? { ...current, context_state: merged, contextState: merged }
        : current)
      return merged
    })
  }, [setConversation])

  const refreshContextStats = useCallback(async () => {
    const targetId = conversationIdRef.current
    if (!targetId) {
      setContextState(null)
      setContextError('')
      return
    }
    setContextLoading(true)
    setContextError('')
    try {
      const result = await chatApi.getContextStats(targetId)
      if (conversationIdRef.current === targetId) {
        patchContextState(result.contextState)
      }
    } catch (err) {
      if (conversationIdRef.current === targetId) {
        setContextError(typeof err === 'string' ? err : (err as Error).message || '上下文统计失败')
      }
    } finally {
      if (conversationIdRef.current === targetId) {
        setContextLoading(false)
      }
    }
  }, [patchContextState])

  const refreshToolIndicator = useCallback(async () => {
    if (!isTauriRuntime()) {
      setEnabledTools([])
      setEnabledToolCount(null)
      setToolsDisabledReason('')
      setToolsRequested(false)
      setMcpServers([])
      return
    }
    try {
      const settings = await getSettingsCached()
      const chatTools = settings.chatTools
      setMcpServers(chatTools?.servers ?? [])
      setWebSearchEnabled(chatTools?.nativeTools?.webSearch !== false)
      setProviderApiFormats(
        Object.fromEntries((settings.providers ?? []).map((provider) => [provider.id, provider.apiFormat ?? ''])),
      )
      setProviderBaseUrls(
        Object.fromEntries((settings.providers ?? []).map((provider) => [provider.id, provider.baseUrl ?? ''])),
      )
      const nextDisabledSkillIds = chatTools?.disabledSkillIds ?? []
      setDisabledSkillIds((prev) =>
        prev.length === nextDisabledSkillIds.length
        && prev.every((id, index) => id === nextDisabledSkillIds[index])
          ? prev
          : nextDisabledSkillIds,
      )
      if (!chatTools) {
        setEnabledTools([])
        setEnabledToolCount(null)
        setToolsDisabledReason('')
        setToolsRequested(false)
        return
      }
      const anyMcpEnabled = chatTools.enabled && chatTools.servers.some((server) => server.enabled)
      const requested = anyMcpEnabled
        || hasEnabledNativeBuiltinTool(chatTools.nativeTools)
        || hasEnabledSkillRuntime(chatTools.nativeTools)
      setToolsRequested(requested)
      if (!requested) {
        setEnabledTools([])
        setEnabledToolCount(null)
        setToolsDisabledReason('')
        return
      }
      const result = await api.chatMcpListTools()
      const tools = result.success ? result.tools : []
      setEnabledTools(tools)
      setEnabledToolCount(tools.length)
      setToolsDisabledReason(result.success ? '' : result.error || '工具不可用')
    } catch (err) {
      setEnabledTools([])
      setToolsRequested(false)
      setEnabledToolCount(null)
      setToolsDisabledReason(err instanceof Error ? err.message : String(err))
    }
  }, [])

  useEffect(() => {
    void refreshToolIndicator()
    return subscribeSettings((next) => {
      setMcpServers(next.chatTools?.servers ?? [])
      setWebSearchEnabled(next.chatTools?.nativeTools?.webSearch !== false)
    })
  }, [refreshToolIndicator])

  useEffect(() => {
    void refreshContextStats()
  }, [
    conversationId,
    conversation?.model,
    conversation?.provider_id,
    conversation?.updated_at,
    storedActiveSkillId,
    refreshContextStats,
  ])

  useEffect(() => {
    if (!projectId) {
      setBoundProject(null)
      return
    }
    let cancelled = false
    void chatApi.getProjects().then((projects) => {
      if (!cancelled) setBoundProject(projects.find((project) => project.id === projectId) ?? null)
    }).catch(() => {
      if (!cancelled) setBoundProject(null)
    })
    return () => {
      cancelled = true
    }
  }, [projectId])

  useEffect(() => {
    if (!setId) {
      setBoundSet(null)
      return
    }
    let cancelled = false
    void chatApi.getSets().then((sets) => {
      if (!cancelled) setBoundSet(sets.find((item) => item.id === setId) ?? null)
    }).catch(() => {
      if (!cancelled) setBoundSet(null)
    })
    return () => {
      cancelled = true
    }
  }, [setId])

  useEffect(() => {
    let cancelled = false
    dockApi.resolveCwd(conversationId, projectId).then((cwd) => {
      if (!cancelled) setGitWorkdir(cwd)
    }).catch(() => {
      if (!cancelled) setGitWorkdir('')
    })
    return () => {
      cancelled = true
    }
  }, [conversationId, projectId, runtime.kind, runtime.externalAgentId])

  useEffect(() => {
    if (!isTauriRuntime()) {
      setSkills([])
      return
    }
    let cancelled = false
    void api.chatSkillsList(undefined, gitWorkdir || undefined).then((result) => {
      if (cancelled) return
      if (result.success) setSkills(result.skills.map(normalizeSkill))
      else setSkills([])
    }).catch(() => {
      if (!cancelled) setSkills([])
    })
    return () => {
      cancelled = true
    }
  }, [gitWorkdir])

  useTauriEvent(api.onChatContext, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    if (payload.live) {
      setContextState((prev) => {
        const next = applyLiveContextUsage(prev, payload.live!)
        if (!next || next === prev) return prev
        setConversation((current) => current
          ? { ...current, context_state: next, contextState: next }
          : current)
        return next
      })
      return
    }
    if (!payload.contextState) return
    patchContextState(payload.contextState as ConversationContextState)
    setContextError('')
  }, [patchContextState, setConversation])

  useTauriEvent(api.onChatCompaction, (payload) => {
    if (payload.conversationId !== conversationIdRef.current) return
    if (payload.trigger !== 'manual') {
      setContextCompressing(payload.phase === 'started')
    }
  }, [])

  const handleRefreshContext = useCallback(() => {
    void refreshContextStats()
  }, [refreshContextStats])

  const handleCompressContext = useCallback(async () => {
    if (contextCompressing) return
    setContextCompressing(true)
    setContextError('')
    try {
      const result = await chatApi.compressContext(conversationId)
      if (conversationIdRef.current === conversationId) {
        setConversation(result.conversation)
        patchContextState(result.contextState)
      }
    } catch (err) {
      if (conversationIdRef.current === conversationId) {
        setContextError(typeof err === 'string' ? err : (err as Error).message || '上下文压缩失败')
      }
    } finally {
      setContextCompressing(false)
    }
  }, [contextCompressing, conversationId, patchContextState, setConversation])

  const handleClearContext = useCallback(async () => {
    setContextError('')
    try {
      const result = await chatApi.clearContext(conversationId)
      if (conversationIdRef.current === conversationId) {
        setConversation(result.conversation)
        patchContextState(result.contextState)
      }
    } catch (err) {
      if (conversationIdRef.current === conversationId) {
        setContextError(typeof err === 'string' ? err : (err as Error).message || '清空上下文失败')
      }
    }
  }, [conversationId, patchContextState, setConversation])

  const openMainSettings = useCallback(() => {
    void api.openSettingsWindow()
  }, [])

  const handleSelectProject = useCallback(async (project: ChatProject | null) => {
    const next = await chatApi.updateConversation(conversationId, {
      projectId: project?.id ?? null,
    })
    applyConversationMeta(setConversation, next)
    setBoundProject(project)
  }, [conversationId, setConversation])

  const handleSelectSet = useCallback(async (set: ChatSet | null) => {
    const next = await chatApi.updateConversation(conversationId, {
      setId: set?.id ?? null,
    })
    applyConversationMeta(setConversation, next)
    setBoundSet(set)
  }, [conversationId, setConversation])

  const handleSelectAssistant = useCallback(async (assistant: ChatAssistant | null) => {
    const next = await chatApi.updateConversation(conversationId, {
      assistantId: assistant?.id ?? '',
    })
    applyConversationMeta(setConversation, next)
    if (assistant) void refreshContextStats()
  }, [conversationId, refreshContextStats, setConversation])

  const handleChangeKnowledgeBaseIds = useCallback(async (ids: string[]) => {
    const next = await chatApi.updateConversation(conversationId, { knowledgeBaseIds: ids })
    applyConversationMeta(setConversation, next)
  }, [conversationId, setConversation])

  const handleToggleForceKnowledgeSearch = useCallback(async () => {
    const current = conversation?.force_knowledge_search ?? conversation?.forceKnowledgeSearch ?? false
    const next = await chatApi.updateConversation(conversationId, { forceKnowledgeSearch: !current })
    applyConversationMeta(setConversation, next)
  }, [conversation, conversationId, setConversation])

  const handleChangeAdditionalDirectories = useCallback(async (directories: AdditionalDirectory[]) => {
    const next = await chatApi.updateConversation(conversationId, { additionalDirectories: directories })
    applyConversationMeta(setConversation, next)
  }, [conversationId, setConversation])

  const handleSetWebSearchMode = useCallback(async (mode: WebSearchMode) => {
    saveLastWebSearchMode(mode)
    const next = await chatApi.updateConversation(conversationId, { webSearchMode: mode })
    applyConversationMeta(setConversation, next)
  }, [conversationId, setConversation])

  const handleChangeReplyModels = useCallback(async (models: ModelRef[]) => {
    const next = await chatApi.updateConversation(conversationId, { replyModels: models })
    applyConversationMeta(setConversation, next)
  }, [conversationId, setConversation])

  const handleToggleMcpServer = useCallback(async (serverId: string) => {
    try {
      const settings = await refreshSettings()
      const prevServers = settings.chatTools?.servers ?? []
      const current = prevServers.find((server) => server.id === serverId)
      if (current && isPluginManagedServer(current)) return
      const servers = preservePluginManagedServers(
        prevServers,
        prevServers.map((server) =>
          server.id === serverId ? { ...server, enabled: !server.enabled } : server,
        ),
      )
      setMcpServers(servers)
      await saveSettingsCached({
        ...settings,
        chatTools: { ...settings.chatTools, servers },
      })
      await refreshToolIndicator()
    } catch (err) {
      console.error('Failed to toggle MCP server:', err)
      void refreshToolIndicator()
    }
  }, [refreshToolIndicator])

  const handleAgentPlanModeChange = useCallback(async (mode: AgentPlanMode) => {
    const next = await chatApi.setAgentPlanMode(conversationId, mode)
    applyConversationMeta(setConversation, next)
    void refreshContextStats()
  }, [conversationId, refreshContextStats, setConversation])

  const handleComposerModeChange = useCallback(async (value: string) => {
    if (usesExternalRuntime) {
      const next = await chatApi.setAgentRuntime(conversationId, {
        ...runtime,
        kind: 'external',
        externalSandbox: value,
      })
      applyConversationMeta(setConversation, next)
      return
    }
    await handleAgentPlanModeChange(value as AgentPlanMode)
  }, [conversationId, handleAgentPlanModeChange, runtime, setConversation, usesExternalRuntime])

  const handleExternalPresetChange = useCallback(async (preset: string) => {
    const next = await chatApi.setAgentRuntime(conversationId, {
      ...runtime,
      kind: 'external',
      externalAgentPreset: preset,
    })
    applyConversationMeta(setConversation, next)
  }, [conversationId, runtime, setConversation])

  const enabledSkills = useMemo(
    () => skills.filter((skill) => !disabledSkillIds.includes(skill.id)),
    [disabledSkillIds, skills],
  )
  const slashSkills = useMemo(
    () => enabledSkills.map((skill) => ({
      id: skill.id,
      name: skill.name,
      description: skill.description,
      argumentHint: skill.argumentHint ?? skill.argument_hint ?? undefined,
      disableModelInvocation: skill.disableModelInvocation ?? skill.disable_model_invocation,
    })),
    [enabledSkills],
  )
  const effectiveSkill = useMemo(
    () => (storedActiveSkillId
      ? enabledSkills.find((skill) => skill.id === storedActiveSkillId) ?? null
      : null),
    [enabledSkills, storedActiveSkillId],
  )
  const recommendedTools = skillRecommendedTools(effectiveSkill)
  const unavailableRecommendedTools = useMemo(
    () => recommendedTools.filter(
      (recommended) => !enabledTools.some((tool) => toolMatchesRecommendation(tool, recommended)),
    ),
    [enabledTools, recommendedTools],
  )
  const toolStatusHint = useMemo(() => {
    if (toolsDisabledReason && (enabledToolCount ?? 0) === 0 && (toolsRequested || recommendedTools.length > 0)) {
      return recommendedTools.length > 0
        ? `当前 Skill 需要工具，但${toolsDisabledReason}`
        : toolsDisabledReason
    }
    if (unavailableRecommendedTools.length > 0) {
      return `当前 Skill 推荐的工具不可用：${unavailableRecommendedTools.slice(0, 3).join(', ')}`
    }
    return ''
  }, [enabledToolCount, recommendedTools.length, toolsDisabledReason, toolsRequested, unavailableRecommendedTools])

  const conversationBlank = isBlankConversation(conversation)
  const explicitWebSearch = conversation?.webSearchMode ?? conversation?.web_search_mode
  const webSearchMode: WebSearchMode = explicitWebSearch
    || loadLastWebSearchMode()
    || (webSearchEnabled ? 'third_party' : 'off')
  const assistantSnapshot = conversation?.assistant_snapshot ?? conversation?.assistantSnapshot ?? null
  const currentAssistant = assistantSnapshot
    ? { id: assistantSnapshot.id, name: assistantSnapshot.name }
    : null
  const conversationProject = useMemo<{ id: string; name: string } | null>(() => {
    if (!projectId) return null
    return { id: projectId, name: conversation?.folder ?? boundProject?.name ?? '' }
  }, [boundProject?.name, conversation?.folder, projectId])

  const contextSlot = useMemo(
    () => (
      <ContextIndicator
        contextState={contextState}
        messageCount={displayMessages.length}
        lastMessageId={displayMessages[displayMessages.length - 1]?.id}
        loading={contextLoading}
        compressing={contextCompressing}
        generating={streaming}
        error={contextError}
        usesExternalRuntime={usesExternalRuntime}
        onRefresh={handleRefreshContext}
        onCompress={handleCompressContext}
        onClear={usesExternalRuntime ? undefined : handleClearContext}
        lang={lang}
      />
    ),
    [
      contextCompressing,
      contextError,
      contextLoading,
      contextState,
      displayMessages,
      handleClearContext,
      handleCompressContext,
      handleRefreshContext,
      lang,
      streaming,
      usesExternalRuntime,
    ],
  )

  const usageSlot = useMemo(
    () => (
      <SessionUsageStrip
        messages={displayMessages}
        lang={lang}
        apiFormats={providerApiFormats}
        defaultApiFormat={conversation ? (providerApiFormats[conversation.provider_id] ?? '') : ''}
        cacheIncludedInInput={
          usesExternalRuntime
            ? runtime.externalAgentId === 'codex'
            : undefined
        }
      />
    ),
    [
      conversation,
      displayMessages,
      lang,
      providerApiFormats,
      runtime.externalAgentId,
      usesExternalRuntime,
    ],
  )

  return {
    onSend,
    disabled,
    onCancel,
    cancelVisible,
    cancelling,
    onOpenSettings: openMainSettings,
    onOpenTools: openMainSettings,
    onCompactContext: handleCompressContext,
    enabledTools,
    toolsDisabledReason,
    toolStatusHint,
    sendDisabledReason: recommendedTools.length > 0 ? toolStatusHint : '',
    agentPlanState: conversation?.agent_plan_state ?? conversation?.agentPlanState ?? null,
    agentTodoState: conversation?.agent_todo_state ?? conversation?.agentTodoState ?? null,
    onAgentPlanModeChange: handleAgentPlanModeChange,
    usesChatRuntime,
    enabledSkills: usesChatRuntime ? [] : slashSkills,
    onOpenSkillSettings: openMainSettings,
    selectedProject: boundProject,
    conversationProject,
    onSelectProject: handleSelectProject,
    showProjectEntry: true,
    selectedSet: boundSet,
    onSelectSet: handleSelectSet,
    currentAssistant,
    onOpenAssistantCenter: openMainSettings,
    onSelectAssistant: handleSelectAssistant,
    autoFocus: true,
    usesExternalRuntime,
    externalAgentName: runtime.externalAgentId ?? null,
    conversationId,
    knowledgeBaseIds: conversation?.knowledge_base_ids ?? conversation?.knowledgeBaseIds ?? [],
    onChangeKnowledgeBaseIds: handleChangeKnowledgeBaseIds,
    forceKnowledgeSearch: conversation?.force_knowledge_search ?? conversation?.forceKnowledgeSearch ?? false,
    onToggleForceKnowledgeSearch: handleToggleForceKnowledgeSearch,
    additionalDirectories: additionalDirectoriesOf(conversation),
    onChangeAdditionalDirectories: handleChangeAdditionalDirectories,
    additionalDirectoryPrimaryRoot: boundProject?.root_path ?? boundProject?.rootPath ?? null,
    mcpServers,
    onToggleMcpServer: handleToggleMcpServer,
    webSearchMode,
    onSetWebSearchMode: handleSetWebSearchMode,
    builtinWebSearchSupported: builtinWebSearchSupported(
      providerApiFormats[conversation?.provider_id ?? ''],
      providerBaseUrls[conversation?.provider_id ?? ''],
    ),
    replyModels: conversation?.reply_models ?? conversation?.replyModels ?? [],
    onChangeReplyModels: handleChangeReplyModels,
    contextSlot,
    gitWorkdir: usesChatRuntime ? null : gitWorkdir || null,
    gitLang: lang,
    onOpenGitPanel: () => {},
    modeOptions: composerModes.options,
    modeValue: composerModes.current,
    onModeChange: handleComposerModeChange,
    presetOptions: composerPresets.options,
    presetValue: composerPresets.current,
    onPresetChange: handleExternalPresetChange,
    presetLocked: Boolean(conversation) && !conversationBlank,
    presetLockedReason: i18n[lang].chatAgentPresetLocked,
    usageSlot,
  }
}
