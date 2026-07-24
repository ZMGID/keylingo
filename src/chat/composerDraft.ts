import type { PendingAttachment } from './types'

/**
 * 输入框草稿的内存暂存：切换对话/页面导致 InputBar 卸载重挂时，保住已打的字、引用与附件。
 * 按会话 id 存（新建对话用 NEW_CHAT_KEY）。仅进程内存活——app 关窗销毁即清，符合"本次使用期间"语义。
 * ponytail: 内存 Map 足够；要跨 app 重启保留再换 sessionStorage（附件 temp 路径届时可能已被 GC，需另处理）。
 */
export interface ComposerDraft {
  input: string
  quotes: string[]
  attachments: PendingAttachment[]
}

const NEW_CHAT_KEY = '__new__'
const drafts = new Map<string, ComposerDraft>()

export function draftKey(conversationId: string | null | undefined): string {
  return conversationId || NEW_CHAT_KEY
}

export function getComposerDraft(key: string): ComposerDraft | undefined {
  return drafts.get(key)
}

export function setComposerDraft(key: string, draft: ComposerDraft): void {
  if (!draft.input && draft.quotes.length === 0 && draft.attachments.length === 0) {
    drafts.delete(key)
  } else {
    drafts.set(key, draft)
  }
}
