import type { Conversation, ConversationListItem } from '../types'

/** 主窗卸掉消息堆时用的轻量会话：只留侧栏高亮和顶栏选择器用的元数据。 */
export function emptyPopoutConversation(
  id: string,
  source?: Conversation | ConversationListItem | null,
): Conversation {
  const runtime = source?.agent_runtime ?? source?.agentRuntime
  return {
    id,
    revision: source?.revision ?? 0,
    title: source?.title ?? '',
    provider_id: source?.provider_id ?? '',
    model: source?.model ?? '',
    messages: [],
    created_at: source?.created_at ?? 0,
    updated_at: source?.updated_at ?? 0,
    pinned: source?.pinned,
    archived: source?.archived,
    folder: source?.folder,
    project_id: source?.project_id ?? source?.projectId,
    projectId: source?.project_id ?? source?.projectId,
    set_id: source?.set_id ?? source?.setId,
    setId: source?.set_id ?? source?.setId,
    assistant_id: source?.assistant_id ?? source?.assistantId,
    assistantId: source?.assistant_id ?? source?.assistantId,
    agent_runtime: runtime,
    agentRuntime: runtime,
  }
}

export function stripConversationMessages(conversation: Conversation): Conversation {
  if (conversation.messages.length === 0) return conversation
  return { ...conversation, messages: [] }
}
