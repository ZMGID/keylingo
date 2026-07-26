# 拆分 SettingsShell.tsx 与 Chat.tsx 巨型组件

## Goal

把仓库里最大且改动最频繁的两个前端文件拆到可维护的规模，**不改变任何运行时行为**。

这是一次纯结构重构：不修 bug、不加功能、不调样式、不动交互。

## 背景（为什么是这两个文件）

按「行数 × 改动热度」交叉筛选（`git log -200` 的 name-only 统计）：

| 文件 | 行数 | 近 200 提交改动次数 |
|------|------|--------------------|
| `src/settings/SettingsShell.tsx` | 3824 | **19（全仓最高）** |
| `src/chat/Chat.tsx` | 4298 | 15 |
| `src-tauri/src/lib.rs` | — | 17 |

其余大文件（`Lens.tsx` 2930、`api/tauri.ts` 2044、`settings.rs` 3984、`compaction.rs` 2902）改动热度均未进前 15，或大而稳定（serde 结构体、单一算法、测试），拆分收益不足以抵消风险 —— **本任务不碰**。

`lib.rs` 改动热度第二但属于 Tauri 启动装配，追加式增长，同样不在范围内。

## Requirements

### 通用（两个子任务都适用）

- **行为零变化**：拆分前后 UI 渲染结果、交互、动画、事件时序完全一致。
- **不夹带任何其他改动**：不顺手修 bug、不调 lint 风格、不改文案。发现问题另开任务。
- 遵循 CLAUDE.md 的 Code Style：2 空格缩进、单引号、无分号、组件 `PascalCase.tsx`、工具 `camelCase.ts`。
- 每一步都保持 `npm run typecheck`、`npm run lint`、`npm test` 全绿；不允许中间态破损后一次性修。
- 新增文件必须落在既有目录约定下（`src/settings/`、`src/chat/`），不新建顶层目录。

### 子任务划分

| 子任务 | 交付物 |
|--------|--------|
| `07-26-split-settings-shell-tabs` | SettingsShell 的 tab JSX 抽成独立展示组件 |
| `07-26-extract-chat-hooks` | Chat.tsx 的领域逻辑抽成 custom hooks |

**执行顺序**：先 `split-settings-shell-tabs`，再 `extract-chat-hooks`。前者边界干净、风险低，用来验证「纯结构重构 + 三道检查」这套节奏可行；后者难度高一档（跨 hook 的 ref 共享），依赖前者建立的信心和模式。这个顺序写在各子任务的 `implement.md` 里，不靠 Trellis 依赖机制表达。

## Acceptance Criteria

- [ ] 两个子任务各自的 Acceptance Criteria 全部达成
- [ ] `npm run typecheck` / `npm run lint` / `npm test` 全绿（315 个测试不减少）
- [ ] `SettingsShell.tsx` 与 `Chat.tsx` 行数均显著下降，且新增文件无一超过 600 行
- [ ] 手动冒烟：聊天窗口打开/关闭设置、14 个设置 tab 逐个切换、发一轮带工具调用的消息、切换会话 —— 与拆分前无差异
- [ ] `git diff` 审阅确认无行为性改动混入（纯搬迁 + 必要的 props 传递）

## Constraints

- **无 e2e 测试兜底**（CLAUDE.md 明确说明），行为等价只能靠 typecheck + 单测 + 手动冒烟三者叠加保证。因此每个子任务必须**增量提交、单步可回滚**，不允许一个大 commit 搬完。
- Chat.tsx 的 `chat-stream` / `chat-tool` / `chat-context` 事件处理是 UI 契约（CLAUDE.md：payload 形状是契约，不是 provider 细节）。抽 hook 时不得改变这些事件的订阅时序与处理顺序。
- 本次会话已在 `Chat.tsx` / `index.css` 留有未提交的动画改动（`SettingsEnterPane`、侧栏常驻挂载、`chat-motion-view-in` 的 `fill: backwards`）。**开工前必须先确认这些改动的归属**：提交或另开任务，不要和重构混在一个 diff 里。

## Notes

- 用户已确认的范围决策（本任务据此定稿）：
  - SettingsShell 采用**只搬 JSX、状态留在 shell**的方案，不做状态下沉。
  - 两个子任务都做，不是只做一个。
