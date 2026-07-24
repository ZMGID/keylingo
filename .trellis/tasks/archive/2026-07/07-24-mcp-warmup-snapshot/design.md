# Design — MCP 预热连接与工具快照落盘

## 总体

不新建子系统。全部改动落在现有 `mcp/manager.rs`、`mcp/registry.rs`、`state.rs`、少量 `commands.rs`/前端两处调用点。模式对齐 external_agents 检测缓存（缓存 + 单飞 + 后台刷新）。

## P1 预热连接

### 新命令 `chat_mcp_warmup(server_ids: Option<Vec<String>>)`
- 位置：`mcp/registry.rs`。
- 语义：对指定 servers（None = 全部 `eligible_mcp_servers`）逐个 `tauri::async_runtime::spawn` 调 `state.mcp_list_tools(app, server)`（它内部走 `mcp_get_or_connect` → 握手 → tools/list → `remember_mcp_tools`，并 emit 状态事件）。fire-and-forget：命令立即返回 `Ok(())`，结果全靠 `onMcpServerState` 推送呈现。
- 并发安全：池单飞门闩已保证同 server 并发只握手一次；与手动测试/对话触发互不冲突。
- 失败：单个 server 失败只 emit Error 状态，不返回错误。

### 触发点（前端两处，各一行级改动）
1. **启用即连**：`McpCenter.tsx` 的启用 Toggle `onChange(enabled=true)` 时，`updateServer` 后追加 `void api.chatMcpWarmup([server.id])`。CLI 导入/市场安装默认 `enabled:false`，用户开开关时自然触发，无需在导入路径重复埋点。
2. **开窗预热**：`Chat.tsx` 挂载后（现有初始化 effect 里）`void api.chatMcpWarmup()` 一次。防抖：命令本身幂等（池命中即返回），不需要前端记状态。

### 首轮不死等（`collect_enabled_mcp_tool_defs` 改造）
现状：`join_all` 等所有 server 的 `mcp_list_tools`，慢/坏 server 拖全场直到其内部超时。
改法（保持函数签名不变）：
- 每个 server 的 listing future 外面包 `tokio::time::timeout(WARM_TOOL_LIST_TIMEOUT, …)`，`WARM_TOOL_LIST_TIMEOUT = 3s` 常量。
- 超时分支与 Err 分支同路：先 `mcp_cached_tools`（内存/落盘快照）兜底；无快照 → `unavailable`。
- 超时后**不取消**底层连接：把 listing spawn 成独立 task 再 timeout 其 JoinHandle 的等待，任务本体继续跑完（连上后 `remember_mcp_tools` 写缓存 + emit Connected），下一轮自然可用。
- 已 Connected 的 server 走池命中，微秒级返回，不受 3s 影响。

## P2 工具快照落盘

### 存储
- 文件：`{app_data}/mcp_tool_snapshots.json`（单文件，全部 server 一起存；量小，无需分文件/DB）。
- 结构：`{ version: 1, snapshots: { [server_id]: { config_fingerprint, tools: [McpTool…], saved_at } } }`。
- **不含任何敏感信息**：`McpTool` 仅 name/description/schema/annotations；fingerprint 是哈希前配置的摘要字符串——确认 `config_fingerprint` 现实现若含 header/token 明文则改为对其做 SHA-256 再落盘（落盘的指纹只用于相等比较，哈希不影响语义）。

### 读写路径（全部在 `state.rs` 现有快照 API 内收敛）
- `remember_mcp_tool_snapshot`（现 `remember_mcp_tools` 底层）：更新内存 map 后，spawn_blocking 序列化写盘（整文件原子写：tmp + rename）。写盘失败仅 eprintln。
- 启动：`AppState` 构造（或首次访问时 lazy）从磁盘加载进内存 map。加载失败/版本不符 → 忽略文件（视为无缓存）。
- `get_mcp_tool_snapshot` 不变（内存查询；启动已灌入落盘数据）。
- 作废：现有指纹比较天然作废改配置的旧快照；server 被删除时（settings 保存路径）可顺手清理对应条目——低优先，首版可不做（脏条目无害，查不中）。

## 数据流（改后）

```
启用开关/开聊天窗 ──chat_mcp_warmup──▶ spawn× N ──▶ mcp_get_or_connect(单飞) ─▶ tools/list
                                                        │ emit_server_state(前端状态点)
                                                        └ remember_mcp_tools ─▶ 内存map + 落盘JSON

首轮工具收集 ──▶ per-server timeout(3s) ┬ 快照命中(内存,含启动时灌入的落盘) → 立即返回
                                        ├ 已连接 → 池命中即回
                                        └ 超时/失败 → 快照兜底 or unavailable；连接后台续跑
```

## 兼容性 / 回滚

- 纯增强：无 settings schema 变化、无事件 payload 变化、`collect_enabled_mcp_tool_defs` 签名不变。
- 回滚 = 删 warmup 命令与两处前端调用、去掉 timeout 包裹、删落盘读写（内存缓存行为回到现状）。
- 落盘文件损坏/缺失 = 视为无缓存，行为等同现状。

## 取舍

- 预热用「连接 + tools/list」而非只 TCP/spawn：一步到位填充快照与状态，多一次 list 开销可忽略。
- 单 JSON 文件而非 per-server 文件/SQLite：快照总量 KB～百 KB 级，整文件原子写足够。
- 3s 常量而非新增设置项：先不给用户新旋钮；不够再暴露（ponytail）。
- 超时不取消底层任务：让慢 server 自己连完并写缓存，代价是短暂的后台任务存活，可接受。
