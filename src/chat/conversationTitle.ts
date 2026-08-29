/** 创建会话时写入的内部占位标题。界面不得展示这个字样。 */
export const PLACEHOLDER_CONVERSATION_TITLE = '新对话'

/** 后端写进标题的分支后缀恒为中文（存量数据也是）。 */
export const FORK_TITLE_SUFFIX = '（分支）'

function titleWithoutForkSuffix(title: string): string {
  return title.endsWith(FORK_TITLE_SUFFIX)
    ? title.slice(0, -FORK_TITLE_SUFFIX.length)
    : title
}

export function isPlaceholderTitle(title: string | null | undefined): boolean {
  const trimmed = title?.trim()
  if (!trimmed) return false
  return titleWithoutForkSuffix(trimmed).trim() === PLACEHOLDER_CONVERSATION_TITLE
}

/** 与 Rust `title_source_for_user_message` 对齐；空内容+无附件返回空串，不回落到「新对话」。 */
export function conversationTitleSource(
  content: string,
  attachmentNames: readonly string[] = [],
): string {
  const trimmed = content.trim()
  if (trimmed) return trimmed
  const names = attachmentNames.filter(Boolean).join(', ')
  return names ? `附件: ${names}` : ''
}

/** 与 Rust `generate_title` 对齐：只 trim 两端，按 Unicode 标量截 30。 */
export function optimisticConversationTitle(
  content: string,
  attachmentNames: readonly string[] = [],
): string {
  const source = conversationTitleSource(content, attachmentNames)
  if (!source) return ''
  const chars = Array.from(source)
  return chars.length > 30 ? `${chars.slice(0, 30).join('')}...` : source
}

/**
 * 侧栏 / 搜索 / 弹出窗标题条用。占位「新对话」（含「（分支）」）换成第一句（或 preview）截断；
 * 真正的模型标题原样返回。
 */
export function displayConversationTitle(
  title: string | null | undefined,
  fallbackText?: string | null,
): string {
  if (!isPlaceholderTitle(title) && title?.trim()) return title
  return optimisticConversationTitle(fallbackText ?? '')
}

/**
 * 标题还是「第一句用户消息」的乐观截断吗（真正的标题尚未生成/落地）？
 * 与 `optimisticConversationTitle` / Rust `generate_title` 口径一致。
 */
export function isProvisionalTitle(title: string, preview: string | null | undefined): boolean {
  if (!preview) return false
  const truncated = optimisticConversationTitle(preview)
  return Boolean(truncated) && title === truncated
}
