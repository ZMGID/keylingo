# 对话生命周期 Hooks 执行契约

> **适用**：改动 `src-tauri/src/chat/hooks.rs`、`agent/loop_.rs` / `rounds.rs` 的 Hook 派发点、`settings::HookDef`，或前端 `settings/tabs/HooksTab.tsx` / `HookModal.tsx` 时**必读**。

## 红线一：调度端到端在 Rust，前端不参与执行

参考实现（LiveAgent `src/lib/automation/hookRunner.ts`）把执行链放在前端 TS，用 `invoke` 反向调 Rust 跑脚本、再用 `scope_id` 跨进程取消。**Kivio 不这样做**：生命周期事件天然发生在 `chat/agent/loop_.rs`，把调度搬到前端等于为每个事件加一条 Tauri 事件 + 一次 IPC 往返 + 一套跨进程 scope 注册表。

前端职责只有两件：**配置 UI**，和**监听 `chat-hook` 展示失败警告**。不要给 Hook 加任何「前端驱动执行」的通道。

## 红线二：Hook 一律 fire-and-forget

不能拒绝工具调用、不能改写工具参数、不能强制模型继续（Claude Code 的 `permissionDecision: deny` / `decision: block`、OpenCode 的可变 `output.args` 都**不实现**）。失败只 `emit("chat-hook", …)`，绝不打断对话。

要加阻断语义就是新任务，得先设计 decision schema 与权限系统的对接，不能顺手在 `run_job` 里返回个 bool 了事。

## 红线三：零 Hook 必须零开销

`HookDispatcher::new` 在没有「已启用 + 事件名合法」的条目时返回 `None`；`AgentHost::hooks()` 默认返回 `None`。调用侧（`reply.rs`）先用 `HookDispatcher::any_enabled(&hooks)` 短路，**在构造入参之前**——否则没配 Hook 的用户每轮都白分配几个 id / model 字符串。

loop 里一律 `if let Some(hooks) = hooks`，不要引入「空调度器」之类的 Null Object：那会让每个事件都走一遍队列判空。

## 事件配对由 RAII 保证，不许手写收尾

loop 有 8 条 return 路径，planning 有 7 个 outcome（含两条 retry `continue`、`DraftFailed`、`Recovered`，外加 `planning_step(..)?` 的错误传播）。**逐条 return 手写 `dispatch(TurnEnd)` 必定漏一条**，留下永不闭合的 `turn_start`。

因此：
- `HookRunGuard` 的 Drop 发 `agent_end` —— 无论成功 / 失败 / 取消 / `?` 早返回都恰好一次。
- `HookTurnGuard` 构造发 `turn_start` + `message_start`，Drop 发 `message_end`（若还欠着）+ `turn_end`。取工具调用那条路上用 `end_message()` 提前闭合消息，本轮留到工具跑完再落 `turn_end`。
- **无工具会话**（工具全关 / provider 不支持）整段跳过工具循环，靠 `(!tool_loop_ran).then(|| HookTurnGuard::new(hooks, 1))` 在 synthesis 前补一「轮」，否则只剩 `agent_start`/`agent_end`。

新增派发点前先问：这条路径上的 start 由谁配 end？答不出来就该挂 guard 而不是加一行 `dispatch`。

回归守卫在 `loop_tests.rs`：`assert_hook_events_well_formed` + 三条路径（工具轮 / 无工具 / 取消）。改动派发逻辑必须让它们继续绿。`HookDispatcher::recording()` 是「只记流水、不真执行」的测试构造器 —— 断言的是 loop 的**派发顺序**，不是执行调度。

## 取消是世代（epoch），不是闭锁（latch）

`cancel()` 递增 `AtomicU64` epoch；worker 见到 epoch 比自己旧的 job 直接丢弃。**不能改成 `AtomicBool` 永久禁用**：`agent_end` 由 Drop guard 在取消**之后**派发，闭锁会把它吞掉——那样「停止对话」与「agent_end 总会触发」两条验收互斥。

两个必须保留的细节：
- 被 cancel 杀掉的脚本以失败返回，**世代已变则静默**，不要把它当成用户脚本的错误报上去。
- `spawn` 后登记 pid，登记完**必须复检世代**：cancel 可能恰好落在「worker 检查过世代」与「pid 登记」之间，那次 cancel 看到的是空槽位，于是刚起来的进程谁也不杀（脚本是 `sleep 600` 就泄漏到 run 结束之后）。`cancel_racing_the_spawn_does_not_leak_the_process` 锁住这一点。

## 工作目录：解析不出就落临时目录，不 mkdir

会话工作目录是**懒创建**的（只有原生工具真的用到才 mkdir），所以一个从没用过工具的普通会话，`resolve_conversation_working_directory` 给出的路径可能根本不存在。直接拿去当 `current_dir` 会让 spawn 以 ENOENT 失败，用户看到「hook 启动失败」而非自己脚本的输出。

`with_sink` 里 `if !ctx.cwd.is_dir() { ctx.cwd = temp_dir() }`。**不要改成 mkdir**：不能因为配了一个 Hook 就在工作区根下凭空生出空目录。

## 载荷：stdin JSON + `KIVIO_*` env，两者都要

stdin 一行 JSON 是 Claude Code / Codex 的事实标准；`KIVIO_*` env 是便于一行脚本使用的补充（LiveAgent 只有 env）。两者都发，字段见 `HookPayload`（camelCase）。

HTTP Hook 用同一 JSON 作 body + `X-Kivio-Hook-Event` 头。`HookDef` **没有**自定义 body 字段——要别的形状就用 command 类 Hook 加一行 curl。

## 并行工具的事件会交错

`execute_parallel_chunk` 里多个工具并发，`tool_execution_start`/`_end` 必然交错。**这是真实语义，不做重排**：脚本靠载荷里的 `toolName` 自己配对。派发点在 `rounds.rs::execute_tool_call_result` 单点（串行与并行两条路都经过它），所以数量一定相等。

## 数据模型：复用 settings，不建第二套存储

`HookDef` 存在 `settings.chat_tools.hooks`，走既有 `settings.json` 持久化 + `sanitize_settings` + 前端整体保存链路。**不抄** LiveAgent 的 `revision` + `AutomationOp` apply/conflict 机制：Kivio settings 是单写者，设置页整体保存，没有并发冲突要解。

`sanitize_hooks` 是防线：事件名 / kind 非法直接丢弃条目（没有合理的「就近」事件可猜），空脚本 / 空 URL 丢弃，补 id，钳制 `timeout_ms` 到 `[1_000, 600_000]`。`chat::hooks::HookEvent::parse` 是事件名的唯一事实源，settings 与执行器共用它——不要在任何一侧另抄一张字符串表。
