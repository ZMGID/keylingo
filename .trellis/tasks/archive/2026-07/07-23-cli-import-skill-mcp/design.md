# Design — 从本地 CLI 导入 Skill/MCP

## 边界与契约

新增一个后端命令做「扫描」，导入动作全部复用现有命令/前端逻辑。

### 后端

**新命令 `chat_cli_import_scan()`**（放 `src-tauri/src/mcp/registry.rs`，紧挨现有 `chat_mcp_import_json`）：

```rust
#[derive(Serialize)] #[serde(rename_all = "camelCase")]
struct CliMcpGroup { available: bool, servers: Vec<ChatMcpServer> }
#[derive(Serialize)] #[serde(rename_all = "camelCase")]
struct CliImportScan { claude: CliMcpGroup, codex: CliMcpGroup, opencode: CliMcpGroup }

#[tauri::command]
fn chat_cli_import_scan() -> CliImportScan
```

- 家目录：复用已有 `directories`（`BaseDirs::new().home_dir()`）。
- `available` = 配置文件存在（不是有没有 server）。
- 每个 server `id = "mcp-{uuid}"`，`enabled=false`，`connector_id=None`，`auth=None`（同现有 import）。

**三个解析器**（均在 registry.rs，纯函数，各带单测）：

- `parse_claude_mcp(home) -> CliMcpGroup`
  - 读 `~/.claude.json`，取顶层 `mcpServers` 对象；直接复用现有 `CursorMcpServer` + `normalize_imported_transport`（schema 完全一致：`type`/`command`/`args`/`env`/`url`/`headers`）。
  - 跳过 `projects[*].mcpServers`。
- `parse_codex_mcp(home) -> CliMcpGroup`
  - 读 `~/.codex/config.toml`，`toml::from_str` 到 `{ mcp_servers: HashMap<String, CodexMcpServer> }`。
  - `CodexMcpServer { command:String, #[serde(default)] args:Vec<String>, #[serde(default)] env:HashMap<String,String>, cwd:Option<String> }`（忽略 `enabled`/`startup_timeout_sec`）。
  - 全部 `transport="stdio"`。
- `parse_opencode_mcp(home) -> CliMcpGroup`
  - 读 `~/.config/opencode/opencode.json`，`serde_json::from_str` 到 `{ mcp: HashMap<String, OpencodeMcpServer> }`。
  - `type=="remote"` → `transport="streamable_http"`，`url`+`headers`。
  - 否则（`local`）→ `transport="stdio"`，`command:Vec<String>` 拆成 `command[0]` + `args=command[1..]`，`environment`→`env`。

单个 CLI 的解析失败（文件损坏/格式变）不应炸整个扫描：解析出错时该组 `available:true, servers:[]`（或 available 保真、servers 空），不 panic、不返回 Err。

**新增依赖**：`toml = "0.8"`（Cargo.toml）。

**Skill 扫描不写后端**：前端直接 `chat_skills_list([...三个 CLI 的技能目录])`（该命令已支持传扫描路径，把额外路径标为 `source:"external"`，返回带 `path` 的 `SkillMeta`）。扫描路径（用 `homeDir()` 拼）：
- Claude：`~/.claude/skills`
- Codex：`~/.codex/skills`
- OpenCode：`~/.config/opencode/skills`、`~/.opencode/skills`

前端按 `skill.path` 的目录前缀把结果归到对应 CLI 分组（同 MCP 的分组展示）。registry 会按 id 去重，跨 CLI 同名技能只出现一条（归到 registry 精度胜出的那个路径所属分组）——可接受。

### 前端

`src/api/tauri.ts` 加：
```ts
chatCliImportScan: () => invoke<CliImportScan>('chat_cli_import_scan')
```
+ `CliImportScan` / `CliMcpGroup` 类型。

**MCP 页**（`src/chat/McpCenter.tsx`，`view==='import'`）：现有「导入 mcp.json」块下方加「从本地 CLI 导入」块。
- 「扫描」按钮 → `chatCliImportScan()`。
- 按 CLI（Claude Code / Codex / OpenCode）分组渲染；`available:false` 的组不渲染或灰显「未检测到」。
- 每个 server 一行 checkbox（默认全勾）。「导入选中 (n)」按钮 → 对选中项各调现有 `handleInstall(server)`（已 append 到 servers）。导入后给个反馈并可切到「已安装」。

**Skill 页**（`src/chat/SkillCenter.tsx`，`view==='import'`）：现有 folder/zip + URL 导入下方加「从本地 CLI 导入」块。
- 「扫描」按钮 → `chatSkillsList([~/.claude/skills, ~/.codex/skills, ~/.config/opencode/skills, ~/.opencode/skills])`（home 用 `homeDir()`）。
- 按 `skill.path` 目录前缀归入 Claude Code / Codex / OpenCode 三组；空组不显示。
- checkbox 列表（默认全勾）。「导入选中」→ 逐项从 `skill.path`（`.../<id>/SKILL.md`）推出文件夹（`dirname`），调 `chat_skills_import(folder)` 复制。全部完成后 `refreshChatSkills()` + `onSkillsChanged()`。

## 数据流

扫描（MCP）：`chat_cli_import_scan` → 读 3 文件 → 3 解析器 → `ChatMcpServer[]` 分组 → 前端勾选 → `handleInstall` → `mutateServers`（先读后端 fresh 再合并整存）→ settings。

扫描（Skill）：`chat_skills_list([~/.claude/skills])` → `SkillMeta[]`(带 path) → 前端勾选 → `chat_skills_import(dirname(path))` → 复制到 user_skills_dir → 刷新列表。

## 兼容性 / 回滚

- 纯新增：新命令 + 两个 import tab 的新 UI 块 + 一个 crate。不改现有命令/类型/存储。
- 回滚 = 删新命令、删两块 UI、撤 Cargo.toml 依赖。无数据迁移。

## 取舍

- 复用 `handleInstall`/`chat_skills_import` 而非新写导入路径：导入语义与「市场安装」「本地导入」一致，零重复。
- Skill 扫描复用 `chat_skills_list` 而非新命令：`~/.claude/skills` 本就是合法 SKILL.md 目录，Kivio 解析器直接吃。
- 单个 CLI 解析失败降级为空组而非整体失败：一个坏配置不该拖垮其它两个。
