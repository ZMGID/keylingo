export function isChatPopoutPath(path: string): boolean {
  return path === 'chat/popout' || path.startsWith('chat/popout/')
}

export function popoutConversationHash(conversationId: string): string {
  return `#chat/popout/${encodeURIComponent(conversationId)}`
}

export function getPopoutConversationIdFromPath(path: string): string | null {
  if (!path.startsWith('chat/popout/')) return null
  const rest = path.slice('chat/popout/'.length)
  if (!rest) return null
  return decodeURIComponent(rest)
}

export function getPopoutConversationId(): string | null {
  const path = window.location.hash.replace('#', '').split('?')[0]
  return getPopoutConversationIdFromPath(path)
}
