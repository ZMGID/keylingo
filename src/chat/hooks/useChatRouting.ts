import { useCallback, useEffect } from 'react'
import {
  conversationHash,
  getRouteConversationId,
  hashPath,
  isChatAssistantCenterPath,
  isChatAutomationsPath,
  isChatKnowledgeCenterPath,
  isChatMcpCenterPath,
  isChatNotesPath,
  isChatOnboardingRoute,
  isChatPluginCenterPath,
  isChatSessionCenterPath,
  isChatSettingsPath,
  isChatSkillCenterPath,
  setHash,
} from '../chatRoutes'

type ChatView =
  | 'conversation' | 'settings' | 'assistants' | 'skill'
  | 'mcp' | 'knowledge' | 'notes' | 'automations' | 'onboarding'

interface UseChatRoutingParams {
  onViewChange: (view: ChatView) => void
  /** 会话路由命中且需要加载时调用（已是当前会话则不会触发，见 loadFromRoute 注释）。 */
  onLoadConversation: (conversationId: string) => void
  /** 路由指向空会话时的重置动作。 */
  onResetConversation: () => void
  /** 读当前会话 id，用于跳过「刚 apply 完又被路由重载一遍」的双读。 */
  currentConversationIdRef: React.MutableRefObject<string | null>
  /** 旧 `#chat/plugins` 入口：插件已迁入设置，重定向到设置 → 插件。 */
  onOpenPluginsSettings?: () => void
  /** 旧 `#chat/sessions` 入口：对话库已迁入设置，重定向到设置 → 对话库。 */
  onOpenSessionsSettings?: () => void
}

/**
 * 聊天窗口的 hash 路由。
 *
 * 对外只写 view 与会话加载两件事，不持有任何自己的状态 —— 这是 Chat.tsx 里
 * 边界最干净的一簇，故作为抽 hook 的第一步。
 *
 * 时序保持与搬迁前一致：挂载时立刻 loadFromRoute() 一次，再订阅 hashchange。
 */
export function useChatRouting({
  onViewChange,
  onLoadConversation,
  onResetConversation,
  currentConversationIdRef,
  onOpenPluginsSettings,
  onOpenSessionsSettings,
}: UseChatRoutingParams) {
  const syncConversationRoute = useCallback((conversationId: string | null) => {
    setHash(conversationHash(conversationId))
  }, [])

  const syncSettingsRoute = useCallback(() => setHash('#chat/settings'), [])
  const syncOnboardingRoute = useCallback(() => setHash('#chat/onboarding'), [])
  const syncAssistantCenterRoute = useCallback(() => setHash('#chat/assistants'), [])
  const syncSkillCenterRoute = useCallback(() => setHash('#chat/skill'), [])
  const syncMcpCenterRoute = useCallback(() => setHash('#chat/mcp'), [])
  const syncKnowledgeCenterRoute = useCallback(() => setHash('#chat/knowledge'), [])
  const syncNotesRoute = useCallback(() => setHash('#chat/notes'), [])
  const syncAutomationsRoute = useCallback(() => setHash('#chat/automations'), [])

  useEffect(() => {
    const loadFromRoute = () => {
      const path = hashPath()
      if (isChatOnboardingRoute(path)) {
        onViewChange('onboarding')
        return
      }
      if (isChatSettingsPath(path)) {
        onViewChange('settings')
        return
      }
      if (isChatAssistantCenterPath(path)) {
        onViewChange('assistants')
        return
      }
      if (isChatSkillCenterPath(path)) {
        onViewChange('skill')
        return
      }
      if (isChatMcpCenterPath(path)) {
        onViewChange('mcp')
        return
      }
      if (isChatKnowledgeCenterPath(path)) {
        onViewChange('knowledge')
        return
      }
      if (isChatNotesPath(path)) {
        onViewChange('notes')
        return
      }
      if (isChatAutomationsPath(path)) {
        onViewChange('automations')
        return
      }
      // 对话库已迁入设置；旧链接 `#chat/sessions` 重定向
      if (isChatSessionCenterPath(path)) {
        onOpenSessionsSettings?.()
        return
      }
      // 插件已迁入设置；旧链接 `#chat/plugins` 重定向
      if (isChatPluginCenterPath(path)) {
        onOpenPluginsSettings?.()
        return
      }
      onViewChange('conversation')
      const conversationId = getRouteConversationId()
      if (!conversationId) {
        onResetConversation()
        return
      }
      // 已是当前会话：说明这次 hash 变化来自点击/创建/分支等「先加载并 apply、再同步路由」的
      // 路径，数据刚落进 state，此处再 force 重载只会让同一对话白读一遍盘（双重 IPC）。
      // 真正的路由导航（前进/后退/启动恢复/外部改 hash）ref 必然不同，照常加载。
      if (currentConversationIdRef.current === conversationId) return
      onLoadConversation(conversationId)
    }
    loadFromRoute()
    window.addEventListener('hashchange', loadFromRoute)
    return () => window.removeEventListener('hashchange', loadFromRoute)
  }, [
    currentConversationIdRef,
    onLoadConversation,
    onOpenPluginsSettings,
    onOpenSessionsSettings,
    onResetConversation,
    onViewChange,
  ])

  return {
    syncConversationRoute,
    syncSettingsRoute,
    syncOnboardingRoute,
    syncAssistantCenterRoute,
    syncSkillCenterRoute,
    syncMcpCenterRoute,
    syncKnowledgeCenterRoute,
    syncNotesRoute,
    syncAutomationsRoute,
  }
}
