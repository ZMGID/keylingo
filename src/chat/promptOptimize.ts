/** Composer 问题优化：空草稿和斜杠命令不发起模型调用。 */
export function canOptimizeComposerText(text: string): boolean {
  const trimmed = text.trim()
  return trimmed.length > 0 && !trimmed.startsWith('/')
}
