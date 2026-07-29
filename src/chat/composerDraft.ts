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

/**
 * 新建会话落库拿到真 id 时，把占位键上的草稿**搬**到真 id 下。
 *
 * 切 plan/orchestrate 模式（以及任何需要先建会话的操作）会让 conversationId 从 undefined
 * 变成真 id，草稿键随之从 NEW_CHAT_KEY 变成该 id。调用方的回填逻辑若把这当成「切到了另一条
 * 会话」，就会把用户刚打的字清掉，且切回原模式也不还原（真 id 不会变回 undefined）。
 *
 * 搬而不是拷：占位键必须腾空，否则下次新建会话又会捡到这条已归属别人的草稿。
 * 目标键已有草稿时不动（那是它自己的，优先），返回 false 让调用方走正常回填。
 */
export function migrateNewChatDraft(fromKey: string, toKey: string): boolean {
  if (fromKey !== NEW_CHAT_KEY || toKey === NEW_CHAT_KEY) return false
  if (drafts.has(toKey)) return false
  const draft = drafts.get(NEW_CHAT_KEY)
  if (!draft) return false
  drafts.set(toKey, draft)
  drafts.delete(NEW_CHAT_KEY)
  return true
}
