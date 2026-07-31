/**
 * 聊天列表「部分虚拟化」——对齐 Paseo web 策略（packages/app/src/agent-stream/web-virtualization.ts）：
 *
 * - 短列表：全部实挂载，无 virtua，无估算高度。
 * - 长列表：只虚拟化**更早**的历史；最近一段始终 DOM 实挂载。
 *
 * 贴底时用户看到的几乎都是实挂载区，高度是真的；virtua 只在上方滚动历史时介入。
 * 这比「全量虚拟化 + 跟随时狂 pin」稳得多——后者会在底部与 remeasure 互抢、抽搐。
 */

/** 超过此条数才启用上方虚拟化（含 spacer 等 render item）。 */
export const VIRTUALIZE_THRESHOLD = 48

/** 底部始终实挂载的最少条数（对齐 Paseo 的 recent window 量级）。 */
export const RECENT_MOUNTED_MIN = 32

/**
 * 边界按此步长量化。不量化的话每来一轮就有 ~2 行从「实挂载」挪进虚拟区，
 * 挪进去的瞬间真实测高被 virtua 估算值顶替 → 正在往上翻历史的用户会看到内容跳。
 * 量化后大约每 8 轮才迁移一次。
 */
export const MIGRATION_STEP = 16

export type HistorySplit<T> = {
  /** true：上方用 Virtualizer，下方实挂载 */
  useVirtual: boolean
  virtualized: T[]
  mounted: T[]
  /** virtualized.length；mounted 在逻辑列表中的起始下标 */
  mountedStartIndex: number
}

/**
 * 冻结期实挂载区的上限（minMounted 的倍数）。用户长时间停在历史里、后台又一直在出新消息时，
 * mounted 会无限长；超过这个量就放弃冻结（一次跳动 < 页面卡死）。
 */
export const FROZEN_MOUNTED_MAX_FACTOR = 3

/**
 * 从末尾向前保留至少 minMounted 条，按 MIGRATION_STEP 向下取整（只会让实挂载区更大，
 * 不会破坏 minMounted 保证），再往前落到实体行（message/group）起点——spacer 不能当锚。
 * 注意：只保证不以 spacer 开头，不保证不把一轮问答劈成两半。
 */
export function findMountedWindowStart(
  items: ReadonlyArray<{ kind: string }>,
  minMounted: number,
): number {
  if (items.length <= minMounted) return 0
  const raw = Math.max(items.length - minMounted, 0)
  let start = Math.floor(raw / MIGRATION_STEP) * MIGRATION_STEP
  while (start > 0) {
    const item = items[start]
    if (item && (item.kind === 'message' || item.kind === 'group')) break
    start -= 1
  }
  return start
}

export function splitHistoryForVirtualization<T extends { kind: string }>(
  items: readonly T[],
  options?: { threshold?: number; minMounted?: number; frozenStart?: number },
): HistorySplit<T> {
  const threshold = options?.threshold ?? VIRTUALIZE_THRESHOLD
  const minMounted = options?.minMounted ?? RECENT_MOUNTED_MIN

  if (items.length <= threshold) {
    return {
      useVirtual: false,
      virtualized: [],
      mounted: items as T[],
      mountedStartIndex: 0,
    }
  }

  // 冻结：用户正在往上翻历史时不许挪边界。挪一行 = 它量准的真实高度被 virtua 的估算值顶替，
  // 上方总高度突变 → 内容在读者眼前跳。冻结期新消息只会让实挂载区变长，不会有行掉进虚拟区。
  const frozen = options?.frozenStart
  const mountedStartIndex = frozen != null
    && frozen > 0
    && frozen < items.length
    && items.length - frozen <= minMounted * FROZEN_MOUNTED_MAX_FACTOR
    ? frozen
    : findMountedWindowStart(items, minMounted)

  // 防御后若虚拟段为空，退化为全挂载
  if (mountedStartIndex <= 0) {
    return {
      useVirtual: false,
      virtualized: [],
      mounted: items as T[],
      mountedStartIndex: 0,
    }
  }

  return {
    useVirtual: true,
    virtualized: items.slice(0, mountedStartIndex) as T[],
    mounted: items.slice(mountedStartIndex) as T[],
    mountedStartIndex,
  }
}
