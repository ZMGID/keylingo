# 子代理角色定义执行契约

> **适用**：改动 `src-tauri/src/agents/**`、`chat/agent/filter.rs`、`chat/sub_agent.rs` 的角色解析 / 工具收窄 / spawn 校验路径时**必读**。
>
> 子代理运行时本身（`run_agent_loop` 复用、并发信号量、深度守卫、级联取消）见 `chat/sub_agent.rs` 模块头注释；本文只约束**角色是怎么被定义、被发现、被收窄的**。

## 红线一：字段名照抄业界规范，不许自创方言

角色定义已有事实标准（Claude Code / Cursor / Gemini CLI / OpenCode 已收敛并互认目录）：`.md` + YAML frontmatter，body 即 system prompt。Kivio 用同一套字段名，用户从那些生态拷来的 `.md` 应当直接可用。

对应关系已经成立，不是可选项：

| 规范 | Kivio | 说明 |
|---|---|---|
| `name` / `description` / `model` | 同名 | `description` 是委派触发器，不是标签 |
| `tools` | 同名 | 允许列表；空 = 不收窄 |
| `disallowedTools` | 同名（同时接受 snake_case `disallowed_tools`，camelCase 优先） | 拒绝列表 |
| `skills` | 同名 | **preload**，不是可见性收窄 |
| `mcp__<server>__<tool>` | `tool_definition_from_mcp` 产出的 tool id 本来就是这个格式 | 通配语法白送 |

**MCP 通配语法必须是 `mcp__<server>` / `mcp__<server>__*` / `mcp__*`。** 曾经的设计草案写过 `notion:*` 这种方言——那是错的：Kivio 的 `mcp/types.rs::tool_definition_from_mcp` 早就是 `format!("mcp__{}__{}", server.id, tool.name)`，自创前缀既不兼容生态，又和自家 id 格式打架。

明确**不实现**的规范字段，各有理由，不要「顺手补齐」：

- `isolation: worktree` —— Kivio 没有 worktree 概念。
- `permissionMode` —— 子代理的 `request_tool_approval` 恒返回 false（无法向用户升级），三态无意义。
- `background` —— dispatch-and-return + 轮询的设计**试过并已删除**（退化成轮询且隐藏了运行中的子代理），见 `sub_agent.rs` 模块头。重新引入前先读那段。
- `hooks` / `mcpServers` 内联 / `memory` / `maxTurns` —— 范围外，要加得单独开任务。

## 红线二：先 deny 后 allow，顺序是规范强制的

```
pool -= { t | 命中任一 disallowed_tools 条目 }     // 阶段一
if !tools.is_empty():
    pool = { t | 命中任一 tools 条目 }             // 阶段二，在剩余池上解析
```

规范原文：*"If both are set, `disallowedTools` is applied first, then `tools` is resolved against the remaining pool."* 写反了会让 deny 形同虚设。

`filter_tools_for_agent` 的 retain 闭包里，顺序是：**剥离 `agent` → deny → 空 allow 早退 → skill 无条件保留 → allow**。

**deny 必须排在「skill 无条件保留」之前。** 否则 `disallowedTools: skill` 失效，skill 成为唯一不可禁的工具。而 allow 侧保留 skill 是「不点名也留着」的宽容语义，两者不冲突。

### 为什么要有 deny（别把它当成白名单的冗余）

`reviewer` 这类角色的真实语义是「继承全部、挖掉写入」。用穷举白名单表达，**新增只读工具时会静默漏掉**。deny 是唯一能表达继承减法的手段。

内置四角色目前仍用穷举白名单，**是故意的**：改用 deny 会让 reviewer 从此能用 MCP 工具和 knowledge_search，属行为变更，需单独决策。不要顺手「优化」掉。

## 红线三：收窄成空集必须拒绝启动，且归因要分两种

规范要求：allow 解析为空集时报错并列出未命中条目，而不是静默启动零工具子代理。Kivio 在此之上还踩过两个坑，都由 `sub_agent.rs::zero_tool_refusal` 固化：

**坑一：守卫不能只看 allow。** 早期判据是 `!def.tools.is_empty()`，于是只传 `disallowed_tools: ["*"]` 而不传 `tools`（general-purpose 就是空白名单）会绕过检测，静默启动零工具子代理。**判据必须是「任一列表非空」**。

**坑二：`unresolved_allow_entries` 在 deny-all 下会说谎。** 它跳过被 deny 的工具，所以 deny-all 之下每个 allow 条目都被报成「未命中」——即使拼写完全正确。实机上模型发了 `{subagent_type: "coder", disallowed_tools: ["*"]}`，错误消息把 `read, grep, glob, edit, write` 五个**正确**的名字列为 unresolved，把模型引向去修根本没错的名字。

因此 `deny_emptied_pool` **必须在收窄前、在原始 catalog 上单独计算**，不能靠 `unresolved.is_empty()` 反推。两种成因分别措辞：

- deny 清空 → `disallowed_tools (...) removed every tool. Narrow the deny-list, or use tools to allow-list...`
- 条目拼错 → `These entries matched no available tool: ...`

回归守卫：`deny_all_refusal_blames_the_denylist_not_the_allow_entries`、`deny_all_is_caught_even_when_no_allow_list_is_set`。

## 红线四：角色清单是数据，模型必须每轮看得见

`agent` 工具的 `subagent_type` 描述由 `subagent_type_description(defs)` 在**每次组装工具表时**按实际加载到的角色列表生成。曾经这里是硬编码字符串「general-purpose, researcher, coder, reviewer, or a user/project-defined type」——最后那半句是空话，用户放了 `.md` 模型也猜不到名字，等于角色层不存在。

**不要加 JSON-Schema `enum` 收紧它。** 两个原因：运行中新建的角色会被 provider 侧直接拒绝（要等下一轮才生效）；内联临时角色根本不传这个字段。软失败 + 列出可用角色比硬拒绝有用。

`agent_tool(&[])` 只在两个地方合法：`native_registry.rs` 的静态 `def` 槽位和测试——那里只需要 name/shape，真正的 schema 永远在 `append_tool_definitions` 里按请求现建。

## 内联角色是整体替换，不是合并

`agent` 调用的 `system_prompt` / `tools` / `disallowed_tools` 以命名角色为基底**整体替换**对应字段。合并语义表达不了「以 researcher 为基底但去掉 web_fetch」，替换能表达任意组合且规则单一。空字符串 / 空数组视为「未提供」（否则空数组会变成「禁止一切」的收窄）。

## skills 是 preload，不是可见性收窄

规范语义：把所列技能的**全文**注入启动 context，且**不阻止**子代理通过 skill 工具调用未列出的技能。所以传给 `build_chat_system_prompt` 的注册表**永远是全量**，`filter.rs` 不因 `skills` 做任何过滤。

**注入走 persona（`custom_system_prompt`）通道，不要改走 `active_skill_detail`。** 后者只在 `skill_fallback_mode` 为 `skill_md_only` / `legacy_full_body` 时渲染（`prepare.rs`），而默认是 `progressive`——走那条路会**静默不注入**，测试还看不出来。这个坑在设计阶段写错过一次。

顺带记住：这条路径修复了一个长期 bug —— 子代理原先拿的是 `SkillRegistry::default()`（空目录），而 `filter.rs` 又特意保留 skill 工具，形成「工具在、目录空」。改动这一带时别把注册表退回默认值。

## 目录与优先级

只读 Kivio 自己的两层，**不做跨厂商目录读取**（不读 `.claude/agents/`、`.cursor/agents/`）：

```
builtin → <app_data>/agents/*.md → <project>/.kivio/agents/*.md
```

后覆盖前，按 id。字段与规范对齐已足够让用户手动拷贝复用生态角色文件；跨厂商读取会把优先级链拉到 6 层，还要处理 `isolation` / `permissionMode` 这些不兼容字段的静默忽略。要做得先评估这两项成本。

## 无头验证

`chat_probe` 通道（debug-only）能跑真实生成路径，写 `<app_data>/chat_probe/request.json`，读 `result-<id>.json`。角色相关的六条已验证过：动态发现 / 纯内联角色 / deny-all 归因 / 拼错归因 / preload 注入 / deny 真的挡住写入。改动本文覆盖的任一红线后，至少重跑「deny-all 归因」和「preload 注入」两条——它们都是单测覆盖不到的端到端语义。
