# Sub-Agent 通用化：自定义角色 + 动态发现 + MCP 工具

## 背景

Kivio 的子代理（`agents/` + `chat/sub_agent.rs`）目前是「3+1 个固定角色」的形态：

- `builtin_agent_definitions()` 硬编码 general-purpose / researcher / coder / reviewer。
- `agent` 工具的 `subagent_type` 参数描述里把这四个名字写死成字符串字面量。
- `.md` 角色层（`<app_data>/agents/*.md`、`<project>/.kivio/agents/*.md`）已经实现并能加载，但**主 Agent 无从得知它们存在** —— schema 和 system prompt 都不列出实际可用角色，只有猜对名字才能命中。
- 子代理拿到父级**全部** enabled 工具，只能用扁平工具名白名单收窄；无法按 MCP server 维度授权（对比 `ChatAssistant` 已有 `mcp_server_ids` / `skill_ids`）。
- 子代理的 system prompt 用 `SkillRegistry::default()`（空注册表）构建，技能目录为空；但 `filter.rs` 又刻意保留 skill 工具 —— 工具在、目录不在，子代理无法发现技能。
- 主 Agent 无法在一次会话里临时定义一个新角色，只能用现成的四个。

## 业界规范（调研结论）

子代理定义**已有事实标准**（无正式标准机构，但各家已收敛且互认目录）：`.md` + YAML frontmatter，markdown body 即 system prompt。

| 工具 | 项目级 | 用户级 |
|---|---|---|
| Claude Code | `.claude/agents/` | `~/.claude/agents/` |
| Cursor | `.cursor/agents/`，**并读 `.claude/`、`.codex/`** | 同 |
| Gemini CLI | `.gemini/agents/` | `~/.gemini/agents/` |
| OpenHands | `.agents/agents/` | `~/.agents/agents/` |
| OpenCode | `.opencode/agents/` | `~/.config/opencode/agents/` |

Cursor 显式声明兼容 `.claude/` 与 `.codex/` 并定义跨厂商优先级 —— 该格式已被当作可互操作标准。

Claude Code 的完整字段集（`--agents` JSON 与 frontmatter 同构）：
`description` `prompt` `tools` `disallowedTools` `model` `permissionMode` `mcpServers` `hooks` `maxTurns` `skills` `initialPrompt` `memory` `effort` `background` `isolation` `color`

规范中对本任务有约束力的三条：

1. **MCP 工具通配语法是 `mcp__<server>` / `mcp__<server>__*`**（`disallowedTools` 中 `mcp__*` 移除全部 MCP 工具）。Kivio 的 `tool_definition_from_mcp`（`mcp/types.rs:177`）已经是 `format!("mcp__{}__{}", server.id, tool.name)` —— **id 格式天生符合规范**，不得自创 `server:*` 方言。
2. **`tools` 与 `disallowedTools` 的组合顺序有明确定义**：先应用 `disallowedTools`，再在剩余池上解析 `tools`；两处都列出的工具被移除。无排序歧义。
3. **`tools` 解析为空集时应拒绝启动并报错**，列出未命中的条目，而不是静默启动一个零工具子代理。

`skills` 的规范语义是 **preload —— 启动时注入技能全文到 context**，且不阻止子代理通过 Skill 工具调用未列出的技能（不是「收窄可见目录」）。

明确不采纳的规范字段：`isolation: worktree`（Kivio 无 worktree 概念）、`permissionMode`（子代理审批恒为拒绝，三态无意义）、`background`（Kivio 已明确删除 dispatch-and-poll 设计，见 `sub_agent.rs:11-17`）、`hooks` / `mcpServers` 内联定义 / `memory` / `maxTurns` / `initialPrompt`（本次范围外）。

目录范围：**只读 Kivio 自己的两层**（`<app_data>/agents/`、`<project>/.kivio/agents/`），不做跨厂商目录读取。字段与规范对齐即可让用户手动拷贝复用生态角色文件。

## 目标

让子代理从「四个写死的角色」变成一个通用的、可被主 Agent 自主发现、组合与扩展的委派机制。

## 需求

### R1 动态角色发现（必须）

主 Agent 必须在每一轮都能看到当前会话实际可用的全部角色（内置 + 用户级 + 项目级），含名称与描述。

- `agent` 工具的 `subagent_type` 描述不再是硬编码字符串，改为运行时按已加载的 `AgentDefinition` 列表生成。
- 角色清单随会话的 project root 变化（项目级 `.kivio/agents/` 覆盖同名用户级角色）。

### R2 主 Agent 可即时自定义角色（必须）

不需要预先建文件，主 Agent 就能在一次 `agent` 调用里定义一个临时角色：

- `agent` 工具新增可选参数，允许直接传入该次运行的 system prompt 与工具白名单。
- 临时定义与 `subagent_type` 可组合：以命名角色为基底，用内联字段覆盖。
- 持久化复用走已有文件层（`<app_data>/agents/*.md`），不新增写入工具 —— 主 Agent 用现成的 `write` 工具即可，前提是 prompt 中告知目录与格式。

### R3 MCP 维度授权 + 继承减法（必须）

角色定义能按 MCP server 授权，且能表达「继承全部、只挖掉几个」。

- 工具白名单条目支持规范通配语法：`mcp__<server>`、`mcp__<server>__*`、`mcp__*`、`*`（沿用 Kivio 既有 id 格式，不自创方言）。
- 新增 `disallowedTools` 字段（denylist）。组合顺序按规范：**先 deny，再在剩余池上解析 allow**；两处都列的工具被移除。
- 空 `tools` 继续表示「不收窄」（沿用现语义，不破坏既有 `.md`）。
- `tools` 非空但解析结果为空集时**拒绝启动**并返回错误，列出未命中的条目（修掉现有的静默残废启动）。

### R4 子代理技能（必须）

修复子代理 system prompt 使用空 `SkillRegistry` 的问题：构建 prompt 时传入真实技能注册表。新增 `skills` 字段，语义按规范为 **preload**（启动时注入所列技能全文），不阻止子代理通过 skill 工具调用未列出的技能。

### R5 兼容性（必须）

- 现有四个内置角色 id / 名称 / 行为不变，仅降级为「预置模板」。
- 现有用户 / 项目 `.md` 角色文件无需修改即可继续工作。
- `filter_tools_for_agent` 始终剥离 `agent` 工具的递归保护不得削弱。
- 子代理无审批升级、级联取消、并发信号量等安全栏杆不得削弱。

## 非目标

- 不做角色管理前端页面（扩展区新增「Agents」中心）。模型侧可用即达成本次目标；UI 待用户明确要求再加。
- 不做跨厂商目录读取（`.claude/agents/`、`.cursor/agents/`）。字段对齐规范已足够让用户手动拷贝复用。
- 不采纳 `isolation` / `permissionMode` / `background` / `hooks` / `mcpServers` 内联 / `memory` / `maxTurns` / `initialPrompt`（理由见上文「业界规范」末段）。
- 不做子代理嵌套（`agent` 工具仍从子代理工具表中剥离）。
- 不做角色定义的运行时热重载缓存优化（每次 spawn 现读目录，成本可忽略）。

## 验收标准

- [ ] 在 `<app_data>/agents/` 放一个 `my-analyst.md`，主 Agent 无需被告知名字，就能在工具 schema 中看到 `my-analyst` 及其描述并正确调用。
- [ ] `agent` 调用只传 `prompt` + 内联 `system_prompt` + 内联 `tools`（不传 `subagent_type`），子代理按内联定义运行，工具表被正确收窄。
- [ ] 角色 `.md` 中 `tools: mcp__notion__*` 放行 Notion server 的全部工具并挡掉其它 server 的工具；`mcp__*` 覆盖全部 MCP 工具。
- [ ] 角色 `.md` 中 `disallowedTools: write, edit` 使子代理继承全部工具但没有这两个；deny 与 allow 同时出现时按「先 deny 后 allow」解析。
- [ ] `tools` 全部拼错时 spawn 返回错误并列出未命中条目，而不是启动一个零工具子代理。
- [ ] 角色 `.md` 中 `skills: pdf` 使子代理启动时 context 内含 pdf 技能全文；不写 `skills` 时子代理仍能通过 skill 工具发现父级全部技能（当前是空注册表，属 bug 修复）。
- [ ] 现有 `researcher` / `coder` / `reviewer` 行为与改动前一致（既有单测全绿）。
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml` 与 `npm run lint && npm run typecheck` 通过。
