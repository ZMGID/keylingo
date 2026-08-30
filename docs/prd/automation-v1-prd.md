# 自动化 v1 PRD：对齐 n8n 的简化编排

**Date:** 2026-08-30
**Scope:** `src/chat/automation/` + `src-tauri/src/automation/` 的下一阶段规划。
**依据:** `docs/research/n8n-editor-ux.md`（n8n 官方文档调研，下称「调研」）+ 现有实现盘点。

---

## 0. 一句话结论

现状已经把 n8n 的「骨架」搭对了（trigger 起手、画布左进右出、表单式配置、Execute vs enabled 两套语义、run 事件高亮）。缺的不是更多节点，而是 n8n 直觉的真正来源——**「边加边测」的数据闭环**：配置依赖上游的字段时看不到上游数据，跑完看不到每步进出了什么。v1 的主线就是补这个闭环，同时把图的契约在后端锁死。

---

## 1. 现状对照调研的五条 UX 规则

| 调研规则 | 现状 | 缺口 |
| --- | --- | --- |
| 1. 空画布只允许 Trigger 起手 | ✅ 空图默认 trigger picker；已有 trigger 时拦截再加 | 仅前端拦截，后端 `validate_graph` 不管单 trigger / 悬空边 / 环 |
| 2. 配置面板默认表单，不是 JSON | ✅ 全表单 | Agent 节点已拆成四槽（代理 / Context / Tool / Skill）；hotkey 是裸文本框不是录制器 |
| 3. 配依赖上游的字段前必须有样本数据 | ❌ **最大缺口** | 无 INPUT/OUTPUT 双栏；「跑到这步」的输出只在本次会话的事件里，不落完整 json；变量提示只有一行文案，无字段点选 |
| 4. 测试与上线两个按钮、两套语义 | ✅ 语义已对（Manual 无视 enabled；schedule/hotkey 要求 enabled） | 生产 run 只有 12 条摘要列表，无详情回放，无法回答「昨晚定时跑挂在哪」 |
| 5. Canvas 语法锁死（右侧 + 加下一步、IF 双口） | 🟡 `canConnect` 规则齐（单入边、禁连 trigger、IF 每口单出边） | **`+` 只定位不连边**（`addFromCatalog` 只加节点）；`layoutFlow`/`pickAppendSource` 写了带测试但编辑器没用；`skipped` 在 UI 里被当 success |

后端执行模型的硬伤（探索结论，`runner.rs`）：

- 多入边时行为不确定：BFS + `visited` + 单 `prev`，谁先到谁赢，其余入边静默丢弃。
- 扇出是串行 BFS，与 n8n v1「先跑完一条分支」的心智都不符，也没文档化。
- 无整 run 超时（仅 HTTP 30s）；schedule 撞上 running 直接丢 tick 且不可见。
- 历史只存 2000 字文本预览，INPUT 面板没有数据来源。

---

## 2. 边界（契约，写死不做的部分）

这是和 n8n 拉开简化差距的地方，全部对齐调研的 Keep/Drop 结论：

1. **数据模型永远是单条 `NodeOutput { text, json }`**。不做 n8n 的 items 数组、隐式 per-item 循环、item linking（调研排名第一的压人来源）。将来需要批量，加一个显式的 for-each 节点，而不是改数据模型。
2. **图形状锁死为「单 trigger 的树」**：每个 step 单入边（已有），IF 双出口，非 IF 允许扇出（每支拿 `prev` 的克隆）。**不做 Merge / 多输入**——这直接消灭了多入边合并语义问题。Switch、Merge、多 trigger 全部后置。
3. **插值只有 `{{output}}` 和 `{{json.path}}`**，不做 `$json` JS 表达式引擎、不做跨节点引用 `$('Node')`。降低学习成本的方式是「从 INPUT 样本点选生成」，不是扩语法。
4. **无独立 credential 系统**：agent/http 复用 Kivio 的 providers 与 settings，这是桌面 AI 助手对 n8n「App 目录 × Credentials」的天然替代。
5. **无 Webhook 触发器（v1）**：桌面场景下 hotkey 就是「外部事件入口」。要 webhook 等于要常驻 HTTP 服务，是独立课题。
6. **无草稿/发布双版本**：一份 JSON + `enabled`，语义已够（Manual 恒可跑 = n8n 的 manual execution；enabled = published）。不做 named versions / review。
7. **Agent 节点 = 无人值守**：工具自动批准、`ask_user` 即取消、对话不进 sidebar、工作区隔离在 `{workingDirectory}/automations/{id}`。这是明确契约，不是缺陷；风险控制靠 P2 的 run 详情可回放。
8. **事件通道维持独立的 `automation-run` Tauri event**，不并入 chat-protocol（不同生命周期，不值得背 sequencing/replay 的复杂度）；编辑器中途打开的补偿靠查询而非事件回放（见 P2）。

---

## 3. 分期计划

### P0 — 锁契约（后端为主，小改动）

- `storage.rs::validate_graph` 补全：边端点必须存在、恰好 ≤1 个 trigger、step 单入边、无环、IF 出口 handle 合法。返回结构化错误（错误码 + 节点/边 id），`automation_save` 拒绝非法图，前端在画布上标红对应元素。
- `runner.rs` 执行序改为**分支完成式 DFS**（对齐 n8n v1：先跑完一条分支，同源多出边按「上→下」= handle/位置序），并在 `mod.rs` 头注释文档化。多入边由 P0 校验杜绝，`visited` 退化为断言。
- 整 run 超时（默认 10 分钟，常量），超时按 error 收尾并发 `run_finished`。
- 前端 `node_finished` 的 `skipped`/`cancelled` 状态单独映射（不再显示成 success）。

### P1 — 补齐 n8n 核心循环「边加边测」（前端为主，v1 主菜）

**P1a 画布对齐 n8n（2026-08-30 已实现）：**

- 节点视觉改为 n8n 式：图标方卡（104×80）+ 名称/配置摘要悬挂卡片下方；触发器为左圆角「D」形；输出口圆点、输入口小圆角矩形；连线带箭头；右上角运行状态角标（running 转圈 / success 勾 / error 叉 / skipped 横线）。
- 节点 hover 工具条：跑到这步 / 停用 / 删除（对齐 n8n 节点 hover 操作）。
- 连线 hover 中点浮出「插入节点」（拆边接中间）与「删除连线」。
- **`+` 自动连边**；底部胶囊「添加节点」自动接到还能出边的尾节点（`pickAppendSource` 接入编辑器）；从输出口拖线放到空白处弹出节点选择器并在落点接上（n8n 签名交互）。
- Inspector 增加节点重命名（名称字段）。
- **JSON 导入/导出**：编辑器头部导出（save 对话框 → `automation_export`）；列表页导入（open 对话框 → `automation_import`，后端强制新 id + `enabled=false`，避免 id 撞车和「导入即注册」热键/定时）。这也是自动化的分享/备份通道。
- **NodeInspector 加 INPUT / OUTPUT 双栏**（n8n NDV 的灵魂）：
  - INPUT = 上游节点最近一次输出（来自 run 历史的节点输出），OUTPUT = 本节点最近输出；无数据时显示「先执行到上一步」引导。
  - 依赖 P2 的完整 json 落盘（两项同期做）。
- **字段点选插入**：INPUT 栏的 json 字段可点击，向当前聚焦的输入框插入 `{{json.path}}`。这是调研规则 3 的「拖放映射」简化版——用户不需要先学语法。
- **表单补漏**：hotkey 触发器复用 settings 的 `HotkeyRecorder`（含冲突检测，别再让用户手打 `Control+Shift+A`）。Agent 四槽是真正的接入节点（代理 / Context / Tool / Skill），虚线接到 Agent 底边菱形；主流程仍是单 trigger 树，槽边不计入入边。
- 画布校验徽标：缺 trigger / 未连接节点显示警示；Execute 禁用时给出原因 tooltip。

### P2 — 运行可观测性

- **节点输出完整落盘**：`AutomationRunNode` 增加 bounded 的 `output_json`（单节点上限 64KB，超限截断并打标），文本预览维持 2000 字。这同时是 P1 INPUT 栏的数据源。
- **Run 详情视图**：`automation_run_get` 命令已有，补 UI——run 列表项可点开，逐节点 status / 耗时 / output / error；点节点联动画布高亮（回放）。
- **中途接入**：编辑器挂载时查询进行中的 run（`automation_active_runs` 暴露一个查询命令），恢复节点高亮状态，而不是空白。
- schedule 撞 running 丢 tick 时写一条 `skipped` 摘要进历史（可见性，不改行为）。

### P3 — 触发器与生产可靠性

- daily 错过 2 分钟窗口的策略显式化：默认不补跑，但在历史里记 `missed`；设置项「错过后启动时补跑一次」后置观察需求。
- interval 首次 Arm 不跑的语义在 UI 文案写明（或改为立即跑一次，二选一定死）。
- 热键注册冲突（与内置热键或其它自动化撞键）目前静默跳过——在自动化列表/编辑器上显示「热键未生效」徽标。

### P4 — 节点扩展（按需，每个都要过「边界」审查）

- `action.set`（Edit Fields 表单版）：把上游 text/json 整形为固定 json，是调研最小集成员，也让 agent 输出可被 IF/HTTP 稳定消费。
- `logic.delay`（等待 N 分钟）。
- Switch / Merge / Webhook / for-each：维持后置，出现真实用户场景再评估。

---

## 4. 验收标准（对齐调研第 7 节的可验收规则）

1. 新建自动化 → 只能从 trigger 起手 → 每次 `+` 都自动连边，全程无需手动拖线即可搭出「schedule → agent → if → notify」。
2. 在 notify 节点配 `{{json.x}}` 前，用户能在 INPUT 栏看到 agent 的实际输出并点选字段；没跑过上游时有明确引导。
3. 「跑到这步」后 OUTPUT 栏立即可见本节点输出；重启应用后再打开，INPUT/OUTPUT 仍在（落盘）。
4. 定时触发失败的 run，能从历史点开看到挂在哪个节点、错误是什么。
5. 后端拒绝一切非法图（悬空边、双 trigger、环、多入边），前端能定位标红。
6. 文档层面：本文件第 2 节的 8 条边界写进 `automation/mod.rs` 头注释的精简版。

---

## 5. Agent 操作面（工具 + 运行回传）

Chat 内置 agent 通过宿主无关层 `automation/tools.rs` 操作自动化（7 个 native 工具：`list` / `get` / `upsert` / `set_enabled` / `run` / `runs` / `delete`）。`upsert` 强制走 `validate.rs`（error 拒绝、warning 放行）；画布手动保存仍是所见即所得。`automation_run` 阻塞等待，可选 `input` 合入 trigger 输出（`{{output}}` / `{{json.input.*}}`），超时返回 `run_id` 而不当失败；用户停止生成时 `runner::cancel` 级联。进度卡片复用 `structured_content.type = "automation_run"` + 现有 `automation-run` 事件，不加 chat-protocol 事件、不加新 segment。工作流内 `action.agent` 默认剥离全部 `automation_*`，防递归。

### 明确不做（本阶段）

- 给外部 CLI 暴露的 MCP server：只预留 `tools.rs` 这一层，不写 server 代码。
- n8n 式 partial diff 更新。
- webhook 触发器。
- 跨工作流调用节点。
- `HookDef` 扩展 `automation_run_start/end`（P2）。
