# 执行计划：对话生命周期 Hooks

分支：`feat/hooks`（已创建）

## 步骤

### 1. Rust：settings 数据模型
- [ ] `settings.rs`：`HookDef` struct + `ChatToolsConfig.hooks: Vec<HookDef>`（`#[serde(default)]`）
- [ ] `sanitize_settings`：丢弃 event/kind 非法的条目，钳制 `timeout_ms` 到 `[1_000, 600_000]`，补空 id
- 验证：`cargo test --manifest-path src-tauri/Cargo.toml settings`

### 2. Rust：`chat/hooks.rs` 执行器
- [ ] `HookEvent` enum（8 值）+ `as_str` / `from_str`
- [ ] 载荷构造 `HookPayload` → JSON
- [ ] `HookDispatcher::new` / `dispatch` / `cancel` + worker task（串行、队列上限 64、溢出一次性警告）
- [ ] command 分支：`build_shell_command`（改 `pub(crate)`）+ stdin JSON + `KIVIO_*` env + timeout + `kill_process_group`
- [ ] http 分支：`api::build_http_client()` + POST JSON + `X-Kivio-Hook-Event`
- [ ] 失败 `app.emit("chat-hook", …)`
- [ ] 单测：脚本收到 stdin JSON 和 env；exit != 0 上报失败；cancel 后不再执行；无启用 Hook 时 `new` 返回 `None`
- 验证：`cargo test --manifest-path src-tauri/Cargo.toml hooks`

### 3. Rust：agent loop 接线
- [ ] `AgentHost::hooks()` 默认 `None`；`ChatAgentHost` 覆写
- [ ] `reply.rs`：构造 `HookDispatcher`（读 settings、解析会话 workdir）塞进 `ChatAgentHost`
- [ ] `loop_.rs`：`agent_start` / `turn_start` / `message_start` / `message_end` / `turn_end`；`HookRunGuard` 在 Drop 发 `agent_end`；取消分支调 `cancel()`
- [ ] `rounds.rs`：`execute_tool_call_result` 前后发 `tool_execution_start` / `_end`
- 验证：`cargo test --manifest-path src-tauri/Cargo.toml`（`loop_tests` 必须全绿——默认 host 不实现 `hooks()`，应零影响）

### 4. 前端：类型与事件
- [ ] `api/tauri.ts`：`HookDef` 类型、`ChatToolsConfig.hooks`、`onChatHook` 监听器
- [ ] `Chat.tsx`：监听 `chat-hook`，渲染非阻断警告条

### 5. 前端：设置页
- [ ] `settings/tabs/HooksTab.tsx`：生命周期导轨 + Hook 列表 + 增删改启停
- [ ] `settings/HookModal.tsx`：编辑弹窗
- [ ] `SettingsShell.tsx`：`navItems` / `pageMeta` / 图标接入
- [ ] `settings/i18n.ts`：中英文串
- [ ] `HooksTab.test.tsx`：渲染 + 事件切换 + 保存回调
- 验证：`npm run lint && npm run typecheck && npm test`

### 6. 手动冒烟（验收 2–5）
- [ ] `agent_end` command Hook 写日志文件 → 发一条消息 → 文件被写入
- [ ] `tool_execution_start` Hook `cat > /tmp/kivio-hook-payload.json` → 触发工具调用 → 载荷含 `toolName`
- [ ] 故意 `exit 1` 的 Hook → 对话正常完成 + 出现警告条
- [ ] 长跑脚本（`sleep 60`）+ 对话中途「停止」→ 进程被杀

### 7. 收尾
- [ ] `trellis-update-spec`：把 Hook 契约（事件集 / 载荷 / 「调度在 Rust」红线 / 零 Hook 零开销）写进 `.trellis/spec/guides/`
- [ ] 提交

## 回滚点

每步独立可编译；步骤 3 是唯一触碰现有热路径的改动，若 `loop_tests` 出现回归，单独 revert 步骤 3 即可保留 1/2（纯新增，无副作用）。

## 已知风险

- **`agent_end` 的 Drop 语义**：worker 是 detached task，`HookDispatcher` Drop 时若立即取消，最后的 `agent_end` 会丢。实现须让 Drop 只关 sender、由 worker 排空后退出。
- **并行工具调用的事件顺序**：`execute_parallel_chunk` 里多个工具并发，`tool_execution_start/_end` 会交错。这是真实语义，脚本靠 `toolName` 自己配对，不做重排。
