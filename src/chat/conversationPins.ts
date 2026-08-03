/**
 * 集/项目里对话的显示顺序：**时间序是底座，被拖过的对话钉在指定行**。
 *
 * 规则（顺序即优先级）：
 * 1. 钉子按 row 升序处理，钉在 `min(row, n-1)`；那一行已被占则往下找第一个空位
 *    —— 两条对话钉同一行、或删对话后行号越界都不该丢项；
 * 2. 剩下的对话按传入的时间序（更新时间倒序）填剩余空位；
 * 3. 钉子指向已不存在的对话直接忽略。
 */
export type ConversationPin = { id: string; row: number }

export function applyConversationPins<T extends { id: string }>(
  timeOrdered: T[],
  pins: ConversationPin[],
): T[] {
  if (pins.length === 0) return timeOrdered
  const n = timeOrdered.length
  if (n === 0) return timeOrdered

  const byId = new Map(timeOrdered.map((item) => [item.id, item]))
  const slots: (T | undefined)[] = new Array(n).fill(undefined)
  const pinnedIds = new Set<string>()

  for (const pin of [...pins].sort((a, b) => a.row - b.row)) {
    const item = byId.get(pin.id)
    if (!item || pinnedIds.has(pin.id)) continue
    let row = Math.min(Math.max(pin.row, 0), n - 1)
    while (row < n && slots[row] !== undefined) row += 1
    if (row >= n) continue // 满了就退回时间序，不丢
    slots[row] = item
    pinnedIds.add(pin.id)
  }

  const rest = timeOrdered.filter((item) => !pinnedIds.has(item.id))
  let cursor = 0
  for (let i = 0; i < n; i += 1) {
    if (slots[i] === undefined) slots[i] = rest[cursor++]
  }
  return slots.filter((item): item is T => item !== undefined)
}

/** 把 `id` 钉到 `row`，替换它原有的钉子。 */
export function withPinAt(pins: ConversationPin[], id: string, row: number): ConversationPin[] {
  return [...pins.filter((pin) => pin.id !== id), { id, row }]
}
