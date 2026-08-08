import { isChatOnboardingPath } from './persistence'

/**
 * 聊天窗口的 hash 路由判定与解析。
 *
 * 纯函数，单列一个 .ts 模块：既便于直接单测，也避免从组件文件导出非组件符号
 * （会破坏 React Fast Refresh，见 settings/memoryLayers.ts 的同类处理）。
 */

export function hashPath(): string {
  return window.location.hash.replace('#', '').split('?')[0]
}

export function isChatSettingsPath(path: string): boolean {
  return path === 'chat/settings' || path.startsWith('chat/settings/')
}

export function isChatAssistantCenterPath(path: string): boolean {
  return path === 'chat/assistants' || path.startsWith('chat/assistants/')
}

export function isChatOnboardingRoute(path: string): boolean {
  return isChatOnboardingPath(path)
}

export function isChatSkillCenterPath(path: string): boolean {
  return path === 'chat/skill' || path.startsWith('chat/skill/')
}

/** @deprecated 插件已迁入设置；保留判定用于把旧 `#chat/plugins` 重定向到设置 → 插件。 */
export function isChatPluginCenterPath(path: string): boolean {
  return path === 'chat/plugins' || path.startsWith('chat/plugins/')
}

export function isChatSessionCenterPath(path: string): boolean {
  return path === 'chat/sessions' || path.startsWith('chat/sessions/')
}

export function isChatMcpCenterPath(path: string): boolean {
  return path === 'chat/mcp' || path.startsWith('chat/mcp/')
}

export function isChatKnowledgeCenterPath(path: string): boolean {
  return path === 'chat/knowledge' || path.startsWith('chat/knowledge/')
}

export function isChatNotesPath(path: string): boolean {
  return path === 'chat/notes' || path.startsWith('chat/notes/')
}

/**
 * 从当前 hash 解析会话 id；非会话路由返回 null。
 * 中心页（settings / assistants / skill / mcp / notes / sessions / plugins / …）一律排除。
 */
export function getRouteConversationId(): string | null {
  const path = hashPath()
  if (!path.startsWith('chat/')) return null
  const rest = path.slice('chat/'.length)
  if (rest === 'settings' || rest.startsWith('settings/')) return null
  if (rest === 'assistants' || rest.startsWith('assistants/')) return null
  if (rest === 'skill' || rest.startsWith('skill/')) return null
  if (rest === 'knowledge' || rest.startsWith('knowledge/')) return null
  if (rest === 'sessions' || rest.startsWith('sessions/')) return null
  if (rest === 'plugins' || rest.startsWith('plugins/')) return null
  if (rest === 'mcp' || rest.startsWith('mcp/')) return null
  if (rest === 'notes' || rest.startsWith('notes/')) return null
  if (rest === 'onboarding' || rest.startsWith('onboarding/')) return null
  return decodeURIComponent(rest)
}

/** 把 hash 换成目标值；已是目标值则不写（避免多余的 hashchange）。 */
export function setHash(next: string): void {
  if (window.location.hash !== next) {
    window.location.hash = next
  }
}

export function conversationHash(conversationId: string | null): string {
  return conversationId ? `#chat/${encodeURIComponent(conversationId)}` : '#chat'
}
