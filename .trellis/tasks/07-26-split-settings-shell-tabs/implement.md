# Implement — 拆分 SettingsShell.tsx 的 tab 面板

## 前置

- [ ] 先处理工作区未提交的动画改动（`src/chat/Chat.tsx` + `src/index.css`）：提交或 stash。**不要**和本次重构混进一个 diff。
- [ ] 记录基线：`wc -l src/settings/SettingsShell.tsx`（3824）、shell 内 hook 总数（107）、`npm test` 通过数（315）。

## 每一步的固定动作（9 次，每个 tab 一遍）

```bash
npm run typecheck && npm run lint && npm test
```

三道全绿 → 手动点开该 tab，改一项设置 + 保存 → 单独提交。任一道失败就先修，不许攒。

提交信息：`refactor(settings): 抽出 <Name>Tab`

## 执行清单

### 阶段 A：零耦合 tab（验证流程）

- [ ] A1 建 `src/settings/tabs/` 目录；抽 `HotkeysTab`（111 行，0 回调）
- [ ] A2 抽 `LensTab`（140 行，0 回调）
- [ ] A3 抽 `MixerTab`（142 行，0 回调）

**A 阶段结束后回看**：props 命名约定是否顺手？shell 侧调用点是否比原来更好读？如果答案是"没变好"，停下来重新考虑方案，不要惯性往下做。

### 阶段 B：低耦合 tab

- [ ] B1 抽 `TranslateTab`（71 行，2 回调）
- [ ] B2 抽 `MemoryTab`（92 行，4 回调）—— 注意 1742 行的 `if (activeTab === 'memory')` 数据准备逻辑**留在 shell**

### 阶段 C：大块 tab

- [ ] C1 抽 `ChatTab`（227 行，2 回调）
- [ ] C2 抽 `ProvidersTab`（319 行，5 回调）—— 最大的一块，收益最明显

### 阶段 D：高耦合 tab

- [ ] D1 抽 `GeneralTab`（210 行，7 回调）
- [ ] D2 抽 `AboutTab`（176 行，9 回调）
      **退出条件**：props 数量 > 10 就放弃，在本文件记录原因并跳过。留在 shell 比拆出 12 参数组件更好维护。

### 阶段 E：收尾

- [ ] E1 复核 shell 内 hook 总数仍为 107（`grep -c "useState\|useCallback\|useEffect\|useMemo\|useRef"`）
- [ ] E2 复核 `wc -l SettingsShell.tsx` ≤ 2000
- [ ] E3 复核新增文件均 ≤ 350 行
- [ ] E4 完整冒烟：14 个 tab 逐个点开（含未抽取的 4 个薄壳）；缩窄窗口确认 820px 以下的图标轨布局正常；触发一次未保存改动确认弹窗
- [ ] E5 通读 `git log -9 --stat` + `git diff <base>..HEAD`，确认无行为性改动混入

## 验证命令

```bash
npm run typecheck
npm run lint
npm test
wc -l src/settings/SettingsShell.tsx src/settings/tabs/*.tsx
grep -c "useState\|useCallback\|useEffect\|useMemo\|useRef" src/settings/SettingsShell.tsx
```

## 评审关卡

- **A 阶段后**：方案是否真的让代码更好读？不好就停。
- **C2 后**（最大一块搬完）：此时收益已过半，确认继续做 D 是否值得。
- **D2 前**：`about` 的 props 数量点算，超 10 就跳过。

## 回滚点

每个 commit 都是回滚点：`git revert <sha>`。刻意不做跨 tab 公共抽象，保证 commit 相互独立。

## 不做

- 状态下沉（父任务 PRD 已定：只搬 JSX）
- `webSearch` / `externalAgents` / `connectors` / `usage` 这 4 个薄壳 tab
- 任何 CSS 改动
- 顺手修 bug —— 发现了另开任务
