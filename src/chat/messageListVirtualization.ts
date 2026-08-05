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

/**
 * 条数不够但**内容太重**时也要虚拟化。
 *
 * 条数是个坏预算：实测一个只有 14 条消息的对话，因为 7 条回答里塞了 231 个代码块，
 * 渲染出 5433 个 DOM 节点、切换要等一秒；而另一个字数相当（4.8 万 vs 5.3 万）但只有
 * 2 个代码块的对话只有 484 个节点，秒开。**成本由带外壳的块的个数驱动，不是字数。**
 * 所以条数门槛之外再加一条成本门槛，两者取或。
 */
export const VIRTUALIZE_COST_THRESHOLD = 1500

/** 底部始终实挂载的最少条数（对齐 Paseo 的 recent window 量级）。 */
export const RECENT_MOUNTED_MIN = 32

/** 重会话下实挂载尾部的成本预算，以及无论如何都要留的条数。 */
export const MOUNTED_COST_BUDGET = 800
export const MOUNTED_MIN_ITEMS = 3

/**
 * 边界按此步长量化。不量化的话每来一轮就有 ~2 行从「实挂载」挪进虚拟区，
 * 挪进去的瞬间真实测高被 virtua 估算值顶替 → 正在往上翻历史的用户会看到内容跳。
 * 量化后大约每 8 轮才迁移一次。
 */
export const MIGRATION_STEP = 16

/**
 * 重会话不量化边界（步长 1）。默认的 16 对重会话是**致命**的：重会话的条数往往很少
 * （14 条消息 ≈ 15 个 render item），按 16 量化后任何小于 16 的实挂载窗口都会被压回 0，
 * 边界回到 0 就等于没虚拟化，成本预算白算。
 *
 * 为什么直接取 1 而不是折中的 2 或 4：量化只能让实挂载条数落在步长的格子上，而重会话里
 * 一条回答动辄上千成本，多带进来一条就前功尽弃。实测一个 15 项的会话，预算算出 4 条，
 * 步长 4 撑成 7 条（降幅 1.4x）、步长 2 撑成 5 条（2.5x）、步长 1 精确 4 条（6.8x）——
 * 多出来的那条正是个 1369 成本的重回答。
 *
 * 不量化的代价（每来一轮边界都挪一格 → 真实测高被 virtua 估算顶替 → 读者眼前内容跳）
 * 已经由 `frozenStart` 兜住了：**用户一旦脱离跟随（= 正在翻历史）边界就冻结**，而跟随态下
 * 用户本来就钉在底部，上方高度变化看不见，钉底还会再纠正一次。量化在重会话里保护的是
 * 一个已经被冻结覆盖的场景，却要付「多挂一整条重回答」的代价。
 */
export const HEAVY_MIGRATION_STEP = 1

/**
 * 估算一段 markdown 渲染出来大概有多少个 DOM 节点。纯字符串扫描，不解析 markdown。
 *
 * 两个系数是拿真实会话反推的，不是拍的：
 *   大对话 231 个围栏 / 52673 字符 → 估 4971，实测 5433 节点
 *   小对话   2 个围栏 / 47885 字符 → 估  359，实测  484 节点
 * 一个代码块的固定外壳 + token span 约 20 个节点，所以围栏权重 20；其余散文按字符摊。
 * 表格单独算只占 13%，并进字符项，少一个要调的旋钮。
 */
const COST_PER_FENCE = 20
const COST_CHARS_PER_UNIT = 150

export function estimateRenderCost(text: string): number {
  if (!text) return 0
  // 行首围栏才算（正文里内联的 ``` 不算）；一对围栏 = 一个代码块。
  const fences = (text.match(/^\s{0,3}```/gm)?.length ?? 0) / 2
  return Math.round(fences * COST_PER_FENCE + text.length / COST_CHARS_PER_UNIT)
}

/**
 * 估算一段 markdown 渲染出来**大概多高**（px）。和 estimateRenderCost 是两套系数：
 * 前者估节点数（决定渲染贵不贵），这里估像素（决定滚动条准不准），两者不成比例
 * —— 一个代码块外壳很贵但不高，一段长散文很便宜但很高。
 *
 * 用途只有一个：喂给 virtua 的 `itemSize`。**不给它的话 virtua 会拿已测量的行去外推屏外的行**，
 * 而我们把实挂载尾部砍到 3~4 条之后，它只能拿这几条去推上面十几条；行高差 30 倍，
 * 推出来必然错，错了就是「往上翻历史内容跳」「拖滚动条跳」。
 *
 * 这**不是**第二个高度估算器 —— itemSize 是 virtua 自己的输入，我们只是别让它瞎猜。
 */
const HEIGHT_PER_FENCE_PX = 96
const HEIGHT_PER_LINE_PX = 24
const CHARS_PER_LINE = 56

export function estimateRenderHeight(text: string): number {
  if (!text) return 0
  const fences = (text.match(/^\s{0,3}```/gm)?.length ?? 0) / 2
  const prose = Math.max(0, text.length - fences * 2 * 8)
  return Math.round(
    fences * HEIGHT_PER_FENCE_PX + (prose / CHARS_PER_LINE) * HEIGHT_PER_LINE_PX,
  )
}

/** virtua 的 itemSize 必须是正数；行全是空的时候给个保守下限。 */
export const MIN_ITEM_SIZE_PX = 40

/**
 * 重会话走「向上渐进加载」而不是虚拟化。
 *
 * 为什么不能虚拟化：实测这个应用里的行高差**三个数量级** —— 用户提问 6~21px，
 * assistant 回答 6885~11992px（一条回答就是 5~12 个屏幕）。virtua 只接受一个标量
 * `itemSize`，均值对两边都是错的（最坏单行偏差 9363px），不给它则它拿实挂载尾部去外推、
 * 错得更多。估算错多少，用户往上翻或拖滚动条时就跳多少。按「消息」这个粒度做虚拟化，
 * 在这种数据上**结构性地做不到不跳**。
 *
 * 渐进加载没有「估算高度」这回事：屏幕上永远只有真实测量过的内容，上方内容是长出来的，
 * 长出来多少就把 scrollTop 补偿多少，所以结构上不会跳。代价是滚动条长度只代表已加载部分
 * （往上滚时变长）—— Telegram / Slack 就是这个行为。
 */
export const LOAD_EARLIER_TRIGGER_PX = 480
export const EARLIER_BATCH_COST_BUDGET = 800

/**
 * 从 `from - 1` 往前累加成本，返回这一批揭示后的新起始下标。
 * 至少揭示一条（否则滚到顶就卡住），之后只要不超预算就继续往前吃。
 */
export function earlierBatchStart(
  costs: readonly number[],
  from: number,
  budget: number = EARLIER_BATCH_COST_BUDGET,
): number {
  if (from <= 0) return 0
  let start = from - 1
  let total = costs[start] ?? 0
  while (start > 0) {
    const next = costs[start - 1] ?? 0
    if (total + next > budget) break
    total += next
    start -= 1
  }
  return start
}

/**
 * 从末尾往前累加成本，返回实挂载尾部应该留几条。
 *
 * **停在超预算之前，不是超了才停**：一条重回答动辄上千成本，「加到超预算」会让最后进来的
 * 那一条把窗口整个撑爆（实测一个会话因此从 773 变成 2142）。第一条无论多贵都得要，
 * 否则实挂载区为空。
 *
 * 至少留 MOUNTED_MIN_ITEMS 条 —— 视口要填满，virtua 也需要有真实高度可量。
 * 内容够轻时返回全部条数，等于不虚拟化。
 */
export function mountedCountForBudget(
  costs: readonly number[],
  budget: number = MOUNTED_COST_BUDGET,
): number {
  let total = 0
  let count = 0
  for (let i = costs.length - 1; i >= 0; i -= 1) {
    const cost = costs[i] ?? 0
    if (count > 0 && total + cost > budget) break
    total += cost
    count += 1
  }
  return Math.max(MOUNTED_MIN_ITEMS, count)
}

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
  migrationStep: number = MIGRATION_STEP,
): number {
  if (items.length <= minMounted) return 0
  const raw = Math.max(items.length - minMounted, 0)
  const step = Math.max(1, migrationStep)
  let start = Math.floor(raw / step) * step
  while (start > 0) {
    const item = items[start]
    if (item && (item.kind === 'message' || item.kind === 'group')) break
    start -= 1
  }
  return start
}

export function splitHistoryForVirtualization<T extends { kind: string }>(
  items: readonly T[],
  options?: { threshold?: number; minMounted?: number; frozenStart?: number; migrationStep?: number },
): HistorySplit<T> {
  const threshold = options?.threshold ?? VIRTUALIZE_THRESHOLD
  const minMounted = options?.minMounted ?? RECENT_MOUNTED_MIN
  const migrationStep = options?.migrationStep ?? MIGRATION_STEP

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
    : findMountedWindowStart(items, minMounted, migrationStep)

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

/**
 * 发送后尾部预留的高度。基准是**滚动视口**的实测高度，不是窗口高 —— ask_user 面板吊在输入框
 * 上方、在滚动区之外，它一出现视口就矮一大截，按窗口算的预留会比视口还高、把刚发出的那条消息
 * 整个顶出屏幕。所以再夹一道「视口 - 锚点行高 - 上下留白」：比例给多大，那条消息都得留在屏幕里。
 */
export const SEND_RESERVE_RATIO = 0.45

export function sendReserveHeight(viewportH: number, anchorH: number, edgePadding: number): number {
  return Math.max(
    0,
    Math.min(viewportH * SEND_RESERVE_RATIO, viewportH - anchorH - edgePadding * 2),
  )
}
