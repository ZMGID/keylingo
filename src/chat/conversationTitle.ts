/**
 * 标题还是「第一句用户消息」的乐观截断吗（真正的标题尚未生成/落地）？
 * 首轮发送后、模型标题替换前，列表项 title = 截 30 字符的第一句，preview = 截 100 字符的同一句
 * （与 Chat.tsx 的 optimisticConversationTitle 口径一致）。生成期间据此把临时标题置灰。
 */
export function isProvisionalTitle(title: string, preview: string | null | undefined): boolean {
  if (!preview) return false
  const compact = preview.replace(/\s+/g, ' ').trim()
  if (!compact) return false
  const truncated = compact.length > 30 ? `${compact.slice(0, 30)}...` : compact
  return title === truncated
}
