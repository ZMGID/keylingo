# Implement — MCP 预热连接与工具快照落盘

## 顺序 checklist

### P2 先做（P1 的超时兜底依赖落盘快照才完整）
1. [ ] `state.rs`：快照落盘
   - 定义落盘结构（version=1，`{server_id: {config_fingerprint, tools, saved_at}}`）与文件路径 `{app_data}/mcp_tool_snapshots.json`。
   - `remember_mcp_tool_snapshot` 更新内存后 spawn_blocking 原子写盘（tmp+rename，失败仅 eprintln）。
   - AppState 初始化（或首次访问）从磁盘灌入内存 map；损坏/版本不符则忽略。
   - 检查 `config_fingerprint` 是否含敏感明文（headers/token）——若含，落盘前对指纹 SHA-256（内存中同样存哈希，保证比较一致）。
2. [ ] 单测（`manager.rs`/`state.rs` tests）：写盘→清内存→重灌→`mcp_cached_tools` 命中；指纹变化不命中；损坏 JSON 安全忽略。

### P1 预热 + 首轮不死等
3. [ ] `mcp/registry.rs`：`#[tauri::command] chat_mcp_warmup(app, state, server_ids: Option<Vec<String>>)`——对 None=全部 eligible / Some=指定 id 的 servers 各 spawn `mcp_list_tools`（fire-and-forget，立即返回 Ok）。在 `lib.rs` invoke_handler 注册。
4. [ ] `collect_enabled_mcp_tool_defs`：每 server listing 包 3s `tokio::time::timeout`（listing spawn 成 task，timeout 只放弃等待不取消任务）；超时与 Err 同路走 `mcp_cached_tools` 兜底 / unavailable。常量 `WARM_TOOL_LIST_TIMEOUT`。
5. [ ] 单测（loop/registry tests）：一快一慢（>3s）两个 fake server——收集在 ~3s 内返回、快的在列、慢的 unavailable（无快照时）或用快照（有快照时）。
6. [ ] `src/api/tauri.ts`：`chatMcpWarmup(serverIds?: string[])`。
7. [ ] `McpCenter.tsx`：启用 Toggle 打开时追加 `void api.chatMcpWarmup([server.id])`。
8. [ ] `Chat.tsx`：挂载初始化 effect 里 `void api.chatMcpWarmup()` 一次。

## 校验命令
- `cargo test --manifest-path src-tauri/Cargo.toml --lib mcp`
- `npm run lint` && `npm run typecheck`
- 手动冒烟：
  1. MCP 页开启一个 server 开关 → 数秒内状态点变绿（或红+错误）。
  2. 重启 app → 不发消息先看首轮对话工具即用（快照）；调工具透明连接成功。
  3. 配一个坏 server（不存在的命令）+ 一个好 server → 首轮 ~3s 内开始，好 server 工具可用。

## 回滚点
- P2 独立可回滚（删落盘读写，内存行为回到现状）。
- P1 三个子件（warmup 命令 / timeout 包裹 / 前端两处调用）各自独立可回滚。
