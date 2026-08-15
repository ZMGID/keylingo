// Chat API 调用封装
import { invoke } from '@tauri-apps/api/core'
import { estimateTokens } from '../utils/tokens'
import { isExecutableAgentPlanText } from './agentPlan'
import { isTauriRuntime } from './utils'
import type { ConversationPin } from './conversationPins'
import type {
  AgentRuntimeConfig,
  ChatAssistant,
  ChatAssistantSnapshot,
  ChatProject,
  ChatSet,
  Conversation,
  ConversationContextState,
  ConversationLibraryPage,
  ConversationLibraryQuery,
  ConversationListItem,
  ConversationSearchHit,
  AgentPlanMode,
  DetectedExternalAgent,
  PendingAttachment,
} from './types'
import type { ThinkingLevel, WebSearchMode, ModelRef } from './types'
import type { CliImportResult, ImportableCliSession } from './types'

export type { DetectedExternalAgent, AgentRuntimeConfig }

/** `chat_external_cli_scan_cc_switch` 的返回。`hasApiKey` 是布尔，后端不回明文 key。 */
export interface CcSwitchProvider {
  agentId: string
  id: string
  name: string
  remark: string
  env: Array<{ key: string; value: string }>
  configToml: string
  authJson: string
  hasApiKey: boolean
  isCurrent: boolean
}

export interface CcSwitchScan {
  providers: CcSwitchProvider[]
  /** 认得出但 Kivio 没有落地通道而跳过的条数（grok / hermes / openclaw…）。 */
  skipped: number
}

/** `$DSH_HOME/settings.yaml` 里三个官方插件 namespace 的当前值。`null` 字段 = 沿用 schema 默认。 */
export interface DshPluginSettingsSnapshot {
  settingsPath: string
  shell: {
    timeoutMs: number | null
    maxOutputBytes: number | null
    timeoutMsDefault: number
    maxOutputBytesDefault: number
  }
  agentLoop: {
    maxParallelToolCalls: number | null
    maxParallelToolCallsDefault: number
  }
  webSearch: {
    baseUrl: string | null
    maxUses: number | null
    apiKeyEnv: string
    apiKeyConfigured: boolean
    apiKeyWritable: boolean
    baseUrlDefault: string
    maxUsesDefault: number
  }
}

/** 只带要改的 namespace。字段 `null` = 恢复默认。 */
export interface DshPluginSettingsPatch {
  shell?: {
    timeoutMs?: number | null
    maxOutputBytes?: number | null
  }
  agentLoop?: {
    maxParallelToolCalls?: number | null
  }
  webSearch?: {
    baseUrl?: string | null
    maxUses?: number | null
    apiKey?: string
  }
}

export interface DshPluginEntry {
  id: string
  moduleName: string
  enabled: boolean
}

/** `$DSH_HOME/.credentials.yaml` 里官方 DeepSeek 模型密钥的状态。不回读密钥本身。 */
export interface DshOfficialCredential {
  configured: boolean
  writable: boolean
}

export interface DshNativeProviderModel {
  id: string
  name: string
}

/** 用户点「修改」时才回读的 `settings.yaml` 第三方供应商，含凭据文件里的密钥。 */
export interface DshNativeProviderDetail {
  id: string
  name: string
  baseUrl: string
  api: string
  apiKey: string
  apiKeyEnv: string
  models: DshNativeProviderModel[]
  defaultModel: string
}

/** `chat_external_cli_install_info` 的返回。 */
export interface ExternalCliInstallInfo {
  agentId: string
  localVersion: string | null
  latestVersion: string | null
  updateAvailable: boolean
  /** 可直接执行的安装/更新命令；null = 只能照文档手动装。 */
  command: string | null
  docsUrl: string
  /** 已存在的配置目录绝对路径；null = 还没生成。 */
  configDir: string | null
}

/** 订阅安装日志。`done` 那条带最终成功与否，`line` 为 null。 */
export async function onExternalCliInstallLog(
  handler: (event: { agentId: string; line: string | null; done: boolean; success: boolean }) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) return () => {}
  const { listen } = await import('@tauri-apps/api/event')
  const un = await listen<{ agentId: string; line: string | null; done: boolean; success: boolean }>(
    'external-cli-install',
    (e) => handler(e.payload),
  )
  return un
}

/** 订阅后台重探完成的可用性列表（`chat_detect_external_agents` 先返回落盘快照，探完再推这条）。 */
export async function onExternalAgentsUpdated(
  handler: (agents: DetectedExternalAgent[]) => void,
): Promise<() => void> {
  if (!isTauriRuntime()) return () => {}
  const { listen } = await import('@tauri-apps/api/event')
  return await listen<{ agents: DetectedExternalAgent[] }>('external-agents-updated', (e) =>
    handler(e.payload.agents ?? []),
  )
}

export const BUILTIN_AGENT_RUNTIME: AgentRuntimeConfig = {
  kind: 'builtin',
  externalAgentId: null,
  externalModel: null,
  externalReasoning: null,
  externalSandbox: null,
  externalAgentPreset: null,
}

export const CHAT_AGENT_RUNTIME: AgentRuntimeConfig = {
  kind: 'chat',
  externalAgentId: null,
  externalModel: null,
  externalReasoning: null,
  externalSandbox: null,
  externalAgentPreset: null,
}

export function normalizeAgentRuntime(
  conversation?: Conversation | null,
): AgentRuntimeConfig {
  const raw = conversation?.agent_runtime ?? conversation?.agentRuntime
  if (!raw || raw.kind === 'builtin') {
    return { ...BUILTIN_AGENT_RUNTIME }
  }
  if (raw.kind === 'chat') {
    return { ...CHAT_AGENT_RUNTIME }
  }
  return {
    kind: 'external',
    externalAgentId: raw.externalAgentId ?? raw.external_agent_id ?? null,
    externalModel: raw.externalModel ?? raw.external_model ?? 'default',
    externalReasoning: raw.externalReasoning ?? raw.external_reasoning ?? null,
    externalSandbox: raw.externalSandbox ?? raw.external_sandbox ?? null,
    externalAgentPreset: raw.externalAgentPreset ?? raw.external_agent_preset ?? null,
  }
}

export function agentRuntimesEqual(
  left: AgentRuntimeConfig,
  right: AgentRuntimeConfig,
): boolean {
  const normalize = (value: AgentRuntimeConfig): AgentRuntimeConfig => {
    if (value.kind === 'external') return value
    if (value.kind === 'chat') return CHAT_AGENT_RUNTIME
    return BUILTIN_AGENT_RUNTIME
  }
  const a = normalize(left)
  const b = normalize(right)
  return a.kind === b.kind
    && (a.externalAgentId ?? null) === (b.externalAgentId ?? null)
    && (a.externalModel ?? 'default') === (b.externalModel ?? 'default')
    && (a.externalReasoning ?? null) === (b.externalReasoning ?? null)
    && (a.externalSandbox ?? null) === (b.externalSandbox ?? null)
    && (a.externalAgentPreset ?? null) === (b.externalAgentPreset ?? null)
}

const mockStorageKey = 'kivio-chat-dev-conversations'
const mockProjectsStorageKey = 'kivio-chat-dev-projects'
const mockAssistantsStorageKey = 'kivio-chat-dev-assistants'

const nowSeconds = () => Math.floor(Date.now() / 1000)

function loadMockConversations(): Conversation[] {
  try {
    const raw = window.localStorage.getItem(mockStorageKey)
    if (!raw) return []
    const parsed = JSON.parse(raw)
    return Array.isArray(parsed) ? parsed : []
  } catch {
    return []
  }
}

function saveMockConversations(conversations: Conversation[]) {
  window.localStorage.setItem(mockStorageKey, JSON.stringify(conversations))
}

function loadMockProjects(): ChatProject[] {
  try {
    const raw = window.localStorage.getItem(mockProjectsStorageKey)
    if (!raw) return []
    const parsed = JSON.parse(raw)
    return Array.isArray(parsed) ? parsed : []
  } catch {
    return []
  }
}

function saveMockProjects(projects: ChatProject[]) {
  window.localStorage.setItem(mockProjectsStorageKey, JSON.stringify(projects))
}

function loadMockAssistants(): ChatAssistant[] {
  try {
    const raw = window.localStorage.getItem(mockAssistantsStorageKey)
    const parsed = raw ? JSON.parse(raw) : []
    const assistants = Array.isArray(parsed) ? parsed : []
    assistants.sort((a, b) => b.updated_at - a.updated_at || a.name.localeCompare(b.name, 'zh-CN'))
    return assistants
  } catch {
    return []
  }
}

function saveMockAssistants(assistants: ChatAssistant[]) {
  window.localStorage.setItem(mockAssistantsStorageKey, JSON.stringify(assistants))
}

function normalizeAssistant(assistant: ChatAssistant): ChatAssistant {
  const now = nowSeconds()
  return {
    ...assistant,
    name: assistant.name.trim(),
    description: assistant.description?.trim() ?? '',
    icon: assistant.icon?.trim() ?? '',
    color: assistant.color?.trim() ?? '',
    source: assistant.source ?? (assistant.built_in ?? assistant.builtIn ? 'builtin' : 'user'),
    system_prompt: (assistant.system_prompt ?? assistant.systemPrompt ?? '').trim(),
    provider_id: (assistant.provider_id ?? assistant.providerId ?? '').trim(),
    model: (assistant.model ?? '').trim(),
    mcp_server_ids: assistant.mcp_server_ids ?? assistant.mcpServerIds ?? [],
    skill_ids: assistant.skill_ids ?? assistant.skillIds ?? [],
    enabled: assistant.enabled ?? true,
    installed: assistant.installed ?? true,
    archived: assistant.archived ?? false,
    built_in: assistant.built_in ?? assistant.builtIn ?? false,
    created_at: assistant.created_at ?? assistant.createdAt ?? now,
    updated_at: now,
  }
}

function assistantSnapshot(assistant: ChatAssistant): ChatAssistantSnapshot {
  return {
    id: assistant.id,
    name: assistant.name,
    description: assistant.description ?? '',
    source: assistant.source,
    system_prompt: assistant.system_prompt ?? assistant.systemPrompt ?? '',
    provider_id: assistant.provider_id ?? assistant.providerId ?? '',
    model: assistant.model ?? '',
    mcp_server_ids: assistant.mcp_server_ids ?? assistant.mcpServerIds ?? [],
    skill_ids: assistant.skill_ids ?? assistant.skillIds ?? [],
  }
}

function normalizeProjectName(name: string): string {
  const trimmed = name.trim()
  if (!trimmed) throw new Error('项目名称不能为空')
  if ([...trimmed].length > 80) throw new Error('项目名称不能超过 80 个字符')
  return trimmed
}

function loadMockProjectsWithLegacyFolders(): ChatProject[] {
  const projects = loadMockProjects()
  const now = nowSeconds()
  let changed = false
  for (const folder of loadMockConversations()
    .map((conversation) => conversation.folder?.trim())
    .filter((folder): folder is string => Boolean(folder))) {
    if (projects.some((project) => project.name === folder)) continue
    projects.push({
      id: `proj_dev_${crypto.randomUUID()}`,
      name: folder,
      root_path: null,
      created_at: now,
      updated_at: now,
    })
    changed = true
  }
  projects.sort((a, b) => b.updated_at - a.updated_at || a.name.localeCompare(b.name, 'zh-CN'))
  if (changed) saveMockProjects(projects)
  return projects
}

function toListItem(conversation: Conversation): ConversationListItem {
  const preview = [...conversation.messages]
    .reverse()
    .find((message) => message.role === 'user' || message.role === 'assistant')
    ?.content.trim() ?? ''
  return {
    id: conversation.id,
    title: conversation.title,
    preview: preview.length > 100 ? `${preview.slice(0, 100)}...` : preview,
    provider_id: conversation.provider_id,
    model: conversation.model,
    message_count: conversation.messages.length,
    created_at: conversation.created_at,
    updated_at: conversation.updated_at,
    pinned: conversation.pinned,
    folder: conversation.folder,
    project_id: conversation.project_id ?? conversation.projectId ?? null,
    projectId: conversation.project_id ?? conversation.projectId ?? null,
    assistant_id: conversation.assistant_id ?? conversation.assistantId ?? null,
    assistant_name:
      conversation.assistant_snapshot?.name
      ?? conversation.assistantSnapshot?.name
      ?? null,
  }
}

function estimateMockContext(conversation: Conversation): ConversationContextState {
  const conversationTokens = conversation.messages.reduce(
    (sum, message) => sum + estimateTokens(message.content || ''),
    0,
  )
  const attachmentTokens = conversation.messages.reduce(
    (sum, message) => sum + (message.attachments?.filter((attachment) => attachment.type === 'image').length ?? 0) * 1200,
    0,
  )
  const systemTokens = 900
  const planText = (conversation.agent_plan_state?.plan ?? conversation.agentPlanState?.plan ?? '').trim()
  const planTokens = planText ? estimateTokens(planText) + 80 : 0
  const estimatedInputTokens = systemTokens + planTokens + conversationTokens + attachmentTokens
  const contextWindowTokens = 200_000
  const usageRatio = estimatedInputTokens / contextWindowTokens
  const summary = conversation.context_state?.summary ?? conversation.contextState?.summary ?? null
  return {
    estimated_input_tokens: estimatedInputTokens,
    context_window_tokens: contextWindowTokens,
    context_window_estimated: true,
    usage_ratio: usageRatio,
    status: summary?.stale
      ? 'stale'
      : summary
        ? 'compressed'
        : usageRatio >= 0.95
          ? 'critical'
          : usageRatio >= 0.70
            ? 'warning'
            : 'normal',
    segments: [
      { id: 'system_prompt', label: 'System prompt', estimated_tokens: systemTokens, color: '#7A7A7A' },
      { id: 'agent_plan', label: 'Agent plan', estimated_tokens: planTokens, color: '#8A724C' },
      { id: 'conversation', label: 'Conversation', estimated_tokens: conversationTokens, color: '#D07652' },
      { id: 'attachments', label: 'Attachments', estimated_tokens: attachmentTokens, color: '#6A8FBD' },
    ].filter((segment) => segment.estimated_tokens > 0),
    last_measured_at: nowSeconds(),
    last_compressed_at: summary?.created_at ?? summary?.createdAt ?? null,
    compressed_message_count: summary?.source_message_ids?.length ?? summary?.sourceMessageIds?.length ?? 0,
    compression_count: conversation.context_state?.compression_count
      ?? conversation.contextState?.compressionCount
      ?? (summary ? 1 : 0),
    summary,
  }
}

function withMockContext(conversation: Conversation): Conversation {
  const contextState = estimateMockContext(conversation)
  return {
    ...conversation,
    context_state: contextState,
    contextState,
    agent_plan_state: conversation.agent_plan_state ?? conversation.agentPlanState ?? { mode: 'act', status: 'empty', plan: null, updated_at: 0 },
    agentPlanState: conversation.agentPlanState ?? conversation.agent_plan_state ?? { mode: 'act', status: 'empty', plan: null, updated_at: 0 },
  }
}

const mockChatApi = {
  async getConversations(
    offset = 0,
    limit = 50,
    folder?: string,
    projectId?: string | null,
  ): Promise<ConversationListItem[]> {
    const conversations = loadMockConversations()
      .filter((conversation) => {
        if (projectId) {
          const conversationProjectId = conversation.project_id ?? conversation.projectId ?? null
          return conversationProjectId === projectId || (!conversationProjectId && conversation.folder === folder)
        }
        return !folder || conversation.folder === folder
      })
      .sort((a, b) => b.updated_at - a.updated_at)
    return conversations.slice(offset, offset + limit).map(toListItem)
  },

  async getConversation(conversationId: string): Promise<Conversation> {
    const conversation = loadMockConversations().find((item) => item.id === conversationId)
    if (!conversation) throw new Error('Conversation not found')
    return withMockContext(conversation)
  },

  async createConversation(
    providerId?: string,
    model?: string,
    folder?: string,
    projectId?: string | null,
    assistantId?: string | null,
  ): Promise<Conversation> {
    const now = nowSeconds()
    const assistant = assistantId
      ? loadMockAssistants().find((item) => item.id === assistantId && !item.archived && item.enabled !== false)
      : undefined
    const snapshot = assistant ? assistantSnapshot(assistant) : null
    const conversation: Conversation = {
      id: `conv_dev_${crypto.randomUUID()}`,
      revision: 0,
      title: '新对话',
      provider_id: providerId?.trim() || snapshot?.provider_id || snapshot?.providerId || 'dev-provider',
      model: model?.trim() || snapshot?.model || 'dev-model',
      messages: [],
      active_skill_id: null,
      activeSkillId: null,
      assistant_id: snapshot?.id ?? null,
      assistantId: snapshot?.id ?? null,
      assistant_snapshot: snapshot,
      assistantSnapshot: snapshot,
      created_at: now,
      updated_at: now,
      pinned: false,
      folder,
      project_id: projectId ?? null,
      projectId: projectId ?? null,
      agent_todo_state: { items: [], updated_at: 0 },
      agentTodoState: { items: [], updated_at: 0 },
      agent_plan_state: { mode: 'act', status: 'empty', plan: null, updated_at: 0 },
      agentPlanState: { mode: 'act', status: 'empty', plan: null, updated_at: 0 },
    }
    const withContext = withMockContext(conversation)
    saveMockConversations([withContext, ...loadMockConversations()])
    return withContext
  },

  async getProjects(): Promise<ChatProject[]> {
    return loadMockProjectsWithLegacyFolders()
  },

  async createProject(
    name: string,
    description?: string | null,
    color?: string | null,
    rootPath?: string | null,
  ): Promise<ChatProject> {
    const normalized = normalizeProjectName(name)
    const projects = loadMockProjectsWithLegacyFolders()
    if (projects.some((project) => project.name === normalized)) {
      throw new Error('项目名称已存在')
    }
    const now = nowSeconds()
    const project: ChatProject = {
      id: `proj_dev_${crypto.randomUUID()}`,
      name: normalized,
      description: description ?? null,
      color: color ?? null,
      root_path: rootPath ?? null,
      rootPath: rootPath ?? null,
      created_at: now,
      updated_at: now,
    }
    saveMockProjects([project, ...projects])
    return project
  },

  async updateProject(
    projectId: string,
    updates: { name?: string; description?: string | null; color?: string | null; rootPath?: string | null },
  ): Promise<ChatProject> {
    const projects = loadMockProjectsWithLegacyFolders()
    const index = projects.findIndex((project) => project.id === projectId)
    if (index < 0) throw new Error('项目不存在')
    const oldName = projects[index].name
    const nextName = updates.name === undefined ? oldName : normalizeProjectName(updates.name)
    if (nextName !== oldName && projects.some((project) => project.name === nextName)) {
      throw new Error('项目名称已存在')
    }
    const nextProject: ChatProject = {
      ...projects[index],
      name: nextName,
      description: updates.description !== undefined ? updates.description : projects[index].description,
      color: updates.color !== undefined ? updates.color : projects[index].color,
      root_path: updates.rootPath !== undefined ? updates.rootPath : (projects[index].root_path ?? projects[index].rootPath ?? null),
      rootPath: updates.rootPath !== undefined ? updates.rootPath : (projects[index].rootPath ?? projects[index].root_path ?? null),
      updated_at: nowSeconds(),
    }
    projects[index] = nextProject
    saveMockProjects(projects)

    if (nextName !== oldName) {
      const conversations = loadMockConversations().map((conversation) =>
        conversation.folder === oldName
          ? { ...conversation, folder: nextName, updated_at: nowSeconds() }
          : conversation,
      )
      saveMockConversations(conversations)
    }
    return nextProject
  },

  async openProjectFolder(projectId: string): Promise<void> {
    const project = loadMockProjectsWithLegacyFolders().find((item) => item.id === projectId)
    if (!project) throw new Error('项目不存在')
    const rootPath = (project.root_path ?? project.rootPath ?? '').trim()
    if (!rootPath) throw new Error('该项目尚未配置文件夹')
    console.info('[mock] open project folder:', rootPath)
  },

  async deleteProject(projectId: string): Promise<void> {
    const projects = loadMockProjectsWithLegacyFolders()
    const project = projects.find((item) => item.id === projectId)
    if (!project) throw new Error('项目不存在')
    saveMockProjects(projects.filter((item) => item.id !== projectId))
    saveMockConversations(
      loadMockConversations().map((conversation) =>
        (conversation.project_id ?? conversation.projectId) === project.id || conversation.folder === project.name
          ? { ...conversation, folder: undefined, project_id: null, projectId: null, updated_at: nowSeconds() }
          : conversation,
      ),
    )
  },

  async getAssistants(): Promise<ChatAssistant[]> {
    return loadMockAssistants().filter((assistant) => !assistant.archived)
  },

  async createAssistant(assistant: ChatAssistant): Promise<ChatAssistant> {
    const next = normalizeAssistant({
      ...assistant,
      id: assistant.id || `asst_dev_${crypto.randomUUID()}`,
      built_in: false,
      created_at: assistant.created_at ?? nowSeconds(),
    })
    if (!next.name) throw new Error('助手名称不能为空')
    const assistants = loadMockAssistants()
    if (assistants.some((item) => !item.archived && item.name === next.name)) {
      throw new Error('助手名称已存在')
    }
    saveMockAssistants([next, ...assistants])
    return next
  },

  async updateAssistant(assistant: ChatAssistant): Promise<ChatAssistant> {
    const assistants = loadMockAssistants()
    const index = assistants.findIndex((item) => item.id === assistant.id)
    if (index < 0) throw new Error('助手不存在')
    const next = normalizeAssistant({
      ...assistant,
      built_in: assistants[index].built_in,
      created_at: assistants[index].created_at,
    })
    if (!next.name) throw new Error('助手名称不能为空')
    if (assistants.some((item) => item.id !== next.id && !item.archived && item.name === next.name)) {
      throw new Error('助手名称已存在')
    }
    assistants[index] = next
    saveMockAssistants(assistants)
    return next
  },

  async duplicateAssistant(assistantId: string): Promise<ChatAssistant> {
    const assistants = loadMockAssistants()
    const source = assistants.find((assistant) => assistant.id === assistantId)
    if (!source) throw new Error('助手不存在')
    const baseName = `${source.name} 副本`
    let name = baseName
    let suffix = 2
    while (assistants.some((assistant) => !assistant.archived && assistant.name === name)) {
      name = `${baseName} ${suffix}`
      suffix += 1
    }
    const copy = normalizeAssistant({
      ...source,
      id: `asst_dev_${crypto.randomUUID()}`,
      name,
      built_in: false,
      created_at: nowSeconds(),
    })
    saveMockAssistants([copy, ...assistants])
    return copy
  },

  async deleteAssistant(assistantId: string): Promise<void> {
    const assistants = loadMockAssistants()
    const index = assistants.findIndex((assistant) => assistant.id === assistantId)
    if (index < 0) throw new Error('助手不存在')
    assistants[index] = {
      ...assistants[index],
      archived: true,
      updated_at: nowSeconds(),
    }
    saveMockAssistants(assistants)
  },

  async sendMessage(
    conversationId: string,
    content: string,
    attachments: PendingAttachment[] = [],
    activeSkillId?: string | null,
  ): Promise<Conversation> {
    const conversations = loadMockConversations()
    const index = conversations.findIndex((item) => item.id === conversationId)
    if (index < 0) throw new Error('Conversation not found')
    const now = nowSeconds()
    const conversation = { ...conversations[index] }
    conversation.active_skill_id = activeSkillId ?? conversation.active_skill_id ?? conversation.activeSkillId ?? null
    conversation.activeSkillId = conversation.active_skill_id
    conversation.messages = [
      ...conversation.messages,
      {
        id: `msg_dev_${crypto.randomUUID()}`,
        role: 'user',
        content,
        attachments: attachments.map((attachment) => ({
          id: attachment.id,
          type: attachment.type,
          name: attachment.name,
          path: attachment.path,
        })),
        timestamp: now,
      },
      {
        id: `msg_dev_${crypto.randomUUID()}`,
        role: 'assistant',
        content: '这是浏览器预览模式的本地回复。启动 Tauri 桌面应用后会调用真实模型接口。',
        active_skill_id: conversation.active_skill_id,
        timestamp: now,
      },
    ]
    const currentPlanMode = conversation.agent_plan_state?.mode ?? conversation.agentPlanState?.mode ?? 'act'
    if (currentPlanMode === 'plan') {
      const assistantIndex = conversation.messages.length - 1
      const reply = conversation.messages[assistantIndex]?.content ?? ''
      if (isExecutableAgentPlanText(reply)) {
        conversation.agent_plan_state = {
          mode: 'plan',
          status: 'draft',
          plan: reply,
          updated_at: now,
        }
        conversation.agentPlanState = conversation.agent_plan_state
        conversation.messages[assistantIndex] = {
          ...conversation.messages[assistantIndex],
          agent_plan: conversation.agent_plan_state,
          agentPlan: conversation.agent_plan_state,
        }
      }
    }
    if (conversation.title === '新对话') {
      conversation.title = content.length > 30 ? `${content.slice(0, 30)}...` : content
    }
    conversation.updated_at = now
    const contextState = estimateMockContext(conversation)
    conversation.context_state = contextState
    conversation.contextState = contextState
    conversations[index] = conversation
    saveMockConversations(conversations)
    return conversation
  },

  async setAgentPlanMode(conversationId: string, mode: AgentPlanMode): Promise<Conversation> {
    const conversations = loadMockConversations()
    const index = conversations.findIndex((item) => item.id === conversationId)
    if (index < 0) throw new Error('Conversation not found')
    const now = nowSeconds()
    const current = conversations[index].agent_plan_state ?? conversations[index].agentPlanState ?? {
      mode: 'act',
      status: 'empty',
      plan: null,
      updated_at: 0,
    }
    const conversation = {
      ...conversations[index],
      agent_plan_state: { ...current, mode, updated_at: now },
      updated_at: now,
    }
    conversation.agentPlanState = conversation.agent_plan_state
    const contextState = estimateMockContext(conversation)
    conversation.context_state = contextState
    conversation.contextState = contextState
    conversations[index] = conversation
    saveMockConversations(conversations)
    return conversation
  },

  async executeAgentPlan(conversationId: string, messageId?: string): Promise<Conversation> {
    const conversations = loadMockConversations()
    const index = conversations.findIndex((item) => item.id === conversationId)
    if (index < 0) throw new Error('Conversation not found')
    const now = nowSeconds()
    const messageIndex = messageId
      ? conversations[index].messages.findIndex((message) => message.id === messageId && message.role === 'assistant')
      : -1
    if (messageId && messageIndex < 0) throw new Error('计划消息不存在')
    const messagePlan = messageIndex >= 0
      ? conversations[index].messages[messageIndex].agent_plan ?? conversations[index].messages[messageIndex].agentPlan ?? null
      : null
    if (messageId && !isExecutableAgentPlanText(messagePlan?.plan)) throw new Error('该消息不是可执行计划')
    const current = messagePlan ?? conversations[index].agent_plan_state ?? conversations[index].agentPlanState ?? {
      mode: 'act',
      status: 'empty',
      plan: null,
      updated_at: 0,
    }
    const hasPlan = isExecutableAgentPlanText(current.plan)
    const conversation = {
      ...conversations[index],
      agent_plan_state: {
        ...current,
        mode: 'act' as AgentPlanMode,
        status: hasPlan ? 'approved' as const : 'empty' as const,
        updated_at: now,
      },
      updated_at: now,
    }
    conversation.agentPlanState = conversation.agent_plan_state
    if (messageIndex >= 0) {
      conversation.messages = conversation.messages.map((message, i) =>
        i === messageIndex
          ? { ...message, agent_plan: conversation.agent_plan_state, agentPlan: conversation.agent_plan_state }
          : message,
      )
    }
    const contextState = estimateMockContext(conversation)
    conversation.context_state = contextState
    conversation.contextState = contextState
    conversations[index] = conversation
    saveMockConversations(conversations)
    return conversation
  },

  async deleteConversation(conversationId: string): Promise<void> {
    saveMockConversations(loadMockConversations().filter((item) => item.id !== conversationId))
  },

  async updateConversation(
    conversationId: string,
    updates: {
      title?: string
      pinned?: boolean
      folder?: string
      projectId?: string | null
      providerId?: string
      model?: string
      activeSkillId?: string | null
      assistantId?: string | null
    }
  ): Promise<Conversation> {
    const conversations = loadMockConversations()
    const index = conversations.findIndex((item) => item.id === conversationId)
    if (index < 0) throw new Error('Conversation not found')
    const hasFolderUpdate = 'folder' in updates
    const hasProjectUpdate = 'projectId' in updates
    const project = hasProjectUpdate && updates.projectId
      ? loadMockProjectsWithLegacyFolders().find((item) => item.id === updates.projectId)
      : undefined
    const conversation = {
      ...conversations[index],
      title: updates.title ?? conversations[index].title,
      pinned: updates.pinned ?? conversations[index].pinned,
      folder: hasProjectUpdate
        ? project?.name
        : hasFolderUpdate
          ? updates.folder || undefined
          : conversations[index].folder,
      project_id: hasProjectUpdate
        ? updates.projectId || null
        : conversations[index].project_id ?? conversations[index].projectId ?? null,
      projectId: hasProjectUpdate
        ? updates.projectId || null
        : conversations[index].projectId ?? conversations[index].project_id ?? null,
      provider_id: updates.providerId ?? conversations[index].provider_id,
      model: updates.model ?? conversations[index].model,
      active_skill_id:
        updates.activeSkillId !== undefined
          ? updates.activeSkillId || null
          : conversations[index].active_skill_id ?? conversations[index].activeSkillId ?? null,
      updated_at: nowSeconds(),
    }
    if ('assistantId' in updates) {
      const assistantId = updates.assistantId?.trim() ?? ''
      if (!assistantId) {
        conversation.assistant_id = null
        conversation.assistantId = null
        conversation.assistant_snapshot = null
        conversation.assistantSnapshot = null
      } else {
        const assistant = loadMockAssistants().find((item) =>
          item.id === assistantId && !item.archived && item.enabled !== false
        )
        if (!assistant) throw new Error('助手不存在或不可用')
        const snapshot = assistantSnapshot(assistant)
        conversation.assistant_id = snapshot.id
        conversation.assistantId = snapshot.id
        conversation.assistant_snapshot = snapshot
        conversation.assistantSnapshot = snapshot
        conversation.active_skill_id = null
      }
    }
    conversation.activeSkillId = conversation.active_skill_id
    const contextState = estimateMockContext(conversation)
    conversation.context_state = contextState
    conversation.contextState = contextState
    conversations[index] = conversation
    saveMockConversations(conversations)
    return conversation
  },

  async updateMessage(
    conversationId: string,
    messageId: string,
    content: string,
  ): Promise<Conversation> {
    const conversations = loadMockConversations()
    const index = conversations.findIndex((item) => item.id === conversationId)
    if (index < 0) throw new Error('Conversation not found')
    const trimmed = content.trim()
    if (!trimmed) throw new Error('消息内容不能为空')
    const conversation = { ...conversations[index] }
    const messageIndex = conversation.messages.findIndex((message) => message.id === messageId)
    if (messageIndex < 0) throw new Error('Message not found')
    if (conversation.messages[messageIndex].role !== 'assistant') {
      throw new Error('仅支持编辑助手回复')
    }
    conversation.messages = conversation.messages.map((message, i) =>
      i === messageIndex
        ? { ...message, content: trimmed, timestamp: nowSeconds() }
        : message,
    )
    conversation.updated_at = nowSeconds()
    const contextState = estimateMockContext(conversation)
    conversation.context_state = contextState
    conversation.contextState = contextState
    conversations[index] = conversation
    saveMockConversations(conversations)
    return conversation
  },

  async deleteMessage(conversationId: string, messageId: string): Promise<Conversation> {
    const conversations = loadMockConversations()
    const index = conversations.findIndex((item) => item.id === conversationId)
    if (index < 0) throw new Error('Conversation not found')
    const conversation = { ...conversations[index] }
    const target = conversation.messages.find((message) => message.id === messageId)
    if (!target) throw new Error('Message not found')
    if (target.role !== 'assistant') throw new Error('仅支持删除助手回复')
    conversation.messages = conversation.messages.filter((message) => message.id !== messageId)
    conversation.updated_at = nowSeconds()
    const contextState = estimateMockContext(conversation)
    conversation.context_state = contextState
    conversation.contextState = contextState
    conversations[index] = conversation
    saveMockConversations(conversations)
    return conversation
  },

  async regenerateMessage(conversationId: string, messageId: string, newContent?: string): Promise<Conversation> {
    const conversations = loadMockConversations()
    const index = conversations.findIndex((item) => item.id === conversationId)
    if (index < 0) throw new Error('Conversation not found')
    const conversation = { ...conversations[index] }
    const messageIndex = conversation.messages.findIndex((message) => message.id === messageId)
    if (messageIndex < 0) throw new Error('Message not found')
    const target = conversation.messages[messageIndex]
    if (target.role === 'user') {
      // 编辑提问并重新生成（镜像后端 chat_regenerate_message 的 user 分支）。
      const trimmed = (newContent ?? '').trim()
      if (newContent !== undefined && !trimmed) throw new Error('消息内容不能为空')
      const edited = trimmed
        ? { ...target, content: trimmed, timestamp: nowSeconds() }
        : target
      const kept = [...conversation.messages.slice(0, messageIndex), edited]
      conversation.messages = [
        ...kept,
        {
          id: `msg_dev_${crypto.randomUUID()}`,
          role: 'assistant',
          content: `（重新生成预览）${edited.content.slice(0, 80)}`,
          timestamp: nowSeconds(),
        },
      ]
    } else {
      if (newContent !== undefined) throw new Error('编辑内容仅支持用户消息')
      if (target.role !== 'assistant') throw new Error('仅支持重新生成助手回复')
      const kept = conversation.messages.slice(0, messageIndex)
      const lastUser = kept[kept.length - 1]
      if (!lastUser || lastUser.role !== 'user') {
        throw new Error('缺少对应的用户消息，无法重新生成')
      }
      conversation.messages = [
        ...kept,
        {
          id: `msg_dev_${crypto.randomUUID()}`,
          role: 'assistant',
          content: `（重新生成预览）${lastUser.content.slice(0, 80)}`,
          timestamp: nowSeconds(),
        },
      ]
    }
    conversation.updated_at = nowSeconds()
    const contextState = estimateMockContext(conversation)
    conversation.context_state = contextState
    conversation.contextState = contextState
    conversations[index] = conversation
    saveMockConversations(conversations)
    return conversation
  },

  async rewindToMessage(
    conversationId: string,
    messageId: string,
  ): Promise<{ conversation: Conversation; content: string }> {
    const conversations = loadMockConversations()
    const index = conversations.findIndex((item) => item.id === conversationId)
    if (index < 0) throw new Error('Conversation not found')
    const conversation = { ...conversations[index] }
    const messageIndex = conversation.messages.findIndex((message) => message.id === messageId)
    if (messageIndex < 0) throw new Error('Message not found')
    const target = conversation.messages[messageIndex]
    if (target.role !== 'user') throw new Error('仅支持回到用户提问')
    conversation.messages = conversation.messages.slice(0, messageIndex)
    conversation.updated_at = nowSeconds()
    const contextState = estimateMockContext(conversation)
    conversation.context_state = contextState
    conversation.contextState = contextState
    conversations[index] = conversation
    saveMockConversations(conversations)
    return { conversation, content: target.content }
  },

  async getContextStats(conversationId: string): Promise<{ contextState: ConversationContextState; conversation: Conversation }> {
    const conversations = loadMockConversations()
    const index = conversations.findIndex((item) => item.id === conversationId)
    if (index < 0) throw new Error('Conversation not found')
    const conversation = withMockContext(conversations[index])
    conversations[index] = conversation
    saveMockConversations(conversations)
    return { contextState: conversation.context_state ?? {}, conversation }
  },

  async compressContext(conversationId: string): Promise<{ contextState: ConversationContextState; conversation: Conversation }> {
    const conversations = loadMockConversations()
    const index = conversations.findIndex((item) => item.id === conversationId)
    if (index < 0) throw new Error('Conversation not found')
    const conversation = { ...conversations[index] }
    const cutoff = Math.max(0, conversation.messages.length - 8)
    const source = conversation.messages.slice(0, cutoff)
    if (source.length < 2) {
      throw new Error('没有足够的旧消息可以压缩')
    }
    const summary = {
      id: `ctxsum_dev_${crypto.randomUUID()}`,
      content: `Browser preview summary for ${source.length} older messages.`,
      source_message_ids: source.map((message) => message.id),
      source_until_message_id: source[source.length - 1]?.id ?? '',
      token_estimate_before: source.reduce((sum, message) => sum + estimateTokens(message.content || ''), 0),
      token_estimate_after: 20,
      created_at: nowSeconds(),
      provider_id: conversation.provider_id,
      model: conversation.model,
      stale: false,
    }
    const priorCount = conversation.context_state?.compression_count
      ?? conversation.contextState?.compressionCount
      ?? 0
    const boundary = {
      id: `ctxbd_dev_${crypto.randomUUID()}`,
      source_until_message_id: summary.source_until_message_id,
      token_estimate_before: summary.token_estimate_before,
      token_estimate_after: summary.token_estimate_after,
      summary_content: summary.content,
      trigger: 'manual' as const,
      created_at: summary.created_at,
    }
    const priorBoundaries = conversation.context_state?.compaction_boundaries
      ?? conversation.contextState?.compactionBoundaries
      ?? []
    const baseState = estimateMockContext(conversation)
    conversation.context_state = {
      ...baseState,
      status: 'compressed',
      summary,
      compaction_boundaries: [...priorBoundaries, boundary],
      last_compressed_at: summary.created_at,
      compressed_message_count: source.length,
      compression_count: priorCount + 1,
      segments: [
        ...(baseState.segments ?? []).filter((segment) => segment.id !== 'summarized_conversation'),
        { id: 'summarized_conversation', label: 'Summarized conversation', estimated_tokens: 20, color: '#BF3F66' },
      ],
    }
    conversation.contextState = conversation.context_state
    conversations[index] = conversation
    saveMockConversations(conversations)
    return { contextState: conversation.context_state, conversation }
  },
}

export const chatApi = {
  // 获取对话列表
  async getConversations(
    offset = 0,
    limit = 50,
    folder?: string,
    projectId?: string | null,
    setId?: string | null,
  ): Promise<ConversationListItem[]> {
    if (!isTauriRuntime()) return mockChatApi.getConversations(offset, limit, folder, projectId)
    const result = await invoke<{ success: boolean; conversations: ConversationListItem[] }>(
      'chat_get_conversations',
      { offset, limit, folder, projectId, setId }
    )
    if (!result.success) {
      throw new Error('Failed to get conversations')
    }
    return result.conversations
  },

  // 全量索引搜索对话（覆盖所有对话，不止侧栏默认加载的前 N 个）
  async searchConversations(query: string, limit = 30): Promise<ConversationSearchHit[]> {
    if (!isTauriRuntime()) return []
    const trimmed = query.trim()
    if (!trimmed) return []
    const result = await invoke<{ success: boolean; conversations: ConversationSearchHit[] }>(
      'chat_search_conversations',
      { query: trimmed, limit }
    )
    return result.success ? result.conversations : []
  },

  /** 对话库统一查询（筛选 / 排序 / 分页 + total） */
  async queryConversations(query: ConversationLibraryQuery = {}): Promise<ConversationLibraryPage> {
    if (!isTauriRuntime()) {
      const all = await mockChatApi.getConversations(0, 500)
      const slice = all.slice(query.offset ?? 0, (query.offset ?? 0) + (query.limit ?? 80))
      return { items: slice as ConversationSearchHit[], total: all.length }
    }
    const result = await invoke<{ success: boolean; items: ConversationSearchHit[]; total: number }>(
      'chat_query_conversations',
      {
        offset: query.offset ?? 0,
        limit: query.limit ?? 80,
        sort: query.sort ?? 'updated',
        order: query.order ?? 'desc',
        q: query.q ?? null,
        fullText: query.fullText ?? true,
        shelf: query.shelf ?? 'all',
        projectId: query.projectId ?? null,
        setId: query.setId ?? null,
        assistantId: query.assistantId ?? null,
        providerId: query.providerId ?? null,
        runtimeKind: query.runtimeKind ?? null,
      },
    )
    if (!result.success) throw new Error('Failed to query conversations')
    return { items: result.items ?? [], total: result.total ?? 0 }
  },

  async bulkUpdateConversations(
    ids: string[],
    patch: {
      pinned?: boolean
      archived?: boolean
      projectId?: string | null
      setId?: string | null
    },
  ): Promise<number> {
    if (!isTauriRuntime()) return 0
    if (ids.length === 0) return 0
    const hasProject = 'projectId' in patch
    const hasSet = 'setId' in patch
    const result = await invoke<{ success: boolean; updated: number }>(
      'chat_bulk_update_conversations',
      {
        ids,
        pinned: patch.pinned,
        archived: patch.archived,
        projectId: hasProject ? patch.projectId ?? '' : undefined,
        setId: hasSet ? patch.setId ?? '' : undefined,
      },
    )
    if (!result.success) throw new Error('Failed to bulk update conversations')
    return result.updated ?? 0
  },

  async bulkDeleteConversations(ids: string[]): Promise<{ deleted: number; warnings: string[] }> {
    if (!isTauriRuntime()) return { deleted: 0, warnings: [] }
    if (ids.length === 0) return { deleted: 0, warnings: [] }
    const result = await invoke<{ success: boolean; deleted: number; warnings: string[] }>(
      'chat_bulk_delete_conversations',
      { ids },
    )
    if (!result.success) throw new Error('Failed to bulk delete conversations')
    return { deleted: result.deleted ?? 0, warnings: result.warnings ?? [] }
  },

  // 获取对话详情
  async getConversation(conversationId: string): Promise<Conversation> {
    if (!isTauriRuntime()) return mockChatApi.getConversation(conversationId)
    const result = await invoke<{ success: boolean; conversation: Conversation }>(
      'chat_get_conversation',
      { conversationId }
    )
    if (!result.success) {
      throw new Error('Failed to get conversation')
    }
    return result.conversation
  },

  async exportConversationMarkdown(
    conversationId: string,
    path: string,
    language: 'zh' | 'en',
  ): Promise<void> {
    if (!isTauriRuntime()) throw new Error('Conversation export requires the desktop app')
    await invoke<void>('chat_export_conversation_markdown', {
      conversationId,
      path,
      language,
    })
  },

  // 创建新对话
  async createConversation(
    providerId?: string,
    model?: string,
    folder?: string,
    projectId?: string | null,
    assistantId?: string | null,
    setId?: string | null,
  ): Promise<Conversation> {
    if (!isTauriRuntime()) return mockChatApi.createConversation(providerId, model, folder, projectId, assistantId)
    const result = await invoke<{ success: boolean; conversation: Conversation }>(
      'chat_create_conversation',
      { providerId, model, folder, projectId, setId, assistantId }
    )
    if (!result.success) {
      throw new Error('Failed to create conversation')
    }
    return result.conversation
  },

  // 用预置的多轮历史 + 截图创建一个新会话（不触发回复）。Lens「在 AI 客户端继续」交接使用。
  async importExternalConversation(
    messages: { role: string; content: string }[],
    attachmentPaths: string[],
    providerId?: string,
    model?: string,
    projectId?: string | null,
  ): Promise<Conversation> {
    if (!isTauriRuntime()) return mockChatApi.createConversation(providerId, model, undefined, projectId, null)
    const result = await invoke<{ success: boolean; conversation: Conversation }>(
      'chat_import_external_conversation',
      { messages, attachments: attachmentPaths, providerId, model, projectId },
    )
    if (!result.success) {
      throw new Error('Failed to import external conversation')
    }
    return result.conversation
  },

  // 创建「对话搭建专家」会话(瞬态搭建助手,只暴露 save_assistant 工具)
  async createBuilderConversation(
    providerId?: string,
    model?: string,
    projectId?: string | null,
  ): Promise<Conversation> {
    if (!isTauriRuntime()) {
      // dev 浏览器无后端 LLM,搭建流程只在 Tauri 内真正可用;这里仅返回一个占位会话。
      return mockChatApi.createConversation(providerId, model, undefined, projectId, null)
    }
    const result = await invoke<{ success: boolean; conversation: Conversation }>(
      'chat_create_builder_conversation',
      { providerId, model, projectId }
    )
    if (!result.success) {
      throw new Error('Failed to create builder conversation')
    }
    return result.conversation
  },

  async getProjects(): Promise<ChatProject[]> {
    if (!isTauriRuntime()) return mockChatApi.getProjects()
    const result = await invoke<{ success: boolean; projects: ChatProject[] }>(
      'chat_get_projects',
    )
    if (!result.success) {
      throw new Error('Failed to get projects')
    }
    return result.projects
  },

  async getSets(): Promise<ChatSet[]> {
    if (!isTauriRuntime()) return []
    const result = await invoke<{ success: boolean; sets: ChatSet[] }>('chat_get_sets')
    if (!result.success) {
      throw new Error('Failed to get sets')
    }
    return result.sets
  },

  /** 侧栏手动顺序：只发 id 顺序，后端 load→重排→存，不覆盖其它字段（docs/adr/0004）。 */
  async reorderProjects(ids: string[]): Promise<ChatProject[]> {
    if (!isTauriRuntime()) return mockChatApi.getProjects()
    const result = await invoke<{ success: boolean; projects: ChatProject[] }>(
      'chat_reorder_projects',
      { ids },
    )
    if (!result.success) {
      throw new Error('Failed to reorder projects')
    }
    return result.projects
  },

  async reorderSets(ids: string[]): Promise<ChatSet[]> {
    if (!isTauriRuntime()) return []
    const result = await invoke<{ success: boolean; sets: ChatSet[] }>('chat_reorder_sets', {
      ids,
    })
    if (!result.success) {
      throw new Error('Failed to reorder sets')
    }
    return result.sets
  },

  /** 集/项目里对话的钉住位置（group_id → 钉子表）。见 chat/conversationPins.ts。 */
  async getConversationPins(): Promise<Record<string, ConversationPin[]>> {
    if (!isTauriRuntime()) return {}
    const result = await invoke<{ success: boolean; pins: Record<string, ConversationPin[]> }>(
      'chat_get_conversation_pins',
    )
    return result.success ? (result.pins ?? {}) : {}
  },

  async setConversationPins(groupId: string, pins: ConversationPin[]): Promise<void> {
    if (!isTauriRuntime()) return
    await invoke<{ success: boolean }>('chat_set_conversation_pins', { groupId, pins })
  },

  async createSet(
    name: string,
    systemPrompt?: string,
    defaultAssistantId?: string | null,
    color?: string | null,
  ): Promise<ChatSet> {
    if (!isTauriRuntime()) throw new Error('集功能仅在桌面应用内可用')
    const result = await invoke<{ success: boolean; set: ChatSet }>(
      'chat_create_set',
      { name, systemPrompt, defaultAssistantId, color },
    )
    if (!result.success) {
      throw new Error('Failed to create set')
    }
    return result.set
  },

  async updateSet(
    setId: string,
    updates: {
      name?: string
      systemPrompt?: string
      defaultAssistantId?: string | null
      color?: string | null
    },
  ): Promise<ChatSet> {
    if (!isTauriRuntime()) throw new Error('集功能仅在桌面应用内可用')
    const result = await invoke<{ success: boolean; set: ChatSet }>(
      'chat_update_set',
      {
        setId,
        name: updates.name,
        systemPrompt: updates.systemPrompt,
        systemPromptSet: updates.systemPrompt !== undefined,
        defaultAssistantId: updates.defaultAssistantId,
        defaultAssistantIdSet: updates.defaultAssistantId !== undefined,
        color: updates.color,
        colorSet: updates.color !== undefined,
      },
    )
    if (!result.success) {
      throw new Error('Failed to update set')
    }
    return result.set
  },

  async deleteSet(setId: string): Promise<void> {
    if (!isTauriRuntime()) return
    const result = await invoke<{ success: boolean }>('chat_delete_set', { setId })
    if (!result.success) {
      throw new Error('Failed to delete set')
    }
  },

  async createProject(
    name: string,
    description?: string | null,
    color?: string | null,
    rootPath?: string | null,
    options?: { ensureRootDir?: boolean },
  ): Promise<ChatProject> {
    if (!isTauriRuntime()) return mockChatApi.createProject(name, description, color, rootPath)
    const result = await invoke<{ success: boolean; project: ChatProject }>(
      'chat_create_project',
      {
        name,
        description,
        color,
        rootPath,
        ensureRootDir: options?.ensureRootDir ?? false,
      },
    )
    if (!result.success) {
      throw new Error('Failed to create project')
    }
    return result.project
  },

  async updateProject(
    projectId: string,
    updates: { name?: string; description?: string | null; color?: string | null; rootPath?: string | null },
  ): Promise<ChatProject> {
    if (!isTauriRuntime()) return mockChatApi.updateProject(projectId, updates)
    const hasDescriptionUpdate = 'description' in updates
    const hasColorUpdate = 'color' in updates
    const hasRootPathUpdate = 'rootPath' in updates
    const result = await invoke<{ success: boolean; project: ChatProject }>(
      'chat_update_project',
      {
        projectId,
        name: updates.name,
        description: hasDescriptionUpdate ? updates.description : undefined,
        descriptionSet: hasDescriptionUpdate,
        color: hasColorUpdate ? updates.color : undefined,
        colorSet: hasColorUpdate,
        rootPath: hasRootPathUpdate ? updates.rootPath : undefined,
        rootPathSet: hasRootPathUpdate,
      },
    )
    if (!result.success) {
      throw new Error('Failed to update project')
    }
    return result.project
  },

  async deleteProject(projectId: string): Promise<void> {
    if (!isTauriRuntime()) return mockChatApi.deleteProject(projectId)
    const result = await invoke<{ success: boolean }>('chat_delete_project', { projectId })
    if (!result.success) {
      throw new Error('Failed to delete project')
    }
  },

  async openProjectFolder(projectId: string): Promise<void> {
    if (!isTauriRuntime()) return mockChatApi.openProjectFolder(projectId)
    const result = await invoke<{ success: boolean; error?: string }>(
      'chat_project_open_folder',
      { projectId },
    )
    if (!result.success) {
      throw new Error('打开项目文件夹失败')
    }
  },

  async getAssistants(): Promise<ChatAssistant[]> {
    if (!isTauriRuntime()) return mockChatApi.getAssistants()
    const result = await invoke<{ success: boolean; assistants: ChatAssistant[] }>(
      'chat_get_assistants',
    )
    if (!result.success) {
      throw new Error('Failed to get assistants')
    }
    return result.assistants
  },

  async createAssistant(assistant: ChatAssistant): Promise<ChatAssistant> {
    if (!isTauriRuntime()) return mockChatApi.createAssistant(assistant)
    const result = await invoke<{ success: boolean; assistant: ChatAssistant }>(
      'chat_create_assistant',
      { assistant },
    )
    if (!result.success) {
      throw new Error('Failed to create assistant')
    }
    return result.assistant
  },

  async updateAssistant(assistant: ChatAssistant): Promise<ChatAssistant> {
    if (!isTauriRuntime()) return mockChatApi.updateAssistant(assistant)
    const result = await invoke<{ success: boolean; assistant: ChatAssistant }>(
      'chat_update_assistant',
      { assistant },
    )
    if (!result.success) {
      throw new Error('Failed to update assistant')
    }
    return result.assistant
  },

  async duplicateAssistant(assistantId: string): Promise<ChatAssistant> {
    if (!isTauriRuntime()) return mockChatApi.duplicateAssistant(assistantId)
    const result = await invoke<{ success: boolean; assistant: ChatAssistant }>(
      'chat_duplicate_assistant',
      { assistantId },
    )
    if (!result.success) {
      throw new Error('Failed to duplicate assistant')
    }
    return result.assistant
  },

  async deleteAssistant(assistantId: string): Promise<void> {
    if (!isTauriRuntime()) return mockChatApi.deleteAssistant(assistantId)
    const result = await invoke<{ success: boolean }>('chat_delete_assistant', { assistantId })
    if (!result.success) {
      throw new Error('Failed to delete assistant')
    }
  },

  // 发送消息
  async sendMessage(
    conversationId: string,
    content: string,
    attachments: PendingAttachment[] = [],
    activeSkillId?: string | null,
  ): Promise<Conversation> {
    if (!isTauriRuntime()) {
      return mockChatApi.sendMessage(conversationId, content, attachments, activeSkillId)
    }
    // 磁盘附件传路径；内存文本附件（粘贴长文本虚拟 txt）直接传内容，由后端注入 prompt，不落盘。
    const diskPaths = attachments.filter((a) => a.content === undefined).map((a) => a.path)
    const textAttachments = attachments
      .filter((a): a is PendingAttachment & { content: string } => a.content !== undefined)
      .map((a) => ({ name: a.name, content: a.content }))
    const result = await invoke<{ success: boolean; conversation?: Conversation; error?: string }>(
      'chat_send_message',
      {
        conversationId,
        content,
        attachments: diskPaths,
        textAttachments,
        activeSkillId,
      }
    )
    if (!result.success || !result.conversation) {
      const error: Error & { conversation?: Conversation } = new Error(
        result.error || 'Failed to send message',
      )
      // 生成失败但后端保留了用户消息（不再回滚）：把对话挂到错误上，调用方据此让问题留在线程里、可重试。
      if (result.conversation) {
        error.conversation = result.conversation
      }
      throw error
    }
    return result.conversation
  },

  /**
   * 运行中「立刻引导」：把这条消息投进后端 steering 信箱，等当前 run 的下一个轮次边界注入。
   *
   * 返回 false = 没进信箱（该会话此刻没有活跃 run / 文本为空），调用方应改走普通发送。
   * 返回 true 只表示**已投递**，不表示已生效——真正生效的信号是那张 `user_steer` 工具卡事件
   * （模型可能已经在写终答、后面再没有轮次边界，那条就得由运行结束后的队列自动发送兜住）。
   */
  async steerMessage(
    conversationId: string,
    steerId: string,
    content: string,
    attachments: PendingAttachment[] = [],
  ): Promise<boolean> {
    if (!isTauriRuntime()) return false
    const textAttachments = attachments
      .filter((attachment): attachment is PendingAttachment & { content: string } => (
        attachment.content !== undefined
      ))
      .map((attachment) => ({ name: attachment.name, content: attachment.content }))
    return await invoke<boolean>('chat_steer_message', {
      conversationId,
      steerId,
      content,
      textAttachments,
    })
  },

  // 删除对话。返回未能清理的副产物说明（工作区被占用等）——对话本身一定已经删掉了，
  // 后端只有在「对话文件 / 索引」这两步失败时才抛错。
  async deleteConversation(conversationId: string): Promise<string[]> {
    if (!isTauriRuntime()) {
      await mockChatApi.deleteConversation(conversationId)
      return []
    }
    const result = await invoke<{ success: boolean; warnings?: string[] }>(
      'chat_delete_conversation',
      { conversationId },
    )
    if (!result.success) {
      throw new Error('Failed to delete conversation')
    }
    return result.warnings ?? []
  },

  // 更新对话
  async updateConversation(
    conversationId: string,
    updates: {
      title?: string
      pinned?: boolean
      archived?: boolean
      folder?: string
      projectId?: string | null
      setId?: string | null
      providerId?: string
      model?: string
      activeSkillId?: string | null
      assistantId?: string | null
      knowledgeBaseIds?: string[]
      forceKnowledgeSearch?: boolean
      thinkingLevel?: ThinkingLevel | null
      webSearchMode?: WebSearchMode | null
      replyModels?: ModelRef[]
    }
  ): Promise<Conversation> {
    if (!isTauriRuntime()) return mockChatApi.updateConversation(conversationId, updates)
    const hasFolderUpdate = 'folder' in updates
    const hasProjectUpdate = 'projectId' in updates
    const hasSetUpdate = 'setId' in updates
    const hasThinkingUpdate = 'thinkingLevel' in updates
    const hasWebSearchUpdate = 'webSearchMode' in updates
    const result = await invoke<{ success: boolean; conversation: Conversation }>(
      'chat_update_conversation',
      {
        conversationId,
        title: updates.title,
        pinned: updates.pinned,
        archived: updates.archived,
        folder: hasFolderUpdate ? updates.folder ?? '' : undefined,
        projectId: hasProjectUpdate ? updates.projectId ?? '' : undefined,
        setId: hasSetUpdate ? updates.setId ?? '' : undefined,
        providerId: updates.providerId,
        model: updates.model,
        activeSkillId: updates.activeSkillId,
        assistantId: updates.assistantId,
        knowledgeBaseIds: updates.knowledgeBaseIds,
        forceKnowledgeSearch: updates.forceKnowledgeSearch,
        // null/未知 → 空串，后端解析为 None（回到「跟随全局」）。
        thinkingLevel: hasThinkingUpdate ? updates.thinkingLevel ?? '' : undefined,
        // 会话级三态联网搜索（任务 07-23）：null/未知 → 空串，后端回退全局开关。
        webSearchMode: hasWebSearchUpdate ? updates.webSearchMode ?? '' : undefined,
        // 多模型一问多答（任务 06-30）：持久化会话级多答模型集（决策 D2/D4）。
        replyModels: updates.replyModels,
      }
    )
    if (!result.success) {
      throw new Error('Failed to update conversation')
    }
    return result.conversation
  },

  async updateMessage(
    conversationId: string,
    messageId: string,
    content: string,
  ): Promise<Conversation> {
    if (!isTauriRuntime()) {
      return mockChatApi.updateMessage(conversationId, messageId, content)
    }
    const result = await invoke<{
      success: boolean
      conversation?: Conversation
      error?: string
    }>('chat_update_message', { conversationId, messageId, content })
    if (!result.success || !result.conversation) {
      throw new Error(result.error || 'Failed to update message')
    }
    return result.conversation
  },

  async deleteMessage(conversationId: string, messageId: string): Promise<Conversation> {
    if (!isTauriRuntime()) {
      return mockChatApi.deleteMessage(conversationId, messageId)
    }
    const result = await invoke<{
      success: boolean
      conversation?: Conversation
      error?: string
    }>('chat_delete_message', { conversationId, messageId })
    if (!result.success || !result.conversation) {
      throw new Error(result.error || 'Failed to delete message')
    }
    return result.conversation
  },

  // 多模型一问多答（任务 06-30）：设置某多答组的「选中条」（续聊以它为准，决策 D5）。
  async setGroupSelection(
    conversationId: string,
    groupId: string,
    messageId: string,
  ): Promise<Conversation> {
    if (!isTauriRuntime()) {
      return mockChatApi.getConversation(conversationId)
    }
    const result = await invoke<{
      success: boolean
      conversation?: Conversation
      error?: string
    }>('chat_set_group_selection', { conversationId, groupId, messageId })
    if (!result.success || !result.conversation) {
      throw new Error(result.error || 'Failed to set group selection')
    }
    return result.conversation
  },

  async regenerateMessage(conversationId: string, messageId: string, newContent?: string): Promise<Conversation> {
    if (!isTauriRuntime()) {
      return mockChatApi.regenerateMessage(conversationId, messageId, newContent)
    }
    const result = await invoke<{
      success: boolean
      conversation?: Conversation
      error?: string
    }>('chat_regenerate_message', { conversationId, messageId, newContent: newContent ?? null })
    if (!result.success || !result.conversation) {
      throw new Error(result.error || 'Failed to regenerate message')
    }
    return result.conversation
  },

  // 一键 rewind：删掉该用户提问及其之后的消息，返回新会话 + 被删掉的原文（前端塞回输入框）。
  async rewindToMessage(
    conversationId: string,
    messageId: string,
  ): Promise<{ conversation: Conversation; content: string }> {
    if (!isTauriRuntime()) {
      return mockChatApi.rewindToMessage(conversationId, messageId)
    }
    const result = await invoke<{
      success: boolean
      conversation?: Conversation
      content?: string
      error?: string
    }>('chat_rewind_to_message', { conversationId, messageId })
    if (!result.success || !result.conversation) {
      throw new Error(result.error || 'Failed to rewind conversation')
    }
    return { conversation: result.conversation, content: result.content ?? '' }
  },

  // 对话分支（方案 B）：把源对话某消息及其之前的消息复制进一个新对话，返回新对话。不自动发送。
  async forkConversation(conversationId: string, messageId: string): Promise<Conversation> {
    if (!isTauriRuntime()) {
      return mockChatApi.createConversation(undefined, undefined, undefined, null, null)
    }
    const result = await invoke<{
      success: boolean
      conversation?: Conversation
      error?: string
    }>('chat_fork_conversation', { conversationId, messageId })
    if (!result.success || !result.conversation) {
      throw new Error(result.error || 'Failed to fork conversation')
    }
    return result.conversation
  },

  async getContextStats(conversationId: string): Promise<{ contextState: ConversationContextState; conversation: Conversation }> {
    if (!isTauriRuntime()) return mockChatApi.getContextStats(conversationId)
    const result = await invoke<{
      success: boolean
      contextState?: ConversationContextState
      conversation?: Conversation
      error?: string
    }>('chat_get_context_stats', { conversationId })
    if (!result.success || !result.contextState || !result.conversation) {
      throw new Error(result.error || 'Failed to get context stats')
    }
    return { contextState: result.contextState, conversation: result.conversation }
  },

  async compressContext(conversationId: string): Promise<{ contextState: ConversationContextState; conversation: Conversation }> {
    if (!isTauriRuntime()) return mockChatApi.compressContext(conversationId)
    const result = await invoke<{
      success: boolean
      contextState?: ConversationContextState
      conversation?: Conversation
      error?: string
    }>('chat_compress_context', { conversationId })
    if (!result.success || !result.contextState || !result.conversation) {
      throw new Error(result.error || 'Failed to compress context')
    }
    return { contextState: result.contextState, conversation: result.conversation }
  },

  async setAgentPlanMode(conversationId: string, mode: AgentPlanMode): Promise<Conversation> {
    if (!isTauriRuntime()) return mockChatApi.setAgentPlanMode(conversationId, mode)
    const result = await invoke<{ success: boolean; conversation?: Conversation; error?: string }>(
      'chat_set_agent_plan_mode',
      { conversationId, mode },
    )
    if (!result.success || !result.conversation) {
      throw new Error(result.error || 'Failed to set plan mode')
    }
    return result.conversation
  },

  async executeAgentPlan(conversationId: string, messageId?: string): Promise<Conversation> {
    if (!isTauriRuntime()) return mockChatApi.executeAgentPlan(conversationId, messageId)
    const result = await invoke<{ success: boolean; conversation?: Conversation; error?: string }>(
      'chat_execute_agent_plan',
      { conversationId, messageId },
    )
    if (!result.success || !result.conversation) {
      throw new Error(result.error || 'Failed to execute plan')
    }
    return result.conversation
  },

  async cancelStream(conversationId: string): Promise<void> {
    if (!isTauriRuntime()) return
    await invoke<void>('chat_cancel_stream', { conversationId })
  },

  async detectExternalAgents(
    forceRefresh = false,
    conversationId?: string | null,
  ): Promise<DetectedExternalAgent[]> {
    if (!isTauriRuntime()) {
      return [
        {
          id: 'claude',
          name: 'Claude Code',
          available: false,
          models: [{ id: 'default', label: 'Default' }],
        },
      ]
    }
    const result = await invoke<{ success: boolean; agents: DetectedExternalAgent[] }>(
      'chat_detect_external_agents',
      { forceRefresh, conversationId },
    )
    return result.agents ?? []
  },

  // 懒查：只探选中 agent 的模型（cwd-scoped）。列表阶段不查模型，避免对所有 CLI 跑昂贵探测。
  async detectExternalAgentModels(
    agentId: string,
    conversationId?: string | null,
    force = false,
  ): Promise<{
    models: DetectedExternalAgent['models']
    reasoningOptions: NonNullable<DetectedExternalAgent['reasoningOptions']>
    /** 按模型的 effort 档位（kimi：K3 有 low/high/max，always_thinking 模型无）。 */
    reasoningByModel: Record<string, NonNullable<DetectedExternalAgent['reasoningOptions']>>
    source: 'probed' | 'fallback'
    probeError?: string
    // CLI 自己当前配置的模型/推理等级（用于胶囊自动同步「同步 CLI 当前配置」）。null = 无当前概念。
    currentModel?: string | null
    currentReasoning?: string | null
  }> {
    if (!isTauriRuntime()) {
      return { models: [], reasoningOptions: [], reasoningByModel: {}, source: 'probed' }
    }
    const result = await invoke<{
      success: boolean
      models?: DetectedExternalAgent['models']
      reasoningOptions?: NonNullable<DetectedExternalAgent['reasoningOptions']>
      reasoningByModel?: Record<string, NonNullable<DetectedExternalAgent['reasoningOptions']>>
      source?: 'probed' | 'fallback'
      probeError?: string
      currentModel?: string | null
      currentReasoning?: string | null
    }>('chat_detect_external_agent_models', { agentId, conversationId, force })
    return {
      models: result.models ?? [],
      reasoningOptions: result.reasoningOptions ?? [],
      reasoningByModel: result.reasoningByModel ?? {},
      // 向后兼容：旧后端不返回 source 时视为 probed（不显示降级角标）。
      source: result.source ?? 'probed',
      probeError: result.probeError,
      currentModel: result.currentModel ?? null,
      currentReasoning: result.currentReasoning ?? null,
    }
  },

  /** 设置页「本地 CLI Agent」：本地版本 / npm 最新版 / 安装命令 / 配置目录。 */
  async externalCliInstallInfo(agentId: string): Promise<ExternalCliInstallInfo | null> {
    if (!isTauriRuntime()) return null
    return await invoke<ExternalCliInstallInfo>('chat_external_cli_install_info', { agentId })
  },

  /** 跑安装/更新命令；日志通过 `external-cli-install` 事件流回来（见 onExternalCliInstallLog）。 */
  async externalCliInstall(agentId: string): Promise<void> {
    if (!isTauriRuntime()) return
    await invoke('chat_external_cli_install', { agentId })
  },

  async externalCliOpenConfigDir(agentId: string): Promise<void> {
    if (!isTauriRuntime()) return
    await invoke('chat_external_cli_open_config_dir', { agentId })
  },

  async dshPluginSettingsGet(): Promise<DshPluginSettingsSnapshot | null> {
    if (!isTauriRuntime()) return null
    return await invoke<DshPluginSettingsSnapshot>('chat_dsh_plugin_settings_get')
  },

  async dshPluginSettingsSave(patch: DshPluginSettingsPatch): Promise<DshPluginSettingsSnapshot> {
    return await invoke<DshPluginSettingsSnapshot>('chat_dsh_plugin_settings_save', { patch })
  },

  async dshPluginInventory(): Promise<DshPluginEntry[]> {
    if (!isTauriRuntime()) return []
    return await invoke<DshPluginEntry[]>('chat_dsh_plugin_inventory')
  },

  async dshOpenSettingsFile(): Promise<void> {
    if (!isTauriRuntime()) return
    await invoke('chat_dsh_open_settings_file')
  },

  async dshOfficialCredentialStatus(): Promise<DshOfficialCredential> {
    if (!isTauriRuntime()) return { configured: false, writable: true }
    return await invoke<DshOfficialCredential>('chat_dsh_official_credential_status')
  },

  async dshOfficialCredentialSave(apiKey: string): Promise<DshOfficialCredential> {
    return await invoke<DshOfficialCredential>('chat_dsh_official_credential_save', { apiKey })
  },

  async dshNativeProviderGet(id: string): Promise<DshNativeProviderDetail> {
    return await invoke<DshNativeProviderDetail>('chat_dsh_native_provider_get', { id })
  },

  async dshNativeProviderDelete(id: string): Promise<void> {
    if (!isTauriRuntime()) return
    await invoke('chat_dsh_native_provider_delete', { id })
  },

  /**
   * 删除供应商后清掉它物化出来的文件。
   * 保存设置时后端会自动物化并清缓存（`persist_settings`），所以只有删除需要显式调用。
   */
  async externalCliProviderCleanup(
    agentId: string,
    providerId: string,
    nativeProviderId?: string,
    providerName?: string,
  ): Promise<void> {
    if (!isTauriRuntime()) return
    await invoke('chat_external_cli_provider_cleanup', {
      agentId,
      providerId,
      nativeProviderId,
      providerName,
    })
  },

  /** 供应商弹窗的「获取模型」：拿 base_url + key 去中转站问模型列表（只作建议用）。 */
  async externalCliFetchRelayModels(baseUrl: string, apiKey: string): Promise<string[]> {
    if (!isTauriRuntime()) return []
    return await invoke<string[]>('chat_external_cli_fetch_relay_models', { baseUrl, apiKey })
  },

  /** 扫描本机 cc-switch 的库，列出可导入的供应商（只读）。 */
  async externalCliScanCcSwitch(): Promise<CcSwitchScan> {
    if (!isTauriRuntime()) return { providers: [], skipped: 0 }
    return await invoke<CcSwitchScan>('chat_external_cli_scan_cc_switch')
  },

  async listExternalCliSlashCommands(
    agentId: string,
    conversationId?: string | null,
  ): Promise<import('./externalCliSlashCommands').ExternalCliSlashCommandsResult> {
    if (!isTauriRuntime()) {
      return { supportsSlashCommands: false, commands: [], message: 'CLI slash commands unavailable in browser preview' }
    }
    const result = await invoke<{
      success: boolean
      supportsSlashCommands: boolean
      commands: import('./externalCliSlashCommands').ExternalCliSlashCommandDto[]
      message?: string | null
    }>('chat_list_external_cli_slash_commands', {
      agentId,
      conversationId: conversationId ?? null,
    })
    return {
      supportsSlashCommands: result.supportsSlashCommands,
      commands: result.commands ?? [],
      message: result.message ?? null,
    }
  },

  async setAgentRuntime(
    conversationId: string,
    agentRuntime: AgentRuntimeConfig,
  ): Promise<Conversation> {
    if (!isTauriRuntime()) {
      const conversations = loadMockConversations()
      const index = conversations.findIndex((item) => item.id === conversationId)
      if (index < 0) throw new Error('Conversation not found')
      conversations[index] = {
        ...conversations[index],
        agent_runtime: agentRuntime,
        agentRuntime: agentRuntime,
        updated_at: nowSeconds(),
      }
      saveMockConversations(conversations)
      return conversations[index]
    }
    const payload = {
      kind: agentRuntime.kind,
      externalAgentId: agentRuntime.externalAgentId ?? null,
      externalModel: agentRuntime.externalModel ?? null,
      externalReasoning: agentRuntime.externalReasoning ?? null,
      externalSandbox: agentRuntime.externalSandbox ?? null,
      externalAgentPreset: agentRuntime.externalAgentPreset ?? null,
    }
    const result = await invoke<{ success: boolean; conversation: Conversation }>(
      'chat_set_agent_runtime',
      { conversationId, agentRuntime: payload },
    )
    if (!result.success) {
      throw new Error('Failed to set agent runtime')
    }
    return result.conversation
  },

  // ── 从本地 CLI 导入对话 ───────────────────────────────────────────────────
  // 导入是**项目内的动作**：只列出工作目录等于该项目根的原生会话，导入后仍由原 CLI 续聊。
  // 契约见 docs/adr/0001..0003。

  async listImportableCliSessions(projectId: string): Promise<ImportableCliSession[]> {
    if (!isTauriRuntime()) return []
    const result = await invoke<{ success: boolean; sessions: ImportableCliSession[] }>(
      'chat_list_importable_cli_sessions',
      { projectId },
    )
    return result.sessions ?? []
  },

  async importCliSessions(
    projectId: string,
    items: { agentId: string; sessionId: string }[],
  ): Promise<CliImportResult> {
    if (!isTauriRuntime()) return { success: false, imported: [], failures: [] }
    return invoke<CliImportResult>('chat_import_cli_sessions', { projectId, items })
  },

  // 打开已导入的对话时问一次：CLI 那边有没有新内容。只提示，不同步（ADR-0002）。
  async importedHistoryStale(conversationId: string): Promise<boolean> {
    if (!isTauriRuntime()) return false
    return invoke<boolean>('chat_imported_history_stale', { conversationId })
  },
}
