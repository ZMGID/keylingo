/** Estimate tokens: ASCII is roughly 4 chars/token; CJK and other non-ASCII chars count as 1. */
export function estimateTokens(text: string): number {
  let ascii = 0
  for (let i = 0; i < text.length; i++) {
    if (text.charCodeAt(i) < 128) ascii++
  }
  const nonAscii = text.length - ascii
  return Math.ceil(ascii / 4 + nonAscii)
}

export function formatTokens(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}k`
  return `${n}`
}

/**
 * 同 `formatTokens`，但用**大写 K** —— Chat 界面的既有写法（上下文用量条、窗口大小、
 * 消息用量都是 K）。此前是各调用点自己 `.replace('k', 'K')`，同一个惯例抄在两处。
 */
export function formatTokensK(n: number): string {
  return formatTokens(n).replace('k', 'K')
}
