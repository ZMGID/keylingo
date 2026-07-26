# Implement — 抽取 Chat.tsx 的领域 hooks

## 前置

- [ ] `07-26-split-settings-shell-tabs` 已完成并提交
- [ ] 工作区干净（本次会话的动画改动已归属）
- [ ] 记录基线：`wc -l src/chat/Chat.tsx`（4298）、`npm test` 通过数（315）

## 每一步的固定动作（4 次）

```bash
npm run typecheck && npm run lint && npm test
```

三道全绿 → 跑该簇对应的冒烟项 → 单独提交。

提交信息：`refactor(chat): 抽出 use<Name>`

**冒烟失败就 `git revert`，不要往前修。**

## 执行清单

### 步骤 1：`useChatRouting`（最干净）

- [ ] 1.1 建 `src/chat/hooks/`；把 8 个 `isChat*Path` 判定（283-315）搬过去
- [ ] 1.2 搬 9 个 `sync*Route` 回调（1344-1410）
- [ ] 1.3 搬 `loadFromRoute` effect（2345 起，~76 行）
- [ ] 1.4 冒烟：改 hash 前进/后退，确认各视图路由恢复；打开设置再关闭，确认路由回到会话

### 步骤 2：`useToolConfirm` — **放弃（边界划错）**

实测 `pendingToolConfirmsRef` 有 12 处引用，其中 7 处与 `streamSnapshotsRef`、
`pendingSessionConsentsRef` **成组出现**（`dropConversationLocally`、
`clearStreamSnapshot`、取消流、finishStreamingRun 等），是同一套「按会话清理」
逻辑的三个分量。

单独抽它会把这 7 个点全变成跨模块调用，且「清理一个会话的所有本地状态」这件事
被拆到两个文件 —— 比留在原处更难维护。按 PRD 的停止条件放弃。

真要拆，单位应该是「会话生命周期」（in-flight + snapshot + toolConfirm +
sessionConsent 一起），那是另一次重构。

### 步骤 3：`useExternalSendQueue`

- [ ] 3.1 搬 `externalSendQueueRef` / `externalSendDrainRequestedRef` / `externalSendDrainProcessingRef` + drain 逻辑
- [ ] 3.2 冒烟：切到外部 CLI runtime（claude 或 codex）发一条消息；连发两条确认队列不乱

### 步骤 4：`useChatStream` — **部分完成：只抽出合帧子簇**

原计划「8 个 stream ref + 两个 effect 整体搬出」不成立。实测 84 处引用横跨
3000 行，且与 `inFlightConversationsRef` / `pendingToolConfirmsRef` /
`pendingSessionConsentsRef` 深度交织（同步骤 2 的发现）。

**已抽出**：合帧子簇 `useStreamRenderFrame`（`streamRenderRafRef` +
`pendingStreamRenderRef`）。这两个 ref 的全部 24 处引用都落在 837-1055 行内，
是真边界；顺带把 5 处重复的「取消挂起帧」内联逻辑收敛成两个出口。

**未抽出**：其余 6 个 stream ref（snapshot / content / reasoning / startedAt /
errors / pendingDone）。它们与会话生命周期状态成组出现，抽出会制造跨模块的
状态撕裂。同样按停止条件放弃。

前提是先把「会话生命周期」整体抽成一个 hook，这超出本任务范围。

### 步骤 5：收尾

- [ ] 5.1 确认 `currentConversationIdRef` 仍在 Chat.tsx 定义
- [ ] 5.2 `wc -l src/chat/Chat.tsx` ≤ 2500；新增文件均 ≤ 600 行
- [ ] 5.3 全量冒烟：PRD Acceptance Criteria 里那 6 项逐个走一遍
- [ ] 5.4 `git diff <base>..HEAD` 通读，确认无行为性改动

## 验证命令

```bash
npm run typecheck
npm run lint
npm test
wc -l src/chat/Chat.tsx src/chat/hooks/*.ts
grep -c "currentConversationIdRef" src/chat/Chat.tsx
```

## 评审关卡

- **步骤 1 后**：hook 参数数量是否合理（≤8）？调用点是否更好读？不好就停。
- **步骤 3 后**：前三步都顺利吗？步骤 4 是风险最高的一步，前面有任何不顺就先停下来复盘。
- **步骤 4.2 前**：确认理解 stream ref 的读写顺序。看不懂就不要动 —— 记录下来，留在 Chat.tsx 也是合理结局。

## 回滚点

每个 commit 一个回滚点。步骤 4 尤其：冒烟异常直接 `git revert`，这块没测试兜底。

## 不做

- 下沉 `currentConversationIdRef`
- 合并 effect / 优化依赖数组 / 重排语句
- 动 `SettingsEnterPane`（本次会话新增的动画容器，无状态，不属本任务）
- 抽子组件（JSX 只 493 行，不是问题所在）
