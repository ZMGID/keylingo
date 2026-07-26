# Hook Guidelines

> How hooks are used in this project.

---

## Overview

聊天与设置两个大模块都经历过「一个容器组件持有全部领域状态」的阶段。抽 hook 是
从中拆解的主要手段，但**能不能抽取决于状态的耦合形态，不取决于代码行数**。

已抽出的 hook 都在 `src/chat/hooks/`：

| Hook | 职责 | 为什么能抽 |
|------|------|-----------|
| `useTauriEvent` | 订阅一个 Tauri 事件并处理卸载竞态 | 纯样板，13 处重复 |
| `useChatRouting` | hash 路由解析与同步 | 对外只写 2 个 setter |
| `useExternalSendQueue` | 外部发送队列（Lens 交接） | 3 个 ref 只服务这一件事 |
| `useStreamRenderFrame` | 流式渲染合帧（rAF 节流） | 2 个 ref 的全部引用落在 218 行内 |

---

## 抽 hook 前先做共现分析

**红线：不要用「某簇有几个专属 ref」判断内聚度。** 这个判据踩过坑 —— 按它规划的
`useToolConfirm` 和 `useChatStream`（整体）都在实现阶段被迫放弃。

正确判据是**读写共现**：这些 ref 是否总和另一组 ref 在同一批语句里被同时动。

```bash
# 候选 refA 的每处引用，看附近是否也出现 refB
grep -n refA src/chat/Chat.tsx | cut -d: -f1 | while read n; do
  sed -n "$((n-3)),$((n+3))p" src/chat/Chat.tsx | grep -c refB
done
```

共现率高 → 它们是**同一个概念的分量**，拆分单位应该是覆盖它们全体的那个概念。

实例：`streamSnapshotsRef` / `pendingToolConfirmsRef` / `pendingSessionConsentsRef` /
`inFlightConversationsRef` 在 6 处清理块里成组出现。按 stream 或 toolConfirm 去切会把
「清理一个会话」撕到两个文件；按**动作**切（`clearConversationLocalState`）就成立。

---

## 读取侧与写入侧可以分开处理

同一组 ref 的两侧内聚度往往不同。`conversationLocalState.ts` 的做法：

- **写入侧（6 处删除块）**：高度重复、3 处字段组合完全相同 → 收敛成一个带 scope 的函数
- **读取侧（30 处）**：分散且语义各异（判 busy / 取快照 / 恢复预览）→ 留在原处，ref 也不搬

只抽一侧是合法结果，不必追求「整簇搬走」。

---

## 声明顺序循环用 ref 间接层打破

抽出的 hook 常需要一个定义在它**之后**的回调（那个回调又依赖 hook 的返回值）。
不要为此重排声明顺序 —— 用一个 ref 做间接层：

```tsx
const reloadConversationRef = useRef<((id: string) => void) | null>(null)
const { syncConversationRoute } = useChatRouting({
  onLoadConversation: (id) => reloadConversationRef.current?.(id),
})
// ...reloadConversation 定义在此之后（它依赖 syncConversationRoute）
reloadConversationRef.current = (id) => { void reloadConversation(id, { force: true }) }
```

同款用法见 `useStreamRenderFrame` 的 `applyStreamSnapshotToStateRef`。

---

## 聚合视图必须是 getter，不能是快照

把多个 ref 聚成一个对象供下游使用时，用函数每次现取，**不要** `useRef({...})` 存快照：

```tsx
// 错：flushPendingStreamDone 会整体替换 pendingStreamDoneRef.current，
//     快照将永久指向旧对象，清理静默失效
const stateRef = useRef({ pendingStreamDone: pendingStreamDoneRef.current, ... })

// 对
const localState = useCallback(() => ({
  pendingStreamDone: pendingStreamDoneRef.current, ...
}), [])
```

判断依据：`grep -n 'xxxRef.current = ' file` —— 有整体重赋值就必须用 getter。

---

## 稳定身份是契约的一部分

若某个返回值被调用方放进 effect 依赖数组，它的身份稳定性就是契约。
`useExternalSendQueue` 的 `drainExternalSends` 有三个 effect 依赖它，身份一变就重订阅。

保持办法：hook 的参数回调经 `callbacksRef` 读取，`useCallback` 依赖数组留空。

---

## 命名约定

- 文件与导出同名，`use` 前缀，`src/chat/hooks/use*.ts`
- 纯函数（无 hook 调用）不放 `hooks/`，单列 `.ts` 模块（如 `chatRoutes.ts`、
  `conversationLocalState.ts`）—— 也便于直接单测
- **不要从组件文件（`.tsx`）导出常量或普通函数**：会触发
  `react-refresh/only-export-components` 破坏 Fast Refresh。共享常量单列模块
  （`settings/memoryLayers.ts`、`settings/uiFont.ts` 就是为此而生）。类型导出不受限。

---

## Common Mistakes

1. **用 ref 数量当内聚度指标** —— 见上文共现分析。
2. **聚合视图存快照** —— 见上文 getter 一节。
3. **搬迁时重排语句或改依赖数组** —— 流式相关逻辑的正确性依赖 ref 读写顺序，
   且前端无 e2e 覆盖时序。逐字搬迁，依赖数组一个字符都不改；若因抽 hook 必须
   加入新依赖，先确认它是 `useCallback` 稳定身份。
4. **忽略「刻意的例外」** —— `cancelCurrentRunLocally` 看似第 6 处清理，实则
   刻意保留快照以冻结展示（见其注释）。套用统一函数是行为改变。改之前读注释。
