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

export function isChatPluginCenterPath(path: string): boolean {
  return path === 'chat/plugins' || path.startsWith('chat/plugins/')
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
 *
 * 注意排除清单里没有 mcp / notes / plugins —— 这是搬迁前的既有行为，原样保留。
 * 这三条路由由 loadFromRoute 里更靠前的分支拦截，走不到解析会话这一步。
 */
export function getRouteConversationId(): string | null {
  const path = hashPath()
  if (!path.startsWith('chat/')) return null
  const rest = path.slice('chat/'.length)
  if (rest === 'settings' || rest.startsWith('settings/')) return null
  if (rest === 'assistants' || rest.startsWith('assistants/')) return null
  if (rest === 'skill' || rest.startsWith('skill/')) return null
  if (rest === 'knowledge' || rest.startsWith('knowledge/')) return null
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
