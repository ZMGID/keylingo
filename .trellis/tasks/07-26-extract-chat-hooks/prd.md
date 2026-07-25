# 抽取 Chat.tsx 的领域 hooks

父任务：`07-26-split-giant-chat-components`
**前置**：`07-26-split-settings-shell-tabs` 先完成（顺序理由见父任务 PRD）

## Goal

把 `src/chat/Chat.tsx`（4298 行）里的领域逻辑抽成 custom hooks，**行为零变化**。

## 现状测绘

和 SettingsShell 相反：**JSX 只占 493 行（3805-4298），逻辑占 3182 行**，220 个 hook 调用。

所以这不是「组件太大」，是「一个组件扛了太多职责」。拆法是抽 custom hook，**不是切子组件**。

耦合集中在 27 个 ref 上，按引用频次：

| ref | 引用次数 | 所属簇 |
|-----|---------|--------|
| `currentConversationIdRef` | 62 | **跨簇共享（最大障碍）** |
| `streamRenderRafRef` | 19 | stream |
| `inFlightConversationsRef` | 18 | 会话加载 |
| `streamSnapshotsRef` | 17 | stream |
| `pendingToolConfirmsRef` | 11 | 工具审批 |
| `pendingStreamRenderRef` | 10 | stream |
| `pendingSessionConsentsRef` | 10 | 外部 CLI |
| `pendingStreamDoneRef` | 9 | stream |
| `streamingReasoningRef` / `streamStartedAtRef` / `streamErrorsRef` / `streamingContentRef` | 7-8 各 | stream |
| `externalSend*Ref` ×3 | 7 各 | 外部 CLI |

### 已确认可抽的簇（按内聚度排序）

**1. 路由簇 — 最干净**
9 个 `sync*Route` 回调（1344-1410）+ `loadFromRoute` effect（2345 起，~76 行）+ 8 个 `isChat*Path` 判定（283-315）。
对外只写 **2 个 setter**（`setChatView`、`setSidebarRefreshKey`），不碰任何 ref。这是最该先做的。

**2. stream 簇 — 体量最大，几乎不用 setter**
8 个 `stream*Ref` / `pendingStream*Ref` + `onChatStream` effect（1783）+ `onChatContext`（1912）。
实测该区间只引用 1 个 setter，**全部通过 ref 通信** —— 这既是它可抽的原因，也是它微妙的原因（时序全靠 ref 读写顺序，抽错就是竞态）。

**3. 外部 CLI 发送队列簇**
`externalSendQueueRef` / `externalSendDrainRequestedRef` / `externalSendDrainProcessingRef` + drain 逻辑。三个 ref 只服务这一件事。

**4. 工具审批簇**
`pendingToolConfirmsRef` + `onChatToolConfirm` effect（2204）。

## Requirements

- 抽成 `src/chat/hooks/use*.ts(x)`，每个 hook 一个文件。
- **`currentConversationIdRef` 不下沉**：62 处引用横跨所有簇，它是 Chat.tsx 的真正中枢。作为参数传给各 hook，而不是让某个 hook 持有它。
- 事件订阅的**时序与处理顺序不得改变**。`chat-stream` / `chat-tool` / `chat-context` 的 payload 形状是 UI 契约（CLAUDE.md 明确）。
- 每抽一个簇跑三道检查（typecheck / lint / test）+ 手动冒烟，单独提交。
- 不合并 effect、不"顺手优化"依赖数组 —— 依赖数组的任何改动都可能改变执行时机。

## Acceptance Criteria

- [ ] `src/chat/hooks/` 下有 4 个新 hook 文件（路由 / stream / 外部发送队列 / 工具审批），每个 ≤ 600 行
- [ ] `Chat.tsx` 降到 2500 行以内
- [ ] `currentConversationIdRef` 仍在 Chat.tsx 中定义（未被下沉）
- [ ] `npm run typecheck` / `npm run lint` / `npm test` 全绿（315 个测试不减少）
- [ ] 手动冒烟（每项都必须做）：
  - 发一轮带工具调用的消息，流式渲染逐字出现、工具卡片状态正常
  - 中途点停止，确认取消生效且不留 ghost 状态
  - 连续切换 3 个会话，无重复加载、无内容错位
  - 触发一次工具审批弹窗，批准 + 拒绝各一次
  - 用外部 CLI runtime（claude/codex 任一）发一条消息
  - 浏览器前进/后退改 hash，确认路由恢复正确
- [ ] 4 个独立 commit，每个单独可回滚

## Constraints

- **这是本父任务里风险最高的部分**。stream 簇全靠 ref 读写顺序保证正确性，没有测试覆盖时序。任何一步冒烟发现异常，立即 revert 该 commit 而不是往前修。
- `chat/agent/loop_tests.rs` 是 Rust 侧的覆盖，**帮不到前端 stream 时序**。前端这块只有手动冒烟。
- 如果某个簇抽出来发现要传超过 8 个参数，说明边界划错了 —— 停下来记录，不硬抽。
- Chat.tsx 里已有 `SettingsEnterPane`（本次会话新增的动画容器）。它是纯展示、无状态，**不属于本任务范围**，别顺手动它。
