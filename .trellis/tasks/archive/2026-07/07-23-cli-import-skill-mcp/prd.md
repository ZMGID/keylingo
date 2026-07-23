# 从本地 CLI 导入 Skill/MCP

## Goal

在 Chat 窗口「扩展」的 MCP 页和 Skill 页的「导入」tab 里，新增「从本地 CLI 导入」：扫描本机已安装的
Claude Code / Codex / OpenCode 的配置，把它们已配置的 MCP 服务器（三者）和 Skill（仅 Claude Code）
以**勾选后复制**的方式导入 Kivio。

## Requirements

- **MCP 导入**（Claude Code / Codex / OpenCode 三者）：
  - Claude：读 `~/.claude.json` 顶层 `mcpServers`（schema 同现有 Cursor mcp.json）。
  - Codex：读 `~/.codex/config.toml` 的 `[mcp_servers.*]`（TOML；字段 `command`/`args`/`cwd`/`env` 子表；全 stdio）。
  - OpenCode：读 `~/.config/opencode/opencode.json` 的 `mcp.*`（`type:local`→stdio，`command` 是数组 `[cmd,...args]`，`environment`→env；`type:remote`→streamable_http，`url`/`headers`）。
  - 扫描结果按 CLI 分组，用户逐项勾选，导入 = 复制成 Kivio 自己的 `ChatMcpServer`（`enabled:false`，走现有安装流程）。
- **Skill 导入**（Claude Code / Codex / OpenCode 三者——三者现均支持同一 SKILL.md 标准）：
  - 扫描各 CLI 的技能目录：Claude `~/.claude/skills`、Codex `~/.codex/skills`、OpenCode `~/.config/opencode/skills` + `~/.opencode/skills`（两处都扫，实际安装位置因版本而异）。
  - 结果按 CLI 分组（同 MCP），用户逐项勾选，导入 = 把该技能文件夹复制进 Kivio 用户技能目录（复用 `chat_skills_import`）。
  - 跨 CLI 同名技能在扫描列表里会被 registry 按 id 去重折叠为一条（可接受；导入的是同一份 SKILL.md 标准内容）。
  - 不纳入 `~/.agents/skills`（跨工具共享标准，不属于单个 CLI）。
- 扫不到某 CLI 的配置文件 = 该 CLI 分组灰掉/隐藏，不报错。

## Acceptance Criteria

- [ ] MCP 页「导入」tab 有「从本地 CLI 导入」块；点扫描后按 CLI 分组列出可导入的 MCP 服务器（本机三者的 codegraph 等能正确解析出 command/args/env/transport）。
- [ ] 勾选若干条 → 导入 → 出现在 MCP 页「已安装」，`enabled:false`，可正常启用连接。
- [ ] Skill 页「导入」tab 有「从 Claude Code 导入」块；扫描列出 `~/.claude/skills` 下的技能；勾选导入后复制进 Kivio 用户技能目录并出现在「已安装」。
- [ ] 三个配置文件任一缺失时，对应分组安静地不出现，其余正常。
- [ ] `npm run lint` / `npm run typecheck` / 相关 `cargo test` 通过。

## Notes

- 机制定稿：扫描 + 手动勾选 + 复制（区别于 Cursor 的「全量实时软链、全有或全无」——本方案粒度更细、导入后独立可编辑；代价是不跟上游自动更新，可重导）。
- 简化（ponytail）：Claude MCP 只读顶层 `mcpServers`，跳过 `projects[path].mcpServers`；OpenCode 按纯 JSON 解析（暂不处理 .jsonc 注释）；不做重名去重（用户自己看着勾）。
