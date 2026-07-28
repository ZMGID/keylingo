# 设计：对话生命周期 Hooks

## 核心决策：调度器放 Rust，不放前端

LiveAgent 的 Hook 由前端 TS 驱动（`hookRunner.ts` 维护执行链，通过 `invoke` 调 Rust 执行脚本，再用 `scope_id` 反向取消）。**Kivio 不照抄**：Kivio 的 agent loop 完全在 Rust（`chat/agent/loop_.rs`），生命周期事件天然发生在 Rust 侧。把调度搬到前端意味着为每个事件加一条 Tauri 事件 + 一次 IPC 往返 + 一套跨进程 scope 取消注册表——纯粹是为了迁就参考实现的架构。

因此：**`chat/hooks.rs` 一个模块，端到端在 Rust**。前端只负责配置 UI 和展示警告。

## 数据模型

存在 `settings.chat_tools.hooks: Vec<HookDef>`（复用现有 `settings.json` 持久化 + `sanitize_settings` + 前端保存链路，不新建 store）。

```rust
pub struct HookDef {
    pub id: String,
    pub name: String,
    pub description: String,
    pub event: String,          // 8 个事件之一，未知值在 sanitize 时丢弃
    pub enabled: bool,
    #[serde(rename = "type")]
    pub kind: String,           // "command" | "http"
    pub script: String,         // command 用
    pub url: String,            // http 用
    pub method: String,         // http 用，默认 POST
    pub headers: BTreeMap<String, String>,
    pub timeout_ms: u64,        // 钳制 [1_000, 600_000]，默认 60_000
}
```

**不抄的部分**：`revision` + `AutomationOp` apply/conflict 机制（Kivio settings 是单写者，设置页整体保存，没有并发冲突）；`requests: Vec<HttpRequestSpec>`（一个 Hook 一个请求够用，多请求让用户建多个 Hook）；`MASKED_HEADER_VALUE`（Kivio 设置不外传到 gateway/web 端）。

## 执行器 `chat/hooks.rs`

```rust
pub struct HookDispatcher { /* 内部 */ }

impl HookDispatcher {
    /// 无启用 Hook 时返回 None —— 调用侧零开销（不构造载荷、不 spawn）。
    pub fn new(app: AppHandle, hooks: &[HookDef], conversation_id, run_id, cwd) -> Option<Self>;
    pub fn dispatch(&self, event: HookEvent, tool_name: Option<&str>, round: Option<u32>);
    pub fn cancel(&self);
}
```

- 构造时按 event 分组，只留 `enabled` 的；**空则返回 `None`**（验收 6）。
- 内部一个 `tokio::sync::mpsc::UnboundedSender` + 一个 detached worker task。worker 串行消费，保证事件顺序（对齐 LiveAgent 的 `executionChain`）。
- `dispatch` 只是 `send`，永不阻塞 loop。队列上限 `MAX_QUEUED = 64`，溢出丢弃并只警告一次。
- `cancel()`：置 `AtomicBool` + `kill_process_group(pid)` 杀在跑的脚本（复用 `native_tools::shell::kill_process_group`）。worker 见到 flag 即排空退出。
- Drop：关闭 sender，worker 排空剩余事件后自然退出（`agent_end` 不能被 Drop 吞掉）。

### command 执行

复用 `native_tools::shell::build_shell_command`（现为私有，改 `pub(crate)`）拿到平台 shell，自己接 stdio：载荷 JSON 写 stdin，`KIVIO_*` env 注入，超时用 `tokio::time::timeout`，超时/失败调 `kill_process_group`。cwd = 会话工作目录（`resolve_conversation_working_directory`），解析不出则用临时目录（不像 LiveAgent 那样直接报错——Kivio 无项目会话是常态）。

### http 执行

`api::build_http_client()`，body = 载荷 JSON（用户未设自定义 body 时），加 `X-Kivio-Hook-Event` 头。2xx/3xx 视为成功。

### 失败上报

worker 直接 `app.emit("chat-hook", { conversationId, runId, hookName, event, message })`。前端在 `Chat.tsx` 监听，渲染成一条非阻断的警告条（复用现有错误条样式）。不写进消息体、不进 storage。

## 触发点接线

`AgentHost` 加一个默认返回 `None` 的方法：

```rust
fn hooks(&self) -> Option<&HookDispatcher> { None }
```

只有 `ChatAgentHost` 覆写它。**sub_agent / probe / loop_tests 的 host 一行都不用改**（这是选 host 方法而非 `AgentRunConfig` 字段的理由：后者要改 ~12 处字面量构造）。

`loop_.rs` 内取一次 `let hooks = host.hooks();`，各触发点 `if let Some(h) = hooks { h.dispatch(...) }`：

| 事件 | 位置 |
|------|------|
| `agent_start` | `run_agent_loop` 开头（`RunState` 构造后） |
| `turn_start` + `message_start` | `loop {}` 内、`planning_step` 之前（对齐 LiveAgent `startTurn` 同时发两个） |
| `message_end` | `planning_step` 返回后 |
| `turn_end` | `run_tool_round` 返回 `Continue` 后；`FinalAnswer`/`RoundLimit` break 前 |
| `tool_execution_start` / `_end` | `rounds.rs::execute_tool_call_result` 前后（单点，串行与并行路径都经过它） |
| `agent_end` | RAII guard（`HookRunGuard`）的 `Drop` —— loop 有 8 条 return 路径，逐条加等于漏一条 |

取消：`loop_.rs` 现有的取消分支（`is_generation_active` 为假 / `Cancelled(result)`）额外调 `hooks.cancel()`。

## 前端

- `src/settings/tabs/HooksTab.tsx` —— 左侧生命周期导轨（AGENT/TURN/MESSAGE/TOOL 四阶段分组、可折叠、事件点标 Hook 数）+ 右侧选中事件的 Hook 卡片列表。布局照参考截图，但用 Kivio 的 `kv-panel` / `Button` / `IconButton` / `Toggle`，不引入新 UI 原语。
- `HookModal`：名称 / 描述 / 类型分段（command|http）/ 脚本 textarea 或 URL+方法+headers / 超时。
- `SettingsShell.tsx`：`navItems` 加 `{ id: 'hooks', … }` + `pageMeta` 一条 + 一个新图标。
- `api/tauri.ts`：`HookDef` 类型 + `chat-hook` 事件监听器 `onChatHook`。
- i18n：`settings/i18n.ts` 加中英文串。

## 兼容与回滚

- 纯新增字段，旧 `settings.json` 缺 `hooks` → serde default 空数组，行为与现状字节一致。
- 未配置 Hook 时 `HookDispatcher::new` 返回 `None`，loop 里只多一次 `Option` 判空。
- 回滚 = 撤销分支，无数据迁移。
