# MCP 预热连接与工具快照落盘

## Goal

解决 MCP 懒加载的两个用户痛点：
1. 启用/导入 MCP 后状态一直显示「未连接」，用户无法立刻知道该 server 是否正常。
2. 首轮对话要现场连接所有启用的 MCP 才能拿到工具清单——server 多或有一个连不上时首轮被拖慢到超时；重启后内存缓存全冷，问题重现。

核心思路（业界通行 + 与本项目 external_agents 检测缓存同模式）：**把「工具清单」和「实时连接」解耦**——清单走缓存（落盘），连接走后台预热 + 状态推送。

## Requirements

### P1 预热连接 + 状态可见
- **启用即连**：在 MCP 页打开某 server 的启用开关（含从市场安装、CLI 导入后启用、mcp.json 导入后启用）→ 后台立即发起一次连接（fire-and-forget，不阻塞 UI）；状态点走「连接中 → 已连接/错误」推送（复用现有 `emit_server_state` / `onMcpServerState`）。用户由此立刻看到该 server 是否正常。
- **开窗预热**：打开聊天窗口后，后台把所有 enabled 的 MCP server 预热连接（并行、不阻塞 UI；连接池已有单飞门闩，重复触发无害）。
- **首轮不死等**：agent 收集工具表时，对**未连上且无缓存快照**的 server 不无限等待——给现场连接一个短超时（默认 3s，可沿用/派生现有超时配置）；超时的 server 本轮跳过并列入 `unavailable_mcp_servers`（现有机制），连接在后台继续，下一轮自然可用。已有缓存快照的 server 直接用快照（现有降级路径），不等连接。

### P2 工具快照落盘
- 把内存中的 `mcp_tool_snapshots`（server_id + config_fingerprint → tools）持久化到磁盘（app_data 下，JSON 即可）；写入时机：每次成功 `tools/list` 后（现有 `remember_mcp_tools` 处）。
- 启动时加载落盘快照进内存缓存。重启后首轮工具收集直接命中缓存 → 秒出工具清单，不需要任何 server 已连接。
- 指纹不匹配（用户改了该 server 配置）的落盘快照按现有语义作废。
- 快照仅是清单缓存：真正调用某工具仍走连接池实连（届时多半已被 P1 预热就绪）。

### 不做（backlog，记录不实现）
- P3：每 server 生命周期策略（lazy/eager/keep-alive）配置项；搜索式工具懒加载（tool schema 不全量进上下文）。

## Acceptance Criteria

- [ ] MCP 页打开某 server 启用开关后，无需点「测试连接」或发起对话，状态点在数秒内自动变为「已连接」（或「错误」+ 可见原因）。
- [ ] 打开聊天窗口后台预热：稍候片刻再发首条消息，工具收集不再现场握手（可由日志/耗时观察）。
- [ ] 构造一个连不上的 server（如指向不存在的命令）+ 一个正常 server：首轮对话在短超时内开始，正常 server 工具可用，坏 server 进入 unavailable 提示，不把首轮拖到全局超时。
- [ ] 重启 app 后（不重连任何 server）首轮对话：曾成功连接过的 server 的工具立即出现在工具表（来自落盘快照）；调用该工具时透明实连成功。
- [ ] 修改某 server 配置（如换命令）后，其旧落盘快照不再被使用。
- [ ] `cargo test`（mcp 模块）通过；新增行为有单测（快照落盘/加载/指纹作废、预热触发）。
- [ ] `npm run lint` / `npm run typecheck` 通过。

## Notes

- 与 `.trellis/tasks/07-19-external-cli-detection-cache`（external_agents 检测缓存）同一模式：缓存 + 单飞 + 后台刷新；实现时参考其结构。
- 现有可复用件：连接池单飞（`mcp_get_or_connect_inner`）、状态推送（`emit_server_state`）、内存快照（`AppState.mcp_tool_snapshots` + `remember_mcp_tools` + `mcp_cached_tools`）、失败降级到 last-known schema（`collect_enabled_mcp_tool_defs`）、`chat_mcp_reload_server`。
- 风险点：预热与用户手动「测试连接」/ OAuth 刷新并发——池单飞已覆盖；落盘快照含工具 schema 不含任何密钥，无敏感信息落盘问题（headers/auth 不入快照）。
