# 设计：Sub-Agent 通用化

字段与语义**对齐业界事实标准**（见 prd.md「业界规范」）。凡规范已定义的，照抄；规范未覆盖或与 Kivio 架构冲突的，在此说明取舍。

## 一、总体判断

不新建子系统。四处改动，全部落在既有文件里：

| 文件 | 改动 |
|---|---|
| `src-tauri/src/agents/types.rs` | `AgentDefinition` 加 `disallowed_tools` / `skills` |
| `src-tauri/src/agents/parse.rs` | 解析 `disallowedTools` / `skills` frontmatter |
| `src-tauri/src/chat/agent/filter.rs` | 规范通配匹配 + 先 deny 后 allow + 零工具检测 |
| `src-tauri/src/chat/sub_agent.rs` | `agent_tool()` 动态 schema；内联角色；真实 SkillRegistry；零工具拒绝启动 |
| `src-tauri/src/chat/commands/reply.rs` | 把动态角色清单喂给 `append_tool_definitions` |

不动 `AgentHost` / `run_agent_loop` / 并发信号量 / 深度守卫 / 级联取消。

## 二、动态角色 schema（R1）

### 现状

`agent_tool()` 是零参函数，返回常量 schema。`append_tool_definitions(&mut tools, allow_spawn)` 在 `reply.rs:395` 调用。

### 改法

```rust
// sub_agent.rs
pub fn agent_tool(defs: &[AgentDefinition]) -> ChatToolDefinition
pub fn tool_definitions(defs: &[AgentDefinition]) -> Vec<ChatToolDefinition>
pub fn append_tool_definitions(tools: &mut Vec<ChatToolDefinition>, allow_spawn: bool, defs: &[AgentDefinition])
```

`subagent_type` 的 `description` 由 defs 拼装：

```
Named agent role. Available: general-purpose — General-purpose agent…; researcher — Read-only research…; my-analyst — 财报结构化分析.
Omit to use general-purpose, or omit and pass system_prompt/tools to define an ad-hoc role inline.
```

同时给 `enum` 字段填上已知 id（模型侧硬约束）—— **不填**。理由：R2 允许内联定义时不传 `subagent_type`，且用户可能在运行中新建 `.md`；`enum` 会让新角色在下一轮才生效且拼错时直接被 provider 侧拒绝，反而不如现在的「软失败 + 列出可用角色」错误信息。描述里列全即可。

调用方 `reply.rs` 需先加载 defs：

```rust
let agent_defs = crate::agents::load_agent_definitions(app, project_root.as_deref());
crate::chat::sub_agent::append_tool_definitions(&mut tools, true, &agent_defs);
```

`reply.rs` 已经解析出 project root（`resolve_conversation_project`）—— 复用，不重复解析。

`execute.rs:1400/1418` 两处 `agent_tool()` 只用来取 name/schema 做校验，传 `&[]` 即可（描述内容与校验无关）。

### 角色目录的自助扩展（R2 的持久化半边）

在 `prepare.rs` 已有的 sub-agent 委派段落里追加一句，告知主 Agent：把 `.md` 写到 `<app_data>/agents/` 就能新增常驻角色，并给出 frontmatter 字段名（与业界规范同名，用户从 Claude Code / Cursor 拷来的文件可直接用）。目录路径从 `user_agents_dir()` 取。纯 prompt 改动，零新增工具。

## 三、内联角色（R2）

`agent` 工具 input_schema 新增三个可选字段：

```jsonc
"system_prompt":     { "type": "string", "description": "Ad-hoc persona for this run. Overrides the named role's prompt." },
"tools":             { "type": "array", "items": {"type":"string"}, "description": "Ad-hoc allow-list. Supports `mcp__<server>__*`, `mcp__*`, `*`." },
"disallowed_tools":  { "type": "array", "items": {"type":"string"}, "description": "Ad-hoc deny-list, applied before `tools`." }
```

spawn 侧解析后覆盖：

```rust
let mut def = resolved_def.clone();
if let Some(p) = inline_system_prompt { def.system_prompt = p; }
if let Some(t) = inline_tools { def.tools = t; }
if let Some(d) = inline_disallowed { def.disallowed_tools = d; }
```

覆盖语义 = 整体替换（不是合并）。理由：合并语义在「以 researcher 为基底但去掉 web_fetch」这种场景下无法表达，而整体替换足够表达任何组合，且规则单一。

`subagent_type` 缺省时仍解析为 `general-purpose`（空 prompt / 空白名单），内联字段直接填进去 —— 无需为「纯内联」开特例分支。

## 四、工具匹配：规范通配 + 先 deny 后 allow（R3）

### 语法就是 Kivio 现有的 id 格式

`tool_definition_from_mcp`（`mcp/types.rs:177`）已经产出 `mcp__{server_id}__{tool_name}`，与规范的 `mcp__<server>__*` 完全同构。**不自创方言**。支持的条目形态：

| 条目 | 含义 |
|---|---|
| `*` | 全部工具 |
| `mcp__*` | 全部 MCP 工具（任意 server） |
| `mcp__notion` | Notion server 的全部工具（规范：server 名单独出现即整服放行） |
| `mcp__notion__*` | 同上，显式通配写法 |
| `read` / `web_fetch` | 精确名（含 legacy 别名归一化，走现有 `tool_matches_recommended_name`） |

### 实现

`filter.rs` 现在调用 `prepare::tool_matches_recommended_name`（精确比对 name / id / openai_tool_name / `server_id:name`）。该函数还被 persona/skill 白名单复用 —— **不改它**，在 `filter.rs` 内新增一层：

```rust
fn entry_matches(tool: &ChatToolDefinition, entry: &str) -> bool {
    if entry == "*" { return true }
    // `mcp__notion` / `mcp__notion__*` → 整服匹配（规范语义）
    if let Some(rest) = entry.strip_prefix("mcp__") {
        let server = rest.trim_end_matches('*').trim_end_matches("__");
        if server.is_empty() { return tool.source == "mcp" }          // mcp__*
        if tool.server_id.as_deref() == Some(server) { return true }
    }
    if let Some(prefix) = entry.strip_suffix('*') {
        return candidates(tool).iter().any(|c| c.starts_with(prefix))
    }
    tool_matches_recommended_name(tool, entry)   // 精确 + legacy 别名
}
```

`candidates(tool)` = `[name, id, openai_tool_name()]`。

**只支持后缀 `*`**，不做完整 glob。规范里出现的形态（`mcp__*`、`mcp__server__*`）全是前缀匹配；引 glob crate 是为不存在的需求付代价。

### 组合顺序（规范强制）

```
pool = all_enabled_tools
pool -= { t | any(entry_matches(t, e)) for e in disallowed_tools }   // 先 deny
if !tools.is_empty():
    pool = { t | any(entry_matches(t, e)) for e in tools }           // 再 allow
```

`agent` 工具在两步之外**无条件剥离**（递归守卫，现有行为不变）。

### 零工具检测（规范要求）

`tools` 非空但过滤后剩余集合为空（或只剩被无条件保留的 skill 工具）时，spawn 返回 `err_result`，消息列出未命中的条目：

```
Sub-agent 'x' would launch with zero tools. Unresolved entries: reed_file, mcp__notionn__*.
```

软失败（`Ok` + `is_error`），父循环继续 —— 与现有其它 spawn 失败路径一致。「未命中」= 该条目没匹配到 pool 里任何工具。

## 五、子代理技能：preload（R4）

### 现状 bug

```rust
build_chat_system_prompt(…, &SkillRegistry::default(), …)   // sub_agent.rs:1002
```

技能目录恒为空，而 `filter.rs` 又特意保留 skill 工具。工具在、目录不在。

### 改法

1. spawn 时构建真实注册表：`skills::build_registry(ctx.app, &settings.chat_tools.skill_scan_paths).unwrap_or_default()`（与 `reply.rs:319` 同一路径）。这一条独立成立，是纯 bug 修复。
2. `AgentDefinition` 新增 `skills: Vec<String>`，语义按规范为 **preload**：所列技能的**全文**在启动时注入 context。复用 `build_chat_system_prompt` 已有的 `active_skill_detail` 通道（它就是「注入某技能全文」的现成机制）—— 单个技能直接传；多个技能则拼接后走 `custom_system_prompt` 追加，不新增第 23 个参数。
3. **不收窄注册表**。规范明确：preload 不阻止子代理通过 skill 工具调用未列出的技能。所以 `filter.rs` 保持不变（skill 工具继续无条件保留），注册表始终是全量。

这比原设计（按 `skills` 收窄可见目录）更简单：少一处 `retain`，且语义与生态一致。

## 六、兼容性

- `AgentDefinition` 新增字段用 `#[serde(default)]` / `Vec::new()`，四个内置角色显式给空值。
- `parse.rs` 未写 `skills` / `disallowedTools` → 空 vec → 行为不变。
- `tools` 无通配 → 走原精确匹配路径 = 现行为。
- `filter.rs` 四个既有单测应全绿，仅需给 `def()` helper 补字段。
- frontmatter 键名用规范的 camelCase `disallowedTools`，同时接受 snake_case `disallowed_tools`（Kivio 既有 frontmatter 风格）—— 两个键都查，成本一行。
- `agent_tool()` 签名变更是 crate 内部 API，三处调用点全部同步。

### 内置角色改用 deny 表达

`researcher` / `reviewer` 现在穷举只读工具白名单（`read/grep/glob/web_search/web_fetch`），新增只读工具时会漏。改用规范的继承减法：

```rust
// reviewer: 继承全部，挖掉写入与执行
disallowed_tools: ["write", "edit", "bash"]
tools: []
```

**但这会改变现有行为**（reviewer 从此能用 MCP 工具和 knowledge_search）。R5 要求「行为与改动前一致」，故**本次不改内置角色定义**，只让 deny 能力对用户角色可用。内置角色的语义升级留作独立决策。

## 七、测试

| 层 | 用例 |
|---|---|
| `filter.rs` tests | `mcp__notion__*` 与 `mcp__notion` 都放行同 server 全部工具且挡掉其它 server；`mcp__*` 放行全部 MCP 工具但挡掉 native；`*` 放行全部；无通配退化为精确匹配；`disallowedTools` 单独生效；deny+allow 同时出现时先 deny 后 allow；既有四例不动 |
| `agents/parse.rs` tests | 解析 `skills: pdf, docx`；解析 `disallowedTools` 与 `disallowed_tools` 两种键名；缺省为空 |
| `sub_agent.rs` tests | 动态 schema 的 `subagent_type` 描述含全部 def 名；`apply_inline_overrides` 覆盖生效 / 缺省不变；零工具检测返回未命中条目 |

需要能造带 `server_id` 的 MCP 工具（现有 helper 只有 `native()`），补一个 `mcp(server, name)`。

无法单测的（真实 spawn、SkillRegistry 装配、preload 注入）留手动冒烟：新建 `.md` 角色 → 主 Agent 能列出并调用；带 `skills: pdf` 的角色启动后能直接用 pdf 知识而无需先激活。

## 八、回滚

单分支 `feat/generic-subagents`，五个文件，无数据迁移、无 settings schema 变更、无持久化格式变更。`git revert` 即可完全回退。
