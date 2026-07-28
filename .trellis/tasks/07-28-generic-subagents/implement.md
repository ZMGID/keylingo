# 执行计划：Sub-Agent 通用化

字段名与语义照 prd.md「业界规范」章节，不要自创。顺序按依赖排。

## Step 1 — `AgentDefinition` 加 `disallowed_tools` / `skills`

- [ ] `src-tauri/src/agents/types.rs`：新增 `#[serde(default)] pub disallowed_tools: Vec<String>` 和 `#[serde(default)] pub skills: Vec<String>`；四个内置角色补空值（**不改它们现有的 `tools` 白名单** —— 见 design 六「内置角色改用 deny 表达」）。
- [ ] `src-tauri/src/agents/parse.rs`：两个字段走已有 `parse_list_value`。`disallowedTools`（规范 camelCase）与 `disallowed_tools` 两个键都查，前者优先。
- [ ] 新增单测：解析 `skills: pdf, docx`；两种键名的 `disallowedTools` 都能解析；缺省为空。
- [ ] `filter.rs` 测试里的 `def()` helper 补字段。

验证：`cargo test --manifest-path src-tauri/Cargo.toml agents::`

## Step 2 — 规范通配匹配 + 先 deny 后 allow + 零工具检测

- [ ] `filter.rs` 新增 `fn entry_matches(tool, entry) -> bool`，按 design 四实现：`*` 全放行；`mcp__*` 匹配 `source == "mcp"`；`mcp__<server>` / `mcp__<server>__*` 匹配 `server_id`；其它后缀 `*` 前缀匹配 `[name, id, openai_tool_name()]`；无通配委托现有 `tool_matches_recommended_name`（保留 legacy 别名）。
- [ ] `filter_tools_for_agent` 改为两阶段：先按 `def.disallowed_tools` 剔除，再（`tools` 非空时）按 `def.tools` 取交集。`agent` 工具在两阶段之外无条件剥离。
- [ ] 返回值增加「未命中条目」信息，供上层做零工具检测。签名可改为返回 `(removed, unresolved_entries)` 或新增一个 `fn unresolved_entries(pool, allow) -> Vec<String>`；选后者，`filter_tools_for_agent` 签名不动，减少调用点扰动。
- [ ] 测试 helper 补 `mcp(server, name)`（现有只有 `native()`）。
- [ ] 新增单测：`mcp__notion__*` 与 `mcp__notion` 等效且挡掉其它 server；`mcp__*` 放行全部 MCP 但挡掉 native；`*` 放行全部；无通配退化精确匹配；`disallowedTools` 单独生效；deny+allow 同时出现按「先 deny 后 allow」。
- [ ] 既有四例不得修改（除 helper 字段）。

验证：`cargo test --manifest-path src-tauri/Cargo.toml filter::`

## Step 3 — 动态 schema + 内联角色 + 零工具拒绝启动

- [ ] `sub_agent.rs`：`agent_tool(defs: &[AgentDefinition])` / `tool_definitions(defs)` / `append_tool_definitions(tools, allow_spawn, defs)`。
- [ ] `subagent_type` 描述由 defs 拼装（`name — description`，逗号分隔），并说明可省略以走内联定义、以及如何往 `<app_data>/agents/` 写 `.md` 建常驻角色（含 frontmatter 字段名）。不加 `enum`（见 design 二）。
- [ ] input_schema 新增可选 `system_prompt: string` / `tools: array<string>` / `disallowed_tools: array<string>`；`additionalProperties: false` 保持。
- [ ] 新增纯函数 `fn apply_inline_overrides(def: &mut AgentDefinition, arguments: &Value)`：三个字段整体替换（非合并）；空字符串 / 空数组视为未提供。
- [ ] `handle_agent_spawn` 解析出 def 后调用它；过滤后若 `def.tools` 非空而剩余工具集为空，返回 `err_result` 并列出未命中条目。
- [ ] 新增单测：描述含全部 def 名；内联覆盖生效 / 缺省不变；零工具检测的消息含未命中条目。

验证：`cargo test --manifest-path src-tauri/Cargo.toml sub_agent::`

## Step 4 — 真实 SkillRegistry + skills preload

- [ ] `handle_agent_spawn`：`&SkillRegistry::default()` 换成 `skills::build_registry(ctx.app, &settings.chat_tools.skill_scan_paths).unwrap_or_default()`。**注册表始终全量，不按 `def.skills` 收窄**（规范：preload 不限制发现）。
- [ ] `def.skills` 非空时把所列技能的**全文**注入启动 context：复用 `build_chat_system_prompt` 的 `active_skill_detail` 通道（单个）或拼接后追加到 `custom_system_prompt`（多个）。**不给该函数加第 23 个参数。**
- [ ] `filter.rs` 不改（skill 工具继续无条件保留）。

验证：`cargo build --manifest-path src-tauri/Cargo.toml`

## Step 5 — 调用方同步

- [ ] `src-tauri/src/chat/commands/reply.rs:395`：加载 `agent_defs`（复用该函数已解析的 project root，不重复解析），传入 `append_tool_definitions`。
- [ ] `src-tauri/src/chat/agent/execute.rs:1400` / `:1418`：`agent_tool(&[])`。
- [ ] 其余编译错误逐个修。

验证：`cargo build --manifest-path src-tauri/Cargo.toml`

## Step 6 — 全量检查

- [ ] `cargo test --manifest-path src-tauri/Cargo.toml`（`--lib` 有约 14 个 pre-existing 环境/locale/path 失败，与干净 HEAD 对比判断，不是回归）
- [ ] `npm run lint && npm run typecheck`（本任务不动前端，跑一遍确认无类型联动）
- [ ] 手动冒烟：
  - `<app_data>/agents/my-analyst.md` → 主 Agent 询问「有哪些子代理角色」→ 应列出 my-analyst → 调用之
  - 带 `disallowedTools: write, edit` 的角色 → 子代理有 bash 与 MCP 工具但无写入
  - 带 `skills: pdf` 的角色 → 启动即具备 pdf 知识，无需先激活
  - `tools: reed_file`（故意拼错）→ 返回错误并指出未命中条目

## 回滚点

每个 Step 独立可编译，出问题回退到上一个 Step 的提交即可。分支 `feat/generic-subagents`，无数据迁移。
