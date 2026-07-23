# Implement — 从本地 CLI 导入 Skill/MCP

## 顺序 checklist

### 后端
1. [ ] `src-tauri/Cargo.toml`：加 `toml = "0.8"`。
2. [ ] `src-tauri/src/mcp/registry.rs`：
   - 加 `CliMcpGroup` / `CliImportScan`（serde camelCase）。
   - `parse_claude_mcp(home)`：读 `~/.claude.json`，复用 `CursorMcpJson`（顶层 `mcpServers`）+ `normalize_imported_transport` + `ChatMcpServer` 映射（同 `chat_mcp_import_json`）。
   - `parse_codex_mcp(home)`：`CodexMcpServer` 结构 + `toml::from_str`，全 stdio。
   - `parse_opencode_mcp(home)`：`OpencodeMcpServer` 结构 + `serde_json::from_str`，local→stdio(command 数组拆分)/remote→streamable_http。
   - `#[tauri::command] chat_cli_import_scan()`：拼三组，缺文件→`available:false`，解析失败→`available:true, servers:[]`。
3. [ ] 注册命令：`src-tauri/src/lib.rs` 的 `invoke_handler` 加 `chat_cli_import_scan`。
4. [ ] 单测（registry.rs `#[cfg(test)]`）：三个解析器各喂一段真实样例（见 design 中本机 codegraph/codex 样例），断言 transport/command/args/env 正确；缺文件返回空组不 panic。

### 前端
5. [ ] `src/api/tauri.ts`：加 `CliImportScan`/`CliMcpGroup` 类型 + `chatCliImportScan()`。
6. [ ] `src/chat/McpCenter.tsx`（`view==='import'`）：加「从本地 CLI 导入」块——扫描按钮、按 CLI 分组 + checkbox（默认全勾）、「导入选中」→ 各 `handleInstall`。
7. [ ] `src/chat/SkillCenter.tsx`（`view==='import'`）：加「从 Claude Code 导入」块——扫描（`chatSkillsList([~/.claude/skills])`，home 用 `@tauri-apps/api/path` 的 `homeDir()`）、checkbox、「导入选中」→ 逐项 `chat_skills_import(dirname(skill.path))`，完成后 `refreshChatSkills()`+`onSkillsChanged()`。

## 校验命令
- `cargo test --manifest-path src-tauri/Cargo.toml registry` （或相关模块名）
- `npm run lint`
- `npm run typecheck`
- 手动冒烟：MCP 页导入 tab 扫描→勾选→导入→已安装能启用；Skill 页扫描→勾选→导入→已安装出现。

## 回滚点
- 每完成「后端」与「前端」两阶段各自可独立回滚（后端命令无 UI 调用即死代码；前端不加块即无入口）。
- 全撤：删命令 + 两块 UI + Cargo.toml 一行。
