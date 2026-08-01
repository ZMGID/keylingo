# 实现进度

## 已完成

### 任务 1 — ACP 能力验证（临时探针，已弃）

结论写进了 [ADR-0003](../../../docs/adr/0003-acp-agents-import-via-protocol.md) 的能力矩阵。要点：opencode 全支持（含历史重放），kimi 能绑定但不重放历史，gemini 完全不支持（因此出局）。

### 任务 2 — 可导入会话枚举

`src-tauri/src/external_agents/import.rs`（新模块，已挂进 `external_agents/mod.rs`）
+ `session/acp.rs` 末尾新增 `probe_acp_sessions` / `AcpSessionSummary` / `read_rpc_response`。

对外接口：

- `list_file_based_sessions(project_root) -> Vec<ImportableSession>` —— claude + grok + codex，同步。
- `list_acp_sessions(agent_id, project_root) -> Option<Vec<ImportableSession>>` —— opencode / kimi / cursor，异步。`None` = 该代理不支持导入，`Some(vec![])` = 支持但本目录无会话，两者界面上要区分。
- `ACP_IMPORT_AGENTS` = `["opencode", "kimi", "cursor"]`。
- `paths_match` / `canonical_key` —— realpath-aware 路径比对，供后续任务复用。

实现上的三个判断（都写在代码注释里）：

1. **没有复刻 claude 的目录编码规则。** jsonl 每条都明文带 `cwd`，扫目录读首行比正向算目录名稳——免疫那个 200 字符截断和未文档化的哈希。代价是全量扫首行，标了 `ponytail:` 注释说明上千文件时再优化。
2. **grok 枚举只读 `summary.json`**，不碰 `chat_history.jsonl`——`info.id` / `info.cwd` / `num_chat_messages` / `session_summary` 它全给了。注意用 `num_chat_messages`，`num_messages` 实测常年为 0。
3. **`message_count` 是 `Option<usize>`。** ACP `session/list` 不返回条数，填 0 会在界面上显示成"0 条"——那是错的信息，得显示"未知"。
4. **codex 的归属用 `session_meta.payload.cwd`，不用 `turn_context.cwd`。** 后者是每轮的实际目录，同一条会话在别处 resume 时会不同（本机实测就有这种记录）。

测试：12 个单测 + 3 个 `#[ignore]` 真实数据冒烟测试。跑法：

```
cargo test --manifest-path src-tauri/Cargo.toml --lib external_agents::import
KIVIO_IMPORT_ROOT="<项目根>" cargo test --manifest-path src-tauri/Cargo.toml \
  --lib external_agents::import::tests::smoke -- --ignored --nocapture
```

本机实测结果（2026-08-01）：`C:\Users\11028` → claude 3 / codex 38 / opencode 19 / kimi 2；
`E:\ZM database\kivio cl` → claude 42 / codex 4 / grok 7；cursor 未装，优雅报"不支持"。

### 任务 3（部分）— claude 历史解析器

`src-tauri/src/external_agents/import_history.rs`（新模块）。纯函数，不碰 `AppHandle`、不写盘。

`parse_claude_history(raw) -> Vec<ImportedMessage>`，其中 `ImportedMessage { message: ChatMessage, images: Vec<ImportedImage> }`——内联图片**原样吐出、不落盘**，因为 `Attachment.path` 是对话附件目录的相对路径，只有导入命令那层才知道对话 id。

两个真实数据才暴露的坑：

1. **claude 把 `<system-reminder>` / `<local-command-caveat>` 这类注入当成独立的 `user` 记录**。原来按记录切消息，一个回合的输入被劈成一串气泡。改成**只在角色变化时收尾**，同角色连续记录合并。合成用例没覆盖到，是 10MB 真会话的断言抓出来的。
2. **`tool_result` 是以 `role: user` 回来的**。见到 `user` 就当新用户消息的话，每次工具调用都会多一条空白气泡。判据是这条记录里有没有真正的文本块。

`tool_result` 按 2KB **在字符边界**截断（直接切字节会在多字节字符中间 panic）。`model_messages` / `api_messages` 刻意留空（ADR-0002：快照不参与模型输入）。

另一个查清楚的事实：**claude 的 `thinking` 块正文经常是空的**，只留 `signature`（加密推理）。全库抽查 16 个块里 5 个空。所以导入的历史可能没有思考内容——这不是解析 bug。

实测（10MB / 3522 行的真会话）：126 条消息（user 63 / assistant 63，严格交替），719 个工具调用**全部配到结果**，13 张图片，最大 `result_preview` 2106 字节。另跑一条有正文思考的会话确认 reasoning 路径通。

跑法：

```
cargo test --manifest-path src-tauri/Cargo.toml --lib external_agents::import_history
KIVIO_CLAUDE_JSONL="<某条 .jsonl>" cargo test --manifest-path src-tauri/Cargo.toml \
  --lib external_agents::import_history::tests::smoke_parse_real -- --ignored --nocapture
```

### 任务 3（完成）— grok / codex 解析器 + 相邻合并

`parse_grok_history` 读 `<session>/chat_history.jsonl`：`assistant` 是 OpenAI 风格
（`content` 字符串 + `tool_calls[]`），`reasoning` 的正文在 `summary[].text`（另有
`encrypted_content` 不用管），`tool_result` 单独一行按 `tool_call_id` 回填，图片是
`data:image/...;base64,` 形式的 data URL（**和 claude 的 `source.data` 不同**，两边不能共用提取函数），
`system` 和 `backend_tool_call`（内置联网搜索）跳过。

`parse_codex_history` 读 `rollout-*.jsonl`：显示正文取 `event_msg` 的 `user_message` /
`agent_message`——`response_item.message` 里混着 `role: developer` 的权限说明注入，不是对话内容。
工具有**两套**都要认：`function_call`/`function_call_output` 和 `custom_tool_call`/`custom_tool_call_output`
（`apply_patch` 走后者），只认一套的话打补丁的回合会凭空少掉。

第二个真实数据 bug：**codex 在中断和上下文压缩时会写空的 `user_message`**，把角色切过去又因为
没内容被丢弃，导致两条 assistant 在结果里相邻。修法是出口统一做一次 `coalesce_adjacent`
（合并相邻同角色 + 重排段落序号），而不是在每个解析器里各打补丁——三个解析器共用。

实测：grok 3.2MB → 82 条消息 / 310 工具全配对 / 41 条思考 / 8 张图；
codex 24MB → 46 条消息 / 372 工具全配对。

**kimi 只绑定、不带历史快照**（理由见 design.md）：它 `session/load` 不重放，本地
`wire.jsonl` 的 `context.append_message` 只落用户消息，assistant 正文散在 loop event 里
没有稳定形态。导入后消息区为空 + 一条说明。

### 任务 4 — 导入命令 + 会话绑定

`import.rs` 下半部分 + `external_agents/commands.rs` 三个 Tauri 命令（已注册进 `lib.rs`）：

- `chat_list_importable_cli_sessions(projectId)` → 按项目根过滤的会话列表，已导入的带 `alreadyImported`。
- `chat_import_cli_sessions(projectId, items[])` → 批量导入，**单条失败不影响其它条**，返回 `{imported[], failures[]}`。
- `chat_imported_history_stale(conversationId)` → 过期检测。

关键实现点：

- **绑定文件写哪种由 `resumes_session_via_cli` 决定**：claude 写 `external-agent-sessions/<conv>.json`（`stable_prompt_hash` 留空——首轮重发 instructions，但**不会**丢会话；填假值反而会让首轮误跳过），其余写 `live-<conv>.json`（`LiveSessionHandle`，带 cwd）。写错了续聊会开一条全新会话、历史静默丢失。
- **`ChatMessage.timestamp` 是秒不是毫秒**（前端 `nowSeconds()`）。解析器内部按毫秒累积，`finish()` 里 `/1000`。不换算的话导入的消息显示成公元五万年。
- **图片到这一层才落盘**：解析器只吐 base64，导入命令写进 `conversations/<id>_attachments/`。单张写失败只跳过那张，不让整条导入失败。
- **已导入判定直接扫绑定文件**，不另维护索引——绑定文件才是真相源，多一份索引就多一个会不一致的地方。三种文件形态（camelCase 的 `conv_*.json`、snake_case 的 `live-*.json`、`imported-*.json`）字段名不同，读取时都兜住。
- **过期检测只比来源文件的 mtime**。走 ACP 重放的（opencode）没有文件可比，一律返回 `false`——宁可不提示，也不要靠一次昂贵的 ACP 握手去猜，更不要误报。

同时给 `session/acp.rs` 加了 `probe_acp_session_history`（`initialize` → `session/load` → 收 `session/update`，响应后再留 3s 尾窗口接住延后推送），配套 `import_history::parse_acp_updates` 把 chunk 拼成消息。

### 任务 5 — 前端入口与导入列表

- `ProjectContextMenu.tsx` 加「从 CLI 导入对话」，没绑项目文件夹时禁用（工作目录必须对齐是硬约束）。
- `CliImportDialog.tsx`（新）：按 CLI 分组、显示标题/条数/时间、多选批量导入；已导入的置灰不可选；kimi 分组上直接标「不提供可读历史，导入后消息区为空，但续聊正常」——这话必须在勾选**前**说，不能等导完让用户以为丢了数据。
- `chat/api.ts` + `chat/types.ts` 加三个方法与 `ImportableCliSession` / `CliImportResult`。

### 任务 6 — 过期提示横幅

`Chat.tsx`：切换对话时问一次 `importedHistoryStale`，为真则在消息列表上方挂一条琥珀色 chip（与既有的「分叉自」chip 同位置同样式）。检查失败按"未过期"处理，不打断打开对话。

## 验证状态

- Rust：`cargo test --lib external_agents::import` 23 passed。`external_agents` 全量 377 passed / 1 failed，那 1 条（`cli_command_strips_parent_session_env_from_the_child`）在**干净 HEAD 上同样失败**（跑 `/usr/bin/env`，Windows 没有），是 CLAUDE.md 记录的预存在失败，不是回归。
- 前端：`npm run typecheck` 干净，`npm run lint` 干净，`npm test` 83 files / 592 tests 全绿。
- **未做真机验收**：导入→打开→续聊这条链路没有在跑起来的 app 里点过。下一步就是它。

## 剩余风险

`session/load` 用**别的 CLI 在自己 TUI 里创建的**会话续聊，探针层面验过（opencode 能加载并重放 15 条 update），但没有在 Kivio 的实际续聊路径上跑通过。真机验收第一件事就是：导入一条 opencode 的 TUI 会话，发一条消息，看它是接着聊还是开了新会话。
