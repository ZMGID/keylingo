# Design — 抽取 Chat.tsx 的领域 hooks

## 核心判断

Chat.tsx 的问题不是 JSX 大（只有 493 行），是**一个组件持有 4 类互不相干的领域状态**。所以拆的单位是 hook，不是子组件。

关键约束来自 27 个 ref 的拓扑：绝大多数 ref 只服务单一职责（可以跟着走），但 `currentConversationIdRef` 被引用 62 次、横跨所有簇 —— 它是中枢，**必须留在 Chat.tsx**，以参数形式下发。

## 目标形态

```
src/chat/
  Chat.tsx                      # 中枢 ref + 装配 + JSX（~2500 行）
  hooks/
    useChatRouting.ts           # 路由：9 个 sync*Route + loadFromRoute + isChat*Path
    useChatStream.ts            # 流式：8 个 stream ref + onChatStream + onChatContext
    useExternalSendQueue.ts     # 外部 CLI 发送队列：3 个 externalSend*Ref + drain
    useToolConfirm.ts           # 工具审批：pendingToolConfirmsRef + onChatToolConfirm
```

## 各 hook 的契约

### `useChatRouting`（先做，最干净）

```ts
function useChatRouting(params: {
  onViewChange: (view: ChatView) => void
  onSidebarRefresh: () => void
  currentConversationIdRef: MutableRefObject<string | null>
}): {
  syncConversationRoute: (id: string | null) => void
  syncSettingsRoute: () => void
  // ...其余 7 个
}
```

实测该簇对外只写 2 个 setter、不碰任何 ref（除中枢 ref 只读）。**边界天然成立**。

### `useChatStream`（体量最大，最需小心）

实测 1770-1912 区间只引用 **1 个 setter**，其余全部通过 ref 通信。这说明：

- 好消息：它本来就是自洽的，可以整体搬。
- 坏消息：**正确性完全依赖 ref 的读写顺序**，没有任何测试覆盖这个时序。

所以这个 hook 的抽取原则是**逐字搬迁**：effect 内部代码一行不改，只把 ref 声明和 effect 一起挪进 hook，通过返回值把外部需要的东西暴露出去。不重排语句、不合并 effect、不改依赖数组。

### `useExternalSendQueue` / `useToolConfirm`

各自 3 个 / 1 个专属 ref，职责单一，模式同上。

## 为什么 `currentConversationIdRef` 不下沉

62 处引用分布在：stream 处理（判断 delta 属于哪个会话）、会话加载（防重入）、路由同步、工具审批、外部队列。

它不属于任何一个簇 —— 它是「当前上下文」本身。下沉到任一 hook 都会让其他三个 hook 反向依赖它，制造循环。留在 Chat.tsx 作为参数下发，依赖方向保持单向。

## 逐步顺序

1. `useChatRouting` — 2 个 setter，验证方法
2. `useToolConfirm` — 1 个 ref，小而独立
3. `useExternalSendQueue` — 3 个 ref
4. `useChatStream` — 最后。前三步跑通后再碰最微妙的一块

## 回滚

一个 hook 一个 commit。刻意不做跨 hook 的公共抽象，保证每个 commit 独立可 revert。

冒烟发现异常 → **直接 revert 该 commit**，不要往前修。这块没有测试兜底，"再改一点试试"很容易越描越黑。

## 已知风险

| 风险 | 处理 |
|------|------|
| stream ref 读写顺序被打乱 → 流式渲染错位/竞态 | 逐字搬迁，不重排；冒烟必须发带工具调用的消息并中途停止 |
| effect 依赖数组变化 → 订阅时机改变 | 依赖数组一个字符都不改；变了就是 bug |
| hook 参数超过 8 个 → 边界划错 | 硬性停止条件，记录后跳过该簇 |
| 事件订阅在 hook 里注册顺序与原来不同 | hook 调用顺序 = 原 effect 声明顺序，不重排 |
| 无前端 e2e，Rust 侧 `loop_tests.rs` 覆盖不到 | 只能靠 PRD 里那 6 项手动冒烟，每步都做 |
