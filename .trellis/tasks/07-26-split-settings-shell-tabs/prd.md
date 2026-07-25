# 拆分 SettingsShell.tsx 的 tab 面板

父任务：`07-26-split-giant-chat-components`

## Goal

把 `src/settings/SettingsShell.tsx`（3824 行）里 14 个 tab 的 JSX 抽成独立的展示组件，**状态与逻辑全部留在 shell**。

行为零变化。

## 现状测绘

单个 `return (` 里平铺 14 个 `activeTab === 'x' && (...)` 分支。各分支体量与耦合度（`set*` / `handle*` 引用数）：

| tab | 行数 | 引用 shell 回调数 | 备注 |
|-----|------|------------------|------|
| `providers` | 319 | 5 | 最大，边界清晰 |
| `chat` | 227 | 2 | |
| `general` | 210 | 7 | 耦合偏高 |
| `about` | 176 | 9 | **耦合最高**（含更新下载状态机） |
| `mixer` | 142 | 0 | 零耦合 |
| `lens` | 140 | 0 | 零耦合 |
| `hotkeys` | 111 | 0 | 零耦合 |
| `memory` | 92 | 4 | |
| `translate` | 71 | 2 | |
| `usage` | 33 | 1 | 已委托 `UsageStatsPanel` |
| `connectors` | 25 | 0 | 已委托 `ConnectorsPanel` |
| `webSearch` | 10 | 0 | 已委托，**无需处理** |
| `externalAgents` | 7 | 0 | 已委托，**无需处理** |

`webSearch` / `externalAgents` / `connectors` / `usage` 已经是薄壳（≤33 行，只是转发 props 给既有 panel），**不在抽取范围内** —— 再包一层没有收益。

实际要抽的是 9 个：`providers`、`chat`、`general`、`about`、`mixer`、`lens`、`hotkeys`、`memory`、`translate`。

另有 `if (activeTab === 'memory')` 在 1742 行（早于主 return），是 memory tab 的数据准备逻辑，属于 shell 状态层，**不搬**。

## Requirements

- 9 个 tab 各抽成 `src/settings/tabs/<Name>Tab.tsx` 里的一个纯展示组件。
- shell 保留全部 107 个 hook 调用；状态、副作用、回调定义一行都不下沉。
- 组件通过 props 接收所需的 `settings` 切片、`onUpdate`、`lang`/`t`、以及它用到的 shell 回调（如 `setModelTestProviderId`）。
- props 用显式命名，不允许 `{...allShellState}` 这种整包透传 —— 那只是把耦合藏起来。
- 每抽完一个 tab 就跑三道检查并单独提交，一个 tab 一个 commit。
- 不改 `settingsModals`（渲染在动画容器外，与 `settingsMain` 同级，位置有讲究）。
- 不动 `settings-section-enter` 的 `key={activeTab}` 重播机制。

## Acceptance Criteria

- [ ] `src/settings/tabs/` 下有 9 个新组件文件，每个 ≤ 350 行
- [ ] `SettingsShell.tsx` 降到 2000 行以内
- [ ] shell 里 `useState`/`useCallback`/`useEffect`/`useMemo`/`useRef` 总数不变（状态没被偷偷搬走或合并）
- [ ] `npm run typecheck` / `npm run lint` / `npm test` 全绿
- [ ] 手动冒烟：14 个 tab 逐个点开，每个 tab 内至少改一项设置并保存成功；tab 切换动画正常；未保存改动确认弹窗仍然工作
- [ ] 9 个独立 commit，每个 commit 单独可回滚
- [ ] `git diff` 确认无行为性改动（纯搬迁 + props 传递）

## Constraints

- `about` tab 耦合最高（9 个回调，含应用更新的 `downloadState` 两段式状态机）。放在最后做，如果 props 数量超过 10 个就**停下来记录，不强行拆** —— 留在 shell 里比拆出一个 12 参数的组件更好维护。
- 窄容器响应式规则（`@container settings-shell (max-width: 820px)`）作用在 `.settings-embedded-nav` 上，不受本次改动影响，但冒烟时要缩窗口确认一次。
