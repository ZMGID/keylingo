# External CLI Agents — 执行契约与约定

> 来源：任务 07-20-external-cli-overhaul（子任务 ce76f60 / 4214956 / 3487e05 / 3456997 的审计与修复）。
> 适用：`src-tauri/src/external_agents/**` 及其前端对接面（`src/chat/RuntimePicker.tsx`、`src/chat/api.ts`、`src/chat/Chat.tsx` 的运行时切换）。

## 消息链路（prompt.rs / session/acp.rs）

1. **只发最新消息，历史归 CLI 原生会话**（3456997 起）：`compose_external_prompt` 不重放 transcript。全部 9 个 CLI 均有原生会话（claude `--resume` / codex thread / ACP `session/load` 含 kimi / pi `--session-id`）。禁止任何形式的历史重放回归；fresh 重连丢上下文时必须发可见提示（TextDelta blockquote，cancelled 不发）。
   - **claude 的会话级系统指令走启动 flag，不进正文**（07-29 起）：`--append-system-prompt-file <path>`（**隐藏 flag**，`--help` 里没有；claude 2.1.220 零副作用探针确认存在——不给值报 `option '--append-system-prompt-file <file>' argument missing`，胡编的 flag 报 `unknown option`）。语义是**追加**到 claude 原生系统提示之后，不替换。改动前塞进 prompt 正文的那条消息会被 CLI 自己的上下文压缩摘要掉甚至丢弃，而 `skip_instructions`（内容没变就不重发）保证了**永远不补发** ⇒ 长会话跑一阵子后用户配置的系统提示与 Memory 静默失效，无任何可观测信号。启动 flag 每次进程启动都重新注入，与对话历史无关。**必须用 file 而非内联字符串**：Windows 命令行 32767 字符上限，含 Memory 块可能超；npm 装的用户拿到 `claude.cmd`，长参数还有批处理转义风险。文件按 conversation_id 覆写在 temp（`kivio-extsys-<id>.md`，进 `cleanup_orphan_temp_files` 的启动 GC）。判据是纯函数 `prompt::instructions_via_launch_flag`；**其余 8 个 CLI 仍是正文注入**（没有核实过的等价 flag —— audit N5 记着 pi 曾把目录塞进 `--append-system-prompt`），给任何 CLI 加这条路之前先按第 12 条核实语义。真机验收：`live_append_system_prompt_file_still_applies_on_the_second_turn`（第二轮仍能取回注入的哨兵串）。
     - **B1 常驻之后这个文件只在进程启动时被读一次**（它仍每轮覆写，但进程不再每轮重启）⇒ 中途改系统提示 / Memory **不重连就静默失效**。这条由第 24 条的 `LaunchConfig` 指纹（含 instructions 哈希）负责触发重连，别在正文里补发。
   - **claude 的 per-turn 正文每轮整份发送**（B1 起）：常驻会话的「复用轮」默认只发最新用户消息（历史归 CLI 原生会话），但 claude 的 `composed.full_prompt` 里**没有**会话级指令（它们走启动 flag），剩下的全是 per-turn 内容 —— active skill 正文 + 降级附件说明 + 用户消息。判据是纯函数 `run::persistent_turn_prompt`；改成「只发最新消息」会让 skill 正文与附件说明从第 2 轮起静默消失。
   - **skill 正文永远留在 prompt 正文里**，不受 `skip_instructions` 抑制：active skill 是 **per-turn** 的选择（用户可中途换 skill），跟会话级常量一起被抑制的话，resume 轮新激活的 skill 正文根本发不出去。
2. **禁止全局文本前缀去重**：ACP assistant/thought 输出的去重走 `AcpTextAssembler`（按消息边界维护累积游标，`on_boundary` 只置位、`push_chunk` 的 starts_with 决定是否重置）。一次性驱动 `run_acp_session` 与持久驱动 `AcpSession::run_turn` 必须共用 `acp_apply_session_update`——不要再出现两份拷贝。同理：CLI 自压的边界记录必须复用 `chat::types::CompactionBoundaryRecord`（内置压缩路径同一个类型），不要手写一份 json —— 两份形状迟早分叉。
3. **流 parser 的 per-message 状态**：类似 `text_streamed` 的"已流式"标志必须在新消息开始（message_start / 新 message id）时复位，不能是整轮全局 bool。**per-turn 状态是另一码事，不要复用同一个字段**：`ClaudeStreamState::any_text_emitted`（本轮有没有给过用户任何正文，A4 的 `result.result` 兜底靠它）在 `result` 帧复位；`completed_result_turns` 是 per-session 计数，从不复位。只有真正的正文置位 `any_text_emitted`，系统提示（压缩中 / 权限被拒 / 任务失败）不算——否则一条「正在压缩」提示就能把 `/cost` 的报告吞掉。
3b. **会话-CLI 绑定**：有消息的外部会话禁切 kind/external_agent_id（后端 `check_runtime_switch_allowed` 纯函数 + 前端 locked）；model/reasoning/sandbox 放行。前端任何"回写运行时"的路径（如 draft 落地）必须与后端放行条件一致——只对空会话生效，否则被校验拒绝卡死发送。

## 会话生命周期（session/*、run.rs、errors.rs）

4. **持久会话必须排空 stderr**：任何 `Stdio::piped()` 的长活子进程都要 `spawn_stderr_tail`（环形 8KB），close/错误路径 join 取尾部。不排空会因管道写满挂死子进程。
5. **错误出口统一走 `errors::classify`**：气泡主文案 = 分类后的可操作中文（Auth 附 per-agent 登录命令表），原始错误 + 退出码 + stderr 尾部折叠进 `<details>`。新增错误路径不得把裸协议错误串直接落气泡。"401" 等数字判据必须 token 边界匹配。
6. **重试策略是纯函数**：`persistent_failure_action` —— cancelled 保留 handle；Auth 永不自动重试；transient 失败 fresh 重连恰好一次；NEEDS_RECONNECT（launch-flag 配置变更）重连恰好一次。改动重连逻辑先改这个纯函数和它的单测。
   - **取消是否保留「活会话」是 per-protocol 的**（B1 起，`run::cancel_keeps_live_session`）：claude 的 `run_turn` 发出 `interrupt` 后**一直读到本轮的 `result` 才返回**，流位置回到轮次边界、进程完好 ⇒ 注册表条目**保留**，下一轮直接复用（否则点一次「停止」= 丢掉整个会话上下文 + 再花 3.2s 冷启动，常驻白做）。ACP / codex 发出 `session/cancel` / `turn/interrupt` 后立刻返回，reader 停在流中间，复用会读到残帧 ⇒ 保持原行为（丢弃条目，下一轮从落盘 handle 原生 resume）。
   - **取消有两种**：`"cancelled"`（会话仍可用）与 `session::live::CANCELLED_SESSION_LOST`（进程已死 / 协议级取消超时被硬 `Close`）。两者在出口都按取消呈现（判据 `run::is_cancellation`：不弹错误气泡、不发上下文重置提示、**不重发本轮 prompt**），但后者一律丢弃注册表条目 —— 留着就是个死 actor，下一轮才发现。
7. **handshake 错误带阶段前缀**（`spawn:`/`initialize:`/`session-new:`/`claude-init:`），超时常量 30s 起步，集中在文件顶部。
8. **中途换模型**：ACP 会话轮前 `session/set_config_option`/`set_model`（best-effort ack）；无对应 config 项的（reasoning 为启动 flag，如 grok）走 NEEDS_RECONNECT 重连。UI 所见必须与会话实际配置一致。
   - **claude 走 `LaunchConfig` 指纹**（B1 起，见第 24 条）而不是 NEEDS_RECONNECT：它的 model / effort / permission-mode / 系统指令**全是启动参数**，会话内无从切换。指纹在**轮前**比对（注册表复用判据），不匹配就换进程并原生 `--resume` ⇒ 新 flag 生效且上下文不丢；NEEDS_RECONNECT 那条路是**轮后**失败重连、走 fresh 会丢上下文，不适合这个场景。`SessionCommand::RunTurn` 携带的 model / reasoning 在 claude 的 `run_turn` 里**有意忽略** —— 不要为它加「会话内切换」的假实现。
   - **换模型不丢会话**（07-29 实测推翻旧结论）。此前 `resolve_agent_resume_context` 在「模型变了」时刻意生成一个**新** session id（`--session-id <new>`），依据是一句没有实测支撑的注释：「CLI 在 `--resume` 上会忽略 `--model`，会话钉死在旧模型」。**claude 2.1.220 本机实测推翻了它**：先 `--session-id X --model sonnet` 让它记住一个数字，再 `--resume X --model opus` 起来 —— 它**既报自己是 Opus、又记得那个数字**，切换生效且上下文没丢。所以换模型现在照常 `--resume`（`session/mod.rs`），进程侧由本条上一点的启动参数指纹接管（指纹不匹配 ⇒ 换进程，但带 `--resume <同一个 id>`）。**不要凭想象把「换模型 ⇒ 开新会话」加回来**：那是纯损失（用户换个模型就丢掉整段对话），要加回来先给出能证伪的实测。
   - **claude 的思考档位有两个不走 `--effort` 的取值**（07-29，均按第 12 条核实自 2.1.220，判据是纯函数 `defs::claude::claude_thinking_args`）：**关闭思考 = `--thinking disabled`**（**隐藏 flag**，`--help` 里没有；不给值报 `option '--thinking <mode>' argument missing`，二进制里的 commander 定义是 `.choices(["enabled","adaptive","disabled"]).hideHelp()`，官方 Agent SDK 的 `{type:"disabled"}` 也是拼成这个）—— 这一档**不叠 `--effort`**；**ultracode = `--effort ultracode`**，它**不是**独立 flag（`--ultracode` 不存在）而是 `--effort` 的一个取值（二进制里 `cUc={ultracode:"xhigh"}` 映射强度，`ultracode:_Jn(cli.effort)` 用同一字符串置位会话的 ultracode 编排状态；CLI 自己的 `/effort` 用法串就是 `<low|medium|high|xhigh|max|ultracode|auto>`）。**不要为 ultracode 引入 `--settings`**：那要落临时配置文件、还要论证不会污染用户 `~/.claude/settings.json` 的配置层，而 `--effort ultracode` 是 CLI 自己认的同义写法。真机验收：`live_thinking_off_really_stops_thinking`（同一 prompt，high 有 thinking 块 / off 为 0）、`live_ultracode_is_a_recognized_effort_value`（判据可证伪：胡编的 `--effort` 会在 stderr 打 `Warning: Unknown --effort value …`，`ultracode` 不打）。
   - **CLI 对超出模型能力的档位是静默降级，不报错**（同批实测）：未知 `--effort` 只打一句 stderr warning 后按默认档跑；`--thinking disabled` 撞上 catalog 里标着 `rejects_disabled_thinking` 的 Fable 5 时被直接忽略（仍产出 thinking 块、`is_error=false`）。所以**不要**为「哪个模型支持哪些档位」再建一张能力表 —— 门控纯属观感，代价是又一张会过期的表。
8b. **pi 轮次收尾（07-22 真机验收修复）**：`agent_end` 后进入 3s 宽限排空（到点主动 break，不无限等 EOF——pi 带 `--session-id` 收尾落盘时可能不因 stdin EOF 退出）；drain 返回 Ok 即 `start_kill` 子进程——pi 的会话落盘发生在每条 `message_end` 时（同步 `appendFileSync`），严格先于 `agent_end` 上线，kill 不丢会话、不影响下轮 resume。Unix 下信号退出 `status.code()=None`，不触发出口「非零退出+stderr」规则。

8c. **取消 / 出错时杀**整棵进程树**，且非零退出码必须由协议层完成标志豁免**（07-29 起，两件事必须一起做）。
   - CLI 会按用户配置把 MCP 服务器作为**自己的子进程**拉起来（claude 读 `~/.claude.json`）。只 `start_kill()` 杀的是直接子进程，每轮取消一次就漏一批孤儿 MCP 进程。统一走 `spawn::kill_agent_process_tree`，它复用 `native_tools::kill_process_group`（unix `killpg` SIGTERM→SIGKILL、Windows `taskkill /T /F`）**外加** `start_kill()`：`spawn_agent` 没有 `setsid`，unix 下 `killpg(-pid)` 命中不了非组长进程（无害 ESRCH），必须靠 `start_kill` 兜住直接子进程。**不要写第二份进程组 kill**（第 2 条）。
   - 8b 原先记的已知坑（Windows `TerminateProcess` 退出码恒为 1 ⇒ 「非零退出 + 有 stderr」误判为 error）在杀整树之后**中招面变大**。修法按当时给的方向落地：判据改为 `run::nonzero_exit_is_a_failure(exit_code, protocol_completed)` —— **读到协议层完成标志（claude 的 `result` 帧，经 `StreamHandler::saw_protocol_completion`）就一律豁免非零退出规则**，不再依赖退出码形态。真实的协议层失败（`result.is_error` 等）走 `resolve_turn_error` 那条独立出口，不受豁免影响。
   - **B1 之后这条规则在常驻路径上天然不触发**：持久会话不 `wait()` 子进程 ⇒ `exit_code` 恒为 `None` ⇒ 函数恒返回 false。进程的退出发生在**会话关闭**时（idle 回收 / LRU 淘汰 / 配置变更重连 / 应用退出），与任何单轮都无关，那条路上也没有气泡可污染 —— 所以「退出码归给哪一轮」这个问题不存在，**不要**为常驻路径重新引入它。目前只有 `PiRpc` 还走非持久分支，而它不上报完成标志（`protocol_completed` 恒 false），规则对它照旧生效；改动 pi / ACP / codex 的 kill 行为时要同步补上报。

## 一个会话一个常驻进程（claude）

> 来源：任务 B1（07-29）。全部协议事实为 claude 2.1.220 / Windows 本机实测，可运行探针在
> `session/claude_persist_probe_tests.rs`，实现在 `session/claude_stream.rs`。
> 收益实测：首轮到 `system/init` 约 **3.2s**，第 2+ 轮约 **0.1s**。

24. **claude 在 `-p --input-format stream-json --output-format stream-json` 下能常驻**，实现必须建立在下面这几条上，每条都改变了代码形状：
   - **吐完 `result` 后继续读 stdin**：同一进程连服多轮，上下文自然延续，`session_id` 恒定。
   - **进程只在 stdin 关闭时退出**（exit 0，约 0.5s）⇒ **关停 = 关 stdin 然后 `wait()`，不是 kill**。正常退出时 CLI 自己收尾（落盘会话、关掉它拉起的 MCP 子进程）；只有 `wait` 超时才升级到 `kill_agent_process_tree`（第 8c 条）。
     - **`shutdown()` 之后必须 `drop(stdin)`**：tokio 的 `ChildStdin::poll_shutdown` **只 flush、不关句柄**，句柄要到 drop 才关。少了这一行子进程永远收不到 EOF，每次关停都白等满超时再被杀掉 —— 症状只在耗时上（真机测试 53s → 169s，且测试目录删不掉因为 cwd 还被占着），功能层面完全看不出来。`acp.rs` / `codex_app_server.rs` 的 `close()` 是 `shutdown()` 紧跟 `start_kill()`，掩盖了同一个问题；改它们为优雅关停时要一起补。
   - **每轮恰好一个 `result`**（被中断的轮次也有）⇒ 它就是轮次边界信号。实现用 `StreamHandler::completed_result_turns()` 喂行前后各取一次来判定边界，**不要**为找边界把同一行 JSON 再解析一遍（第 2 条）。
   - **`system/init` 每轮都发，且在收到第一条 user 消息之前根本不发**（本机验证：不写 stdin 只能收到 `hook_started` / `hook_response`，没有 init）。所以 `connect` **不能**以 init 当握手信号 —— 那会死等。握手 = 只 spawn + 一次 `try_wait()`（抓住「参数非法 / `--resume` 的 id 不存在」这类立即退出，带上 stderr 尾部报 `claude-init:`）；其余失败（未登录等）是流里的一条 `result`，走第 5 / 19 条那条正常出口。附带坑见 14g：首轮 init 的 `tools` 是 28 项、第 2 轮起 35 项，**别从首轮 init 缓存工具/斜杠列表**。
   - **首轮还有 `hook_started` / `hook_response`**（用户自己配的 hook），第 2 轮起没有 ⇒ 解析器不得假定固定的开头帧序列。
   - **两轮之间 stdout 完全干净**（无空行 / keep_alive / 非 JSON 行）；**35 秒空闲不超时，不需要心跳**。
   - **stderr 全程零字节，但长活进程仍必须排空**：用 `spawn::spawn_stderr_tail`（环形 8KB），**绝不能用 `drain_stderr`** —— 它读到 EOF 才返回，长活进程下 `await` 会永久挂死（第 4 条）。句柄存成 `Option`，出错路径 `take()` 取尾部折进诊断，`close()` 收尾时 join。
   - **中断走 stdin 的 `control_request`**（init 的 `capabilities` 有 `interrupt_receipt_v1`）：`{"type":"control_request","request_id":"<唯一>","request":{"subtype":"interrupt"}}` → 回 `control_response` `subtype:"success"`，该轮仍吐一条 `result`（形态见第 20 条的豁免），**进程完好、下一轮正常返回**。取消**不 kill**，`run_turn` 发出 interrupt 后继续读到 `result` 才返回 —— 这样流位置回到轮次边界，会话可以直接复用（第 6 条）。

25. **会话 id 走启动参数，所以重连要改写 argv**。claude 的 session id 不像 codex/ACP 由握手 RPC 给，而是我们自己放进 `build_claude_args`（`--session-id` 首次 / `--resume` 续接）。**只能首次用 `--session-id`**：同一个 id 再来一次会被 claude 以「id 已存在」拒绝启动。重连必须用纯函数 `defs::claude::claude_args_resuming(args, id)` 把它换成 `--resume <同一个 id>`（连值一起摘，只摘 flag 会把裸 id 留成非法位置参数）；native id 用 `claude_session_id_from_args` 从 argv 读回来，第一轮再被 init / result 实报的 `session_id` 覆盖。
   - **`connect_persistent_session` 里参数优先于 live handle**，但**理由不是「换模型要开新会话」**（那条契约已被 07-29 的实测推翻，见第 8 条）。真正的理由：argv 里的会话 flag 是 `resolve_agent_resume_context` 为**本轮**做出的决定 —— 有落盘 handle 就 `--resume <那个 id>`，全新会话就 `--session-id <新 id>`；而 live handle 的 native id 只是「上一个进程最后报的那个」。用它覆盖 argv 会把一个**全新会话**接到某个旧/已死的原生会话上（`--session-id <new>` 被改写成 `--resume <old>`），用户看到的是新对话里冒出别人的上下文。所以 handle 只在 argv 里**没有任何**会话 flag 时兜底，且必须改写成 `--resume`（同一个 id 再 `--session-id` 一次会被 claude 拒绝启动）。
   - **`reconnect_fresh` 要返回真实的 `resumed`**：claude 的 argv 常常仍带 `--resume`（重连其实续上了），此时**不能**发「上下文已重置」提示 —— 一条假的重置提示本身就是 bug。
   - 真机验收：`live_reconnect_with_resume_keeps_answering_and_keeps_context`（仍带 `--session-id` 时这条会直接启动失败，单测抓不到）。

26. **`LaunchConfig` 指纹：启动参数变了就换进程**（`session/live.rs`）。常驻打破了「下一轮换个新进程带新 flag」这个白捡的便宜 —— claude 的 `--model` / `--effort` / `--permission-mode` / `--append-system-prompt-file` **全是启动参数**，不补这一层就会「界面显示一套、会话实际跑另一套」（违反第 8 条，是功能退步而非缺功能）。
   - 指纹 = `flags`（`model|reasoning|sandbox`，恒可知）+ `instructions`（`Option<系统指令哈希>`）。判定在**轮前**（注册表 `LiveSession::is_reusable` 的一部分）：不匹配 ⇒ 丢弃条目（actor 收到通道关闭后自行关停进程）⇒ 走连接分支**带原生 resume** ⇒ 新 flag 生效且上下文不丢。
   - **`instructions` 为 `None`（斜杠命令那一轮不注入指令）时不参与判定**。否则「斜杠命令 → 普通消息」会把进程来回重启两次。但反向必须成立：会话是被斜杠命令拉起来的（注册时 `None`）时，紧跟的普通消息**必须**重连，否则用户配置的系统提示与 Memory 在这个会话里永远不生效。这条不对称写在 `LaunchConfig::accepts` 里，配 6 组单测。
   - 非 claude 协议指纹恒为 `default()` ⇒ 永不触发重连，既有行为不变。**不要**为此新建控制通道（`set_model` / `set_permission_mode` 之类留作以后的优化）。

27. **常驻会话的两道取消防御**，都抽成纯函数配单测（第 13 条）：
   - **迟到的 `Error` 回声要吞掉**（`suppress_after_cancel`）：中断会在流里留下一串「本轮失败」的回声（assistant 帧的 `aborted`、`result` 的 `errors:["[ede_diagnostic] …"]`）。只吞 `Error` —— 已经流出来的正文与「本轮回答被中止」这类提示仍要发出去。
   - **abort 类读错误要能原地恢复**（`read_error_is_recoverable`）：中断会让挂起的 pipe 读抛瞬时错误（unix `EINTR`、**Windows `ERROR_OPERATION_ABORTED` = 995**）。把它们当「流结束」会让一个**完好的**常驻进程被判定为死亡并触发重连丢上下文 —— 而这恰好发生在用户点「停止」的那一刻。真正的致命错误（BrokenPipe / UnexpectedEof / …）才结束本轮；连续可恢复错误有上限（32），免得变成忙等。
   - **取消途中遇到 EOF** 报 `CANCELLED_SESSION_LOST` 而不是普通失败：普通失败会走 `RetryFresh` **把用户刚刚停掉的这一轮原样重发一遍**。

28. **真机验收的三条硬判据**（`session/claude_stream.rs::live_tests`，`#[ignore]` 门控，未登录/网络问题诚实 skip）：同一会话连服三轮且第 2/3 轮记得前面轮次的数字；**取消一轮之后同一个会话下一轮仍正常返回且上下文没丢**（这是整个改造的验收点）；换模型触发 `--resume` 重连后仍能回复且续上原会话。三条都断言可证伪的量（回答里含只可能来自上一轮的数字），不是「没崩」。

29. **stdout 上的帧必须按类型分流，而「在等回复的 `control_request`」不许沉默**（07-29，全部形状从本机 claude 2.1.220 二进制 `grep -a` 核实 + 真机对照）。判据是纯函数 `session/claude_stream.rs::classify_inbound_frame`。
   - **`control_response` 的错误形状**：`{"type":"control_response","response":{"subtype":"error","request_id":"<回显>","error":"<字符串>"}}`。两处互相印证：CLI 自己构造错误响应的地方（`mr=(ur,sn)=>j.enqueue({type:"control_response",response:{subtype:"error",request_id:ur.request_id,error:sn}})`），以及 zod schema（`response` 是 success | error 的 union，error 分支 = `{subtype, request_id, error}`）。**`request_id` 嵌在 `response` 里、不在帧顶层**（顶层只有 `type`；远程 bridge 才另挂 `session_id`）——放错层级 = 对端匹配不到 = 等于没回。**这条协议两套命名混用**：请求侧与错误响应的三件套是 snake_case，而 `can_use_tool` 的**成功**载荷是 camelCase（`{behavior, updatedInput?, updatedPermissions?}`），别照一侧推另一侧。
   - **不回复的代价是永久挂死**：CLI 侧的 `pendingRequests` 是个没有超时的 Promise，而我们的轮次读循环也没有超时。所以未实现的子型一律回 error（fail-closed）：CLI 收到 error 时 `o.reject(Error(t.response.error))`，那次工具调用按失败收场、本轮照常收尾 —— 宁可一次工具用不了，也不要整轮挂死。真机核实：往 stdin 写一条 `request_id` 是 CLI 从没问过的 error 响应，CLI 照常处理紧随其后的 user 消息（init → assistant → result），stderr 零字节、流不受污染。
   - **claude 2.1.220 在 Kivio 这套 argv 下实测不发 `control_request`**（别把这条当成「修了一个正在发生的挂死」）：`--permission-mode default` + 让它写文件，权限被**直接拒**（result 里是 "The write needs your permission to proceed"），stdout 一条 `control_request` 都没有；二进制侧 `can_use_tool` / `request_user_dialog` 的发送端（`qHS`/`zHS`）只挂在 **REPL / remote bridge** 传输上，纯 stdio 的 `-p` 走 `--permission-prompt-tool` 或直接拒。整套机制在二进制里是完整的（CLI→客户端的 `control_request` 是 12 成员 union），所以这一手是**保障**，不是绕过。
     - **07-29 更新：那条「走 `--permission-prompt-tool`」的路已经打通并出货了，见第 30 条**。所以现在 `can_use_tool` 是**真的会到达**的（只在带那个 flag 的档位下），不再只是保障；其余 11 个子型仍走这条 fail-closed 的 error 兜底。上面那句「实测不发」的成立前提是**argv 里没有 `--permission-prompt-tool`**——那仍是默认档的形态。

29b. **`--resume` 的目标会话不存在时必须降级，不是把 CLI 的英文原句甩给用户**（07-29）。判据是纯函数 `stream::claude::is_missing_session_error`（认 `No conversation found with session ID`），处置在 `run::persistent_failure_action` 的 `ReconnectWithoutResume` 分支（spec 第 6 条：改重连逻辑先改这个纯函数）。
   - **失败同时落在两条通道上**（本机实测 `-p --output-format stream-json --resume <随机 uuid>`）：stdout 一条 `result`（`subtype:"error_during_execution"` / `is_error:true` / `errors:["No conversation found with session ID: <id>"]`，**无 `terminal_reason`** 所以不会被 `result_is_user_abort` 误判成取消）；stderr 同一句话，然后 `exit 1`。**进程约 2.2s 才退**，比 `connect()` 那次即时 `try_wait()` 晚得多 ⇒ 通常走流那条，偶发走「启动即退出」那条。**两处都要认**，只处理一处会漏掉主要场景。
   - print 模式只有这一句判据。二进制里 `a1r("No conversation found with session ID: ${s.sessionId}", outputFormat)` 紧跟 `exit(1)` 是 print 分支唯一的 resume 失败出口；`Failed to resume session <id>` / `--resume session load failed` 属于**交互式** UI 分支（走 Ink + `J6`），我们走不到。
   - **降级要同时做三件事**（`run::drop_resume_for_fresh_session`），少一件就留坑：改 argv（`claude_args_fresh_session`：摘掉 `--resume <死 id>`，换 `--session-id <新 UUID>`）+ 改**落盘会话记录**的 `session_id`（`--resume` 的值是从它读的，不改则**每一轮**都要降级一次、每轮丢一次上下文）+ 清 live handle。用户看到的提示复用**已有的** `CONTEXT_RESET_NOTICE`，不要新写文案。
   - 真机验收：`live_resuming_a_missing_session_degrades_to_a_fresh_session`（先断言真实失败文案与判据一致，再断言按降级动作改写 argv 后进程能起来且**不再**报 resume 失效）。这条**不需要登录**（resume 的加载发生在认证之前）。

29c. **取消之后的残帧抑制要跨轮，窗口靠下一轮的 `system/init` 关闭**（07-29，判据是纯函数 `session/claude_stream.rs::stale_frame_verdict`）。取消是在**本轮的 `result`** 上收尾的，claude 侧的收尾帧与 interrupt 的 ack 可能还有几帧在路上，会被**下一轮**的读循环读到 ⇒ 上一轮的半截正文漏进新回答，更糟的是上一轮迟到的 `result` 被当成新一轮的结束信号（新回答还在流就被判定结束）。此前的抑制（`suppress_after_cancel`）只覆盖**本轮内**的错误回声这一条通道。
   - 窗口内的帧**整帧丢弃**（不喂解析器、不发事件、不当轮次边界）：它们属于一个我们这边已经收尾的轮次，喂进去只会把本轮的 per-turn 状态在中途清掉。
   - **关闭条件必须有两道**。正常出口是 `system/init`：实测**每轮都发**且排在本轮任何输出之前（两轮探针帧序 `init → status → assistant → result → init → status → assistant → result`），所以本轮真正的输出在结构上不可能被吞；兜底是 `STALE_FRAME_BUDGET`（64 帧），因为**一直抑制会把下一轮真正的输出整段吞掉，那比原 bug 更糟**。子会话（带 `parent_tool_use_id`）的 init 不算新一轮开始。
   - **覆盖不到的残留竞态**（留给 uuid 配对，不是漏了）：`interrupt` 是异步写进 stdin 的，若它到达 claude 的时刻晚于上一轮收尾，中断就落在**下一轮**头上 —— 那时新一轮的 init 已过、窗口早已关闭，帧序推断无从分辨。`result` 帧上的 `user_message_uuid`（二进制已核实存在）能直接回答「这条 result 属于哪条用户消息」，那才是根治办法；它落地后这一层可以简化。

## 工具审批（claude）

> 来源：07-29 的工具审批任务。全部协议事实为 claude 2.1.220 / Windows 本机实测 + `grep -a` 读二进制核实，
> 实现在 `session/claude_stream.rs`（会话侧）与 `run.rs::ApprovalHost`（宿主侧）。

30. **让 claude 走控制通道问权限的开关是 `--permission-prompt-tool stdio`，不是宿主的能力声明**。这条推翻了「必须先发一条 `initialize` 控制请求声明能力，CLI 才会来问」的猜测 —— **`initialize` 与此无关**。
   - 值就是**字面量 `stdio`**（不是某个 MCP 工具名）。两条独立证据：二进制里 `ekm(mode, …)` 的分发是 `if(mode==="stdio") return t.createCanUseTool(n)`，否则才去 MCP 工具表里按名字查（查不到报 `Error: MCP tool … (passed via --permission-prompt-tool) not found. Available MCP tools: …`）；而官方 Agent SDK 在用户提供 `canUseTool` 回调时 push 的正是 `["--permission-prompt-tool","stdio"]`（同一处还会拒绝同时传 `permissionPromptToolName`：`canUseTool callback cannot be used with permissionPromptToolName`）。
   - **对照实测（判据可证伪）**：带 flag + `--permission-mode default` 让它写文件 ⇒ stdout 收到 `control_request` / `can_use_tool`；**不带 flag 的对照组一条都没有**，权限被 CLI 自己直接拒（`"Claude requested permissions to write to …, but you haven't granted it yet."`）。真机验收 `live_tool_approval_allow_really_runs_the_tool_and_deny_really_blocks_it` 把两件事都断在文件系统上：答 allow ⇒ 文件真的建了；答 deny ⇒ 文件没建、且 deny 的 `message` 原样成为那次调用的 `tool_result`（`is_error:true`），模型据此改口说明失败。
   - **`--permission-mode` 与它的值必须相邻**：flag 只能追加在档位值**之后**。插在中间会让档位变成 `--permission-prompt-tool`、真正的档位沦为裸位置参数（单测 `build_args_carries_the_prompt_tool_only_for_the_ask_mode` 抓到过一次）。
   - **只有 `default` 档带这个 flag**（判据是纯函数 `defs::claude::claude_permission_prompt_args`），理由是**不改既有行为**：`bypassPermissions`（Kivio 的默认档，`DEFAULT_PERMISSION_MODE`）在咨询回调**之前**就放行一切（二进制原话：`canUseTool will not be invoked: permissionMode 'bypassPermissions' auto-approves every tool call`），加了 flag 等于空转，却会顺带把 `AskUserQuestion` / `EnterPlanMode` / `ExitPlanMode` 塞进 `system/init` 的工具表（实测：带 flag 28→有这三个，不带没有）—— 那三个我们还答不了；`plan` / `acceptEdits` 今天的行为是「越权操作被 CLI 直接拒」，加了 flag 会变成「开始弹卡片」，那是产品决策，等默认档定了一起放开。**改这个门控前先想清楚它会改变谁的既有体验。**
   - **本轮接不接审批的判据取自 argv 本身**（`run::turn_asks_for_permission`：argv 里有没有 `--permission-prompt-tool`），**不要**在宿主侧重抄一份「哪些档位会询问」的规则 —— 那份规则的唯一副本在 `claude_permission_prompt_args`，抄第二份迟早分叉（第 2 条）。

31. **审批必须接到 Kivio 已有的那条链路上，不许新造一套**（第 2 条）。内置 agent 用的是：后端 `AppState.pending_chat_tool_approvals`（按工具调用 id 存 `oneshot::Sender<bool>`）→ `chat-tool-confirm` 事件 → 前端 `Chat.tsx` 的确认卡 → `chat_confirm_tool_call(toolCallId, approved)` 命令。外部 CLI **原样复用全部四层，前端零改动**。
   - 唯一需要的适配是 **id 映射**：Kivio 那侧按「工具调用 id」寻址，而 CLI 给的是它自己的 `request_id`。所以卡片 id 用 claude 的 `tool_use_id`（这样审批卡与工具卡指向同一次调用），回程按 `request_id` 路由，两者在 `session::live::ApprovalAsk` 里成对携带。`tool_use_id` 在 schema 里是 optional，缺失时回落 `request_id`（卡片必须有个稳定 id 才能被答复）。
   - **`format_tool_approval_summary` 的工具名/字段名要认外部 CLI 的形态**：内置用小写 snake_case（`bash` / `write`）+ `path`，claude 用 PascalCase（`Bash` / `Write`）+ `file_path` / `notebook_path`。不认的话外部 CLI 的审批卡永远落进 `_` 分支、只剩一坨 800 字截断的 JSON —— 与第 23 条前端工具卡踩过的是同一个坑，**修在那一份里，不要再复制一个外部专用的摘要函数**。
   - 超时（60s）/ 用户点停止时 `request_tool_approval` 自己返回 false 并清掉挂起条目 = **默认拒**，正是 fail-closed 想要的。但**宿主侧的挂起条目要显式扫**（`ApprovalHost::forget`）：异常出口（EOF / 硬 `Close`）下那个 `ask` future 被直接丢弃、没机会自清，不扫就在那张进程级的表里永久留一条。

32. **挂起的询问一条都不能漏回**，但**别把它写成「修了一个正在发生的死锁」**。
   - 不变量本身是结构性的：CLI 侧的 `pendingRequests` 是个**没有超时**的 Promise，我们的轮次读循环也没有超时 ⇒ 一条永远不被回复的询问 = 一次永久挂死。所以取消 / 关会话 / 控制通道断 / 轮次收尾时，会话侧的挂起表必须被**整批**拒掉（`claude_stream::reject_pending_lines`），且取消的顺序是**先拒挂起的、再发中断**。
   - **诚实的边界（实测）**：把批量拒绝从 Cancel 分支注释掉之后，真机测试 `live_cancelling_with_a_pending_approval_leaves_the_session_usable` **仍然通过** —— 我们那条 `interrupt` 会让 CLI 自己 abort 掉在飞的权限请求（那个 promise 挂在本轮的 `AbortController` 上）。所以今天**没有**可观测的死锁；批量拒绝是 fail-closed 的兜底，把「不挂死」从「依赖 CLI 内部恰好会 abort」变成「我们自己保证」，代价是几行 JSON。**红→绿判据是单测 `cancelling_rejects_every_pending_approval_exactly_once`（断言每个 request_id 出现且只出现一次、且一律 deny），不是那条真机测试。**
   - **`control_cancel_request` 从「静默忽略」改成「从挂起表摘掉」**（仍然**不回任何东西** —— CLI 自己收到这条时也只 `abort()`）。留着那条作废的询问会让批量拒绝对它回一条多余的响应，更糟的是让「本轮还有人在等」永久为真。
   - **答复的线上形状**：`{"type":"control_response","response":{"subtype":"success","request_id":"<回显>","response":{…}}}`，载荷是 `{"behavior":"allow"}` 或 `{"behavior":"deny","message":<必填>,"interrupt":false}`。**三件套 snake_case、载荷 camelCase**（同一条协议两套命名混用），`request_id` 嵌在 `response` **里面**、不在帧顶层。`interrupt:true` 会让 CLI abort 整轮 —— 拒一个工具不等于停掉整轮，别顺手带上。
   - **`requires_user_interaction` 的工具当场拒、不打扰用户**（`approval_verdict`）：`AskUserQuestion` / `ExitPlanMode` 那类要用户在卡片上直接作答，批准之后 CLI 会紧接着发一条我们**还没实现**的 `request_user_dialog`，那条被 fail-closed 回 error、工具照样失败 —— 给用户一张点了也没用的卡片比诚实拒掉更糟。**实现 plan 模式 / 反问用户时从这里入手**：它们不单独分帧，就是工具名为 `ExitPlanMode` / `AskUserQuestion` 的普通权限询问，按工具名分类处理；plan 批准要**先发切换权限档位的控制帧、再回 allow**，顺序不能反（print 模式的 stdin 确实收 `set_permission_mode` 控制请求，二进制核实）。

   - **`keep_alive` 静默忽略即正确处理**：schema 是 `{type:"keep_alive"}`（无字段、无 `request_id`，描述原文 "Keep-alive message to maintain WebSocket connection"），CLI 自己的两个读取点都是直接 `continue`/`return`，**没有任何需要回应的语义**。它在某些传输上按 30s 间隔发（`setInterval(...,30000)`），我们这条路实测不出现 —— 但按「可能出现」处理，代价为零。
   - **`control_cancel_request` 不回任何东西，但要从挂起表里摘掉那条询问**（07-29 权限审批落地后改的，见第 32 条；此前这里写的是「静默忽略即正确」，那在有挂起表之后就不对了）。它是 CLI 撤回自己先前发出的某个 `control_request`（权限询问已在别处答掉）；CLI 自己收到这条时也只 `abort()` 在飞的请求、**不回任何东西**，所以「不回复」这一半仍然成立。`control_response`（我们那条 `interrupt` 的 ack）是真的有意不接：取消的权威信号是那一轮的 `result`，ack 可能在我们已经返回之后才到，读它没有决策价值。
   - **有意不加轮次超时**（不是漏了）：一轮合法地可以跑很久（连着调几十个工具），任何总时长上限都会误杀并连带丢掉会话上下文。「完全没有帧到达」的静默超时判据是成立的（长工具每 30s 一条 `tool_progress` 心跳，二进制 `setInterval(...,30000)` 发 `tool_heartbeat`），但唯一已知的「永久等下去」成因就是不回 `control_request`，本条已让它在结构上不可能；而用户始终有一条**有界**逃生通道：点停止 → interrupt，10s 不收尾就硬 `Close`。真要加，判据必须是「距上一帧超过 N 分钟」（N ≫ 30s）且**不得**顺手丢掉会话。
     - **权限审批落地后这条仍然成立**（07-29，见第 30–32 条）：一条待答的 `can_use_tool` 会合法地挂上一段时间（等用户点），但它是**有界**的 —— 复用的 `request_tool_approval` 自带 60s 超时且超时 = 拒，取消 / 关会话 / 轮次收尾还有批量拒绝兜底。**不要因为「现在会等用户了」就给轮次加超时**。

## 检测与模型探测（detection.rs、commands.rs、state.rs）

9. **回复路径零探测**：`run_external_cli_reply` 前置只允许 `resolve_binary`（毫秒级）。version/auth/模型探测只属于列表阶段和懒查命令。任何人不得把 `detect_single_agent`/`probe_models` 加回回复路径（audit N2：曾造成每轮 10-25s 延迟）。
10. **流式 reader 对非 JSON 行一律 `continue`**：任何逐行读子进程 stdout 的解析（探测/会话/命令发现）遇到 banner/日志行只跳过，绝不放弃整条流（audit 缺陷 3 的教训；detect_acp_models 曾用 `?` 硬退）。
11. **探测结果必须带来源**：`chat_detect_external_agent_models` 返回 `source: probed|fallback` + `probeError`；fallback 走 30s 负缓存（probed 300s），force 绕过。前端降级必须可见（角标 + 重试），禁止静默降级到静态表。**负缓存同样适用于斜杠命令探测**（07-22 probe-hygiene）：空列表按 `SLASH_COMMANDS_EMPTY_CACHE_TTL`（30s）负缓存、非空 300s（get 侧按 entry 空/非空裁定 TTL）——空结果不写缓存会导致切会话/切 agent 每次重探（kimi 每次探测落一个空壳会话）。
11b. **探测 cwd 统一走 `resolve_detection_cwd`**（非项目会话 = `chat-workspaces/__global__`，绑定项目的会话 = 项目根）：模型探测、斜杠命令探测，以及**所有读写探测缓存的 key**（`slash::cache_key(agent_id, detection_cwd)`）必须用同一 cwd——run.rs 运行时学到的斜杠列表写入、context.rs 的模型缓存读取都曾因 key 用执行 cwd 而与探测 key 分叉（恒 miss / 永远覆盖不了）。执行 cwd（`resolve_effective_cwd`，每会话独立 workspace）只用于真正跑轮次，不得用于缓存 key。斜杠探测本身也用 `detect_availability_single`（不连带 `probe_models`），探测残渣（CLI 落盘的空壳会话）只进 `__global__`。
   - **claude 的探测一律加 `--no-session-persistence`**（07-29）：探测是一次性子进程、只为读 `system/init`，但 claude 会把它记成一个真实会话落到 `~/.claude/projects/<cwd 编码>/<session-id>.jsonl`，用户自己的会话列表就被一堆只含一条 `"."` 的空壳污染（kimi 侧实测某目录下 53 个会话有 52 个是这种残渣）。判据是纯函数 `defs::claude::ephemeral_probe_args`，两个探测入口（`session/claude_init.rs::probe_claude_init`、`slash.rs` 的 ClaudeInit 分支）都要过它。真机核实：不带 flag 且探测存活到 init 之后 5s ⇒ 目录里多出一个以我们 session id 命名的 `.jsonl`；带上 ⇒ 一个都不多，而 init 照样按时到达（两次都约 3.3s）、`slash_commands` 字段完整。**注意残渣是概率性的**：生产代码读到 init 就立刻 kill，常抢在写盘之前，所以真机测试的对照组必须刻意多活几秒，否则会变成一条永远绿的假验证（`live_probe_leaves_no_shell_session` 里那句 `baseline >= 1` 就是这个闸门）。**绝不能加到真实回复路径上** —— 那会让用户的对话不落盘、`--resume` 直接失效，所以做成独立函数而不是塞进 `build_claude_args`。
12. **defs 静态表只是 fallback**：`fallback_models` 首项恒为 `default`（前端 `agent.models[0]` 依赖此契约）；运行时探测到的才是模型事实源。给 CLI 传 flag 前先 `--help` 核实语义（audit N5：pi 曾把目录塞进 `--append-system-prompt`）；`--help` 里没有的**隐藏 flag** 用第 21 条的零副作用探针 + `grep -a` 读二进制核实，别照注释或记忆下结论。**CLI 当前配置（current_model/current_reasoning）的本地配置读取**（codex `~/.codex/config.toml`、pi `~/.pi/agent/settings.json`、kimi `~/.kimi-code/config.toml`）只允许挂在 `detect_agent_models`（懒查模型探测路径，probe 成功分支），禁止进回复热路径；配置缺失/解析失败一律 `None`（前端显示「自动」），解析器必须有非法输入不 panic 的单测。
   - **claude 是这条的例外：静态表就是事实源，不是 fallback**（07-29 补记，免得后人以为是遗漏）。claude **没有**「列出所有模型」的命令，唯一的运行时信息是 `system/init` 里那个**已解析**的模型名 —— 它只回答「默认是谁」，给不出全集。而 `--model` 是**原样透传**的（实测 `--model claude-bogus-9` 照样启动、init 原样回报），CLI 本地不做任何校验，错误要等真正发请求时才炸。所以 claude 的模型目录（别名 + 具体版本）只能是静态表，数据源是**本机装的那个二进制里烘进去的模型目录**（`{id,family,display_name,capabilities,…}` 数组 + 别名白名单 `y1e`，按第 21 条当文档读，原样抄不编）。
   - **配套的版本门控**（`claude_init.rs`）：具体版本条目只在装的 CLI **不低于抄表时那个版本**（`CONCRETE_MODELS_MIN_CLI_VERSION`）时提供，否则整组不给（只剩别名 + 用户自配，= 改动前行为，严格不退步）。更旧的 CLI 烘的是更短的一张目录表，里面有哪些 id 我们**没有证据**，宁可少给几个选项也不要给一个「点了才发现不行」的。版本号从**探活缓存**读（`spawn::cached_cli_version`）—— 探活本来就跑了一次 `--version`，此前把 stdout 丢进 null，现在接住存进同一条缓存（key 已含文件身份，换版本自动失效）；**不许为补版本号另起进程**（第 9 条）。读不到版本 ⇒ 不做门控，未知不该等于功能消失。
   - **具体版本条目一律不带上下文窗口**：二进制的 catalog 里确实有 `context:{window,…}`，但那正是第 14g 条明令不许再建的那张表。这里只回答「有哪些可选」。
   - **`claude_config_effort` 读的环境变量是 `CLAUDE_CODE_EFFORT_LEVEL`，不是 `CLAUDE_EFFORT`**（07-29 修正）。后者在二进制里是个**输出**变量 —— CLI 把「本轮实际生效的档位」导给 hook / Bash 用（zod 描述原文 "Also exposed to hook commands and Bash as the CLAUDE_EFFORT env var"）。读错的后果不是「读不到」而是**读到别人的**：Kivio 若从 Claude Code 内启动，`CLAUDE_EFFORT` 会被继承进来（它不在 `spawn::PARENT_SESSION_ENV_VARS` 的剥离清单里），胶囊显示的就是宿主那一轮的档位。`unset` / `auto` 按 CLI 自己的语义当「没配」。

## 上下文用量口径（分子/分母）

> 来源：任务 07-26-local-cli-context-usage。全部数字为 2026-07-26 本机实测，复现命令见该任务的 `research/cli-wire-facts.md`。起因：kimi 会话在 Kivio 显示 `~24 / 200.0K`，而 kimi CLI 自报 `0/256k`——分子分母**各错各的**。

14. **cache token 照样占上下文窗口，任何只读 `input + output` 的口径都会低估一个数量级**。实测 cache 占输入侧比例：kimi 97.6%（23040/23605）、pi 62%（4096/6571）、opencode 13%。这与计费口径无关——缓存命中不重复**计费**，但它**依然在窗口里**。`stream/mod.rs::usage_from_parts` 是所有外部 CLI 唯一的 usage 构造入口，旧签名 `usage_from_numbers(input, output)` 从签名上就把 cache 挡在门外，四个调用点（claude / codex / pi / acp）全部受害——这是本轮的共同根因。

14a. **各 CLI 的 cache 包含关系不同，必须逐个用真实 payload 做加法对账**。这是本轮最隐蔽的坑：
   - **Anthropic 口径**（claude / pi / ACP）：`cache_read` 与 `input` **不相交**，须相加。pi 实测 `6571 + 1578 + 4096 = 12245 = totalTokens` ✓。
   - **OpenAI 口径**（codex）：`cachedInputTokens` 是 `inputTokens` 的**子集**，再加一遍就是双算。codex-cli 0.145.0 实测 `inputTokens 16865 + outputTokens 7 = 16872 = totalTokens`，其中 `cachedInputTokens` 3456 已含在 16865 内；错加会得到 20328，**虚高 20%**。

   由 `CliUsageParts::cache_included_in_input` 表达。**新增 CLI 前先取一条真实 payload 做加法对账**（看 `input + output (+cache?)` 哪个等于其自报的 `totalTokens`）再决定该标志——不要照抄邻近 CLI。这与内置路径 `chat::agent::context_estimate::anchor_total_tokens` 按 `api_format` 分家族消歧是同一件事，别在任何一侧「统一」，会算错一整类 provider。

14b. **分子取 `total_tokens`，不是 `input_tokens`**。`external_agents/context.rs::cli_reported_context_tokens` 是唯一出口。Anthropic 口径下 `input_tokens` 只是**非缓存**部分，只读它等于把 14/14a 补的 cache 全丢在显示前最后一层——本轮真实发生过：L0–L7 全部正确，`collect_external_session_usage` 一句 `usage.input_tokens` 让整个修复空转（45300 cache token 的会话显示 1200 而非 47300）。`total_tokens` 缺失时才退回 `input + output`（改动前落盘的旧会话没有 total，它们要到下一轮才准）。

14c. **多次上报要取「当前上下文快照」，不是累计**：
   - codex `thread/tokenUsage/updated` 读 `tokenUsage.last`（最近一次请求的快照），**不是** `total`（整个 thread 的累计计费口径，随轮次单调增长，当已用上下文会持续虚高直至把进度条推满）。`last` 缺失才退 `total`（旧版兼容）。
   - claude `result.usage.iterations[]` 取**末项**，不累加：一轮内多次 LLM 往返，每项是独立快照，累加得到的是本轮计费总量。数组为空/无该键才用 `usage` 顶层。
   - 推理 token 通常**已含在 output 内**（codex `5+7=12`、pi `6571+1578+4096=12245` 均已对账），再计一次会重复；只有 ACP 的 `thoughtTokens` 与 output 并列（opencode 实测 `11685+4+11+1792=13492=totalTokens`）。

14d. **ACP 有官方 `usage_update` 通道，同时给分子和分母**（ACP RFD「Session Usage and Context Status」）。形如 `{"sessionUpdate":"usage_update","used":13477,"size":200000,"cost":{...}}`——字段**平铺在 `update` 下**，不嵌套在 `usage` 对象里。实测 opencode 在发，kimi / cursor 不发。**它不是消息边界**：在 `acp_apply_session_update` 里必须**先单独匹配**，绝不能落进 `_` 分支——那条会委托 `apply_acp_session_update` 并在返回 true 时触发 `text/thought.on_boundary()`，正在流式的正文游标被重置后累积快照的 `starts_with` 判断失效，**整段正文重复发一遍**。两处分发共用同一个 `parse_acp_usage_update`（第 2 条：禁止两份拷贝）。另有 `PromptResponse.usage`（ACP 标记 UNSTABLE，缺失不得报错）带 `cachedReadTokens` / `cachedWriteTokens` / `thoughtTokens`；它**不带** `size`，而 `usage_update` 通常先到，所以 `run.rs::merge_cli_usage` 要做**窗口字段粘滞**（新值窗口为 `None` 时保留旧值），否则分母会被后到的上报冲掉。

14e. **窗口拿不到时外部 CLI 路径返回 `None`，不得兜底 200K**。`context_window_for_external_model` 的优先级链：CLI 本轮实报（**claude `result.modelUsage[model].contextWindow`** / ACP `usage_update.size`，最高——模型可能中途切换，比任何静态表都准；**值必须 `> 0` 才算实报**）> 探测上报（`RuntimeModelOption.context_window_tokens`）> claude 别名表（**只认显式 `[1m]`/`[1M]` 标记**，见 14g）> kimi 静态映射 > 数据库/关键词 > `None`。假分母比没有分母更有害：它让 `usage_ratio` 算出假百分比，进而在**错误的点**触发压缩阈值。`context_window_for_model` 自带的 200K 兜底对**内置**路径是合理的（那里 provider 元数据可靠）——**只在外部 CLI 这条路剥掉**，内置分支保持不变。前端 `ContextIndicator` 已能渲染窗口未知（`? Token` + `满度未知`，不显示百分比与阈值刻度）；注意区分「窗口永远拿不到」与「用量待上报」两种文案，前者用 `contextFullnessWindowUnknown`。

14f. **窗口来源是 per-CLI 的，别假设统一形态**：**claude 走 `result.modelUsage[当前模型].contextWindow`**（见 14g）；codex 走 `debug models` 的 `context_window`（**不是** app-server 的 `model/list`，实测那里没有窗口字段）；grok 走 ACP `_meta.totalContextTokens`；pi 走 `--list-models` 定宽表第 3 列；**cursor 把窗口写在 modelId 的方括号里**（`claude-opus-5[thinking=true,context=300k,effort=high]`，实测 33 个模型 14 个带 `context=`，且**全都没有** `_meta`），且它的模型走 **`configOptions` 分支**而不是 `models.availableModels`——那条分支命中后会提前 return，只给 availableModels 加解析在生产里等于没做（实测 0/33，单测却是绿的，只有真机 `#[ignore]` 测试才抓到）。kimi 的 ACP 上游什么都不给（`session/new` 无 token 字段、`session/prompt` 无 usage、不发 `usage_update`），只能静态映射 + 读其 `wire.jsonl`；后者按 **workDir** 关联（kimi `session_index.jsonl` 的 `workDir` 恰等于 `resolve_effective_cwd()`，**不能**用 session id——kimi 走 ACP，id 由它自己生成，Kivio 根本没存），且必须**跳过空壳会话**（实测某 workDir 下 53 个 session 有 52 个是第 11b 条所述的 slash 探测残渣，判据：存在 `type=="usage.record"` 且 `usageScope=="turn"`）。

14g. **claude 的分母只有 CLI 自己知道 —— 静态别名表已被实测推翻，不要再建一张**（07-29）。
   - `result.modelUsage` 是 `Record<模型id, entry>`，entry 字段：`inputTokens` / `outputTokens` / `cacheReadInputTokens` / `cacheCreationInputTokens` / `webSearchRequests` / `costUSD` / **`contextWindow`** / `maxOutputTokens` / `canonicalModel?` / `provider?`。CLI 内部给 `contextWindow` 赋值时依次考虑 `CLAUDE_CODE_MAX_CONTEXT_TOKENS` 环境覆盖、`context-1m-2025-08-07` beta、per-model 表 —— **任何外部静态表都复现不了**（尤其 env 覆盖与第三方 router 模型）。
   - **`modelUsage` 里的 token 数是进程累计**（CLI 内部 `+=`），只有 `contextWindow` / `maxOutputTokens` 是静态属性。实测同一轮 `usage.input_tokens=2` 而 `modelUsage.inputTokens=7041`（三轮累加），`costUSD` 同样累计。**绝不能拿 `modelUsage` 的 token 数当分子**；分子仍走 `usage.iterations[]` 末项 / 顶层（14c + 14h）。
   - **取值规则**（`stream/claude.rs::context_window_from_model_usage`）：按 `system/init` 的 resolved model **精确匹配 key**（实测 init 报的就是带后缀的 `claude-opus-4-8[1M]`）→ 匹配 entry 的 `canonicalModel`（不带后缀）→ 退「取最大值」。不能直接取最大：`modelUsage` 是**会话累计 map**，会话里换过模型 / 跑过用 haiku 的子 agent 时，最大值可能是历史上那个更大的窗口。`contextWindow` **可以是 0**（本机 `~/.claude/stats-cache.json` 里 8 个模型全是 0），必须 `> 0`；被中断的轮次 `modelUsage` 实测为 `{}`，一律当缺失。
   - **`system/init` 每轮都发**（不是只在开头），所以 resolved model 存「最近一次」正好是想要的语义（中途换模型能跟上）。附带坑：第 1 轮 init 的 `tools` 是 28 项、第 2 轮起 35 项（MCP 懒连接后才补全）—— **任何「从首轮 init 缓存工具/斜杠列表」的假设都会拿到不完整的表**。
   - **旧的别名表已删**（`context_window_from_claude_model_alias` 现在只认显式 `[1m]`/`[1M]`）。原规则「在别名白名单里 ⇒ 200K」被实测推翻：claude 2.1.220 本机 init 探测显示 `--model opus → claude-opus-4-8[1M]`、`sonnet → claude-sonnet-5[1M]`、`fable → claude-fable-5[1M]`、`haiku → claude-sonnet-5` —— **4 个裸别名里 3 个是 1M**，旧表给的是小 5 倍的假分母（压缩阈值在真实占用 20% 时就触发）。**不要改成硬编码「opus/sonnet = 1M」**：那只是把同一张会过期的表换个值。首轮 result 之前显示「满度未知」是诚实代价。
   - 真机验收：`live_result_model_usage_reports_a_context_window`（断言 `modelUsage` 非空、`contextWindow > 0`、解析出的 `ModelUsage.context_window_tokens` 与实报一致、分子 < 窗口）。

14h. **零用量的 `result` 不许把分子清零**（07-29，三道防线）。没有 LLM 往返的轮次——未登录、`/help`、未知斜杠命令、**Kivio 自己发的 `/compact`**（`compact.rs`）——返回 `iterations: []` + 顶层四字段全 0。
   1. `stream/claude.rs` 的 result 分支：token 分量全零时**不产出 `Usage` 事件**（窗口不为空时才发，只为携带分母）。
   2. `run::merge_cli_usage`：**全零的 incoming 不覆盖非零的 previous**（只采纳它的窗口）。同一轮里 `message_start` 已报过真实占用，被后到的零值 result 整体覆盖就会让用量条从 47K 掉到 0。
   3. `context::collect_external_session_usage` 挑「最近一条带 usage 的 assistant 消息」时**跳过全零**。判据不能是 `input_tokens.is_some()` —— `Some(0)` 会命中它。
   另外 `claude_result_usage_snapshot` 的**顶层回退带闸门**（仅本会话第一个 `result`）：顶层 `usage` 是本轮**计费总量**（= 各 iteration 之和），第一轮单次往返时恰等于上下文占用，第二轮起既不是快照也漏掉历史。实测 `iterations` 在**不调工具的轮次里恒为 `[]`**，所以这个回退很常走 —— 正因为常走才必须只在第一轮放行；闸门关上时那轮的分子由 `message_start.message.usage`（服务端算、每次请求都有）承担。
   - **B1 之后这个闸门真正开始生效**（同一个 `ClaudeStreamState` 跨轮存活），于是第 2 轮起**分子完全依赖 `message_start.message.usage`** ⇒ `--include-partial-messages` 必须**始终开着**（`run.rs` 恒传 `include_partial_messages: true`）。关掉它、或者把解析器改成每轮新建，用量条从第 2 轮起就没有分子了。单测锚点：`second_turn_numerator_comes_from_message_start_because_the_top_level_fallback_is_gated`。

14i. **上下文占用必须在生成过程中就跟着走，实时通道只带两个数**（07-29）。此前分子/分母只在轮末刷新一次，一轮连着调十几次工具时用户全程看不到占用在涨。
   - **输出侧是独立的一条通道**：claude 的输入侧在每次请求**开始前**由 `message_start.message.usage` 报（14h），输出侧则是边生成边报的 `stream_event → message_delta.usage`，而它**只带 `output_tokens`**。因为 `merge_cli_usage` 是「后到覆盖先到」，直接把这条当完整快照上报会把输入侧（含 cache，常几万 token）整体冲掉——必须与本次请求的输入侧快照**相加**（`stream/claude.rs::message_delta_usage`，快照存在 `LaneState::request_input`，在 `message_start` 整体替换、`result` 随车道清空，满足第 3 条的跨轮复位）。
   - **实时通道建在统一形状那一层**：`run.rs::apply_unified_event` 的 `Usage` 分支是全部外部 CLI（claude `message_delta`/`result`、ACP `usage_update`、codex `thread/tokenUsage/updated`、pi）唯一的汇合点，接在这里全体受益且边际成本为零。**不要**在各 CLI 的解析器里各接一份（第 2 条）。
   - **分流必须在实时通道之前**：子会话（sidechain / 子 agent）的用量根本不产出 `Usage` 事件（按 `parent_tool_use_id` 提前 return）。顺序反了，派子任务的那几秒用量条会跳到一个与主对话无关的数字再跳回来（子 agent 常用便宜小模型，窗口小 5 倍）。单测锚点 `sidechain_usage_never_reaches_the_main_conversation_realtime_value`。
   - **复用 `chat-context` 这一条通道，载荷用 `live` 与权威快照 `contextState` 区分**（`chat/commands/context.rs::emit_chat_context_usage_live`）。实时路径**不许重算整个上下文状态**：轮末那次要读磁盘（会话文件、kimi 的 `wire.jsonl`）、列 MCP 工具、算分段，放在每个增量上就是热路径灾难（第 9 条的精神）。所以只推分子 + 分母，节流 350ms（对齐子 agent 进度卡），分子为 0 / 数字没变都不推。
   - **分子口径的单一真源仍是 `context::cli_reported_context_tokens`**，实时与轮末共用它——在事件层或前端另算一套，用户就会在轮末看到数字跳一下。分母：CLI 本轮实报优先，兜底（探测上报 / 静态表）在**读流前**算好一次；前端 `applyLiveContextUsage` 对 `contextWindowTokens: null` 做**粘滞**（claude 只在轮末的 `result` 里带窗口，冲掉旧值会退回「满度未知」）。状态阈值与来源标签**不在前端重算**，留给轮末的权威快照。
   - **内置模型路径同样接了**，来源是 `agent/compaction.rs::maybe_compact_send_view`：它每个 planning 轮都跑一次，且**已经**按权威口径算出了分子（`effective_context_tokens`）与分母（`context_window_for_model`）——压缩阈值判定要用，所以实时通道零额外计算。走 `AgentHost::emit_context_usage_live`（默认 no-op），**子 agent 的 host 必须保持默认**，理由同上一条。粒度是「每轮一次」：内置路径的分子来自 provider 的 usage，一次模型调用结束才有新数，中途没有更细的真实来源。

## CLI 协议失败必须走错误出口

> 来源：任务 07-27-claude-stream-json-parity。

19. **`UnifiedAgentEvent::Error` 必须真的落到用户可见的出口，不能只 `eprintln!`**。改动前 `run.rs` 对该变体只打一行 debug 日志 —— 于是 claude 的 `result.is_error`、codex 的 `turn/completed status=failed`、pi 的 `stopReason:error` **全部被静默吞掉**，用户拿到一个空回复、没有任何提示。这与第 5 条（错误统一走 `errors::classify`）名义上不冲突却实际架空了它。判据：**读流成功 ≠ 本轮成功**。CLI 干净退出、stdout 全是合法 JSON 时读流照样返回 `Ok`，失败只存在于流里那一条消息中；`resolve_turn_error(read_error, stream_error)` 把两种来源并到同一出口，读流错误优先（它更接近传输层根因）。

20. **判 `result` 是否失败要同时看 `subtype` 与 `is_error`，只看一个会漏**。官方 `SDKResultError.subtype` 有四种（`error_during_execution` / `error_max_turns` / `error_max_budget_usd` / `error_max_structured_output_retries`，带 `errors: string[]`），但**未登录时的真实样本是 `subtype: "success"` + `is_error: true`**，错误文案落在 `result` 字段里（本机实测原样本见该任务 research）。文案优先级：`errors[]` > `result` > subtype 兜底。裸英文只进 `<details>`，主文案由 `errors::classify` 给可操作中文。
   - **唯一的豁免是「用户中断」，且必须加在这条判据之前、不得削弱它**（B1 起，`stream::claude::result_is_user_abort`）：被 `interrupt` 打断的轮次实测形态是 `is_error: true` / `subtype: "error_during_execution"` / `terminal_reason: "aborted_streaming"` / `result: null` / `modelUsage: {}` / `errors: ["[ede_diagnostic] …"]`。判据**只认 `terminal_reason`** 这一个明确机器码 —— 放宽成「`error_during_execution` 就当取消」会把真实的执行失败一起吞掉，而未登录那条样本（`terminal_reason: "api_error"`）仍必须报错。不豁免的表现：用户每点一次「停止」都在「已取消」之后多一个假错误气泡，`stream_outcome` 还会从 cancelled 翻成 error。

21. **claude 的 stream-json 与官方 Agent SDK 是同一条协议，`sdk.d.ts` 当文档用而非依赖**。实测反编 SDK 的 `sdk.mjs`：它 spawn 的命令行是 `--output-format stream-json --verbose --input-format stream-json`，与 Kivio 的 `build_claude_args` 完全一致（paseo 的 `query.ts` 也只是劫持其 spawn 钩子换可执行文件）。**不要为了「对齐参考实现」引入 Node/SDK 依赖**：语言边界代价不成比例（打包 Node 运行时 +40MB，或 Rust→Node→claude 三层套壳），且直接解析原始流是**超集** —— `usage.iterations[]` 末项就是 SDK 抽象层拿不到、而修对用量口径必需的字段。`sdk.d.ts`（MIT）作为 `SDKMessage` 变体的权威清单查阅即可。刻意不接的变体（如 `hook_started`/`hook_response`，其 `stdout` 会把整个 SessionStart 注入内容搬进流里刷屏）**必须写注释说明「有意不接」**，别让后人分不清是漏了还是故意的。
   - **真相源是本机装的那个二进制，不是记忆里的 d.ts**：`grep -a` 就能从 `claude` 可执行文件里读出 zod schema 与帧构造处（本机 `C:/Users/11028/.local/bin/claude`）。07-29 的教训：代码里三处注释断言「`compact_metadata` 只有 `trigger` 与 `pre_tokens`，**没有 post_tokens**，别去读一个不存在的字段」，而二进制里的构造处明写 `...postTokens!==void 0 && {post_tokens}` —— **这类「别去查」的注释最坑，照着走就永远不会发现它错了**。写这种断言时必须附上核实方式与版本号；发现过期就连注释一起改掉。
   - **给 CLI 加隐藏 flag 前用零副作用探针确认存在**（第 12 条的具体做法）：不带值跑一次，看报的是 `unknown option` 还是 `argument missing`，并跑一个胡编的 flag 作对照组。

22. **claude 还有三条「不接就静默丢信息」的通道**（07-29 补齐，均已核实自 2.1.220 二进制）：
   - **`system/local_command_output`**（`{subtype:"local_command_output", content:string}`，zod 描述原文 "Output from a local slash command (e.g. /voice, /usage)"）+ **成功 result 的 `result` 字段**：`/cost` `/usage` `/context` `/status` `/doctor` 由 claude **客户端执行**，没有模型往返，报告正文只走这两条。两条都不接的话用户点了 `/cost` 只会看到 `run.rs` 的兜底「claude 命令已执行」——**报告内容 100% 丢失**，而我们是主动把 claude 的斜杠命令列表暴露给用户的（`slash.rs` 从 init 解析），这条路一定会被走到。`result` 兜底的三条判据必须同时成立：本轮成功 + `usage.output_tokens == 0` + **本轮还没发过任何正文**（第 3 条的 `any_text_emitted`），否则会把正常回答重复一遍。
   - **assistant 帧的 `error` / `aborted`**（挂在**帧顶层**、与 `message` 平级，不在 `message` 里面）：取值 `authentication_failed` / `oauth_org_not_allowed` / `billing_error` / `rate_limit` / `overloaded` / `invalid_request` / `model_not_found` / `server_error` / `unknown` / `max_output_tokens`。`max_output_tokens` 最典型 —— 回答**中途硬截断**而 `result` 那边可能仍是成功（错误只写在 assistant 帧上），不接就把半截回答标成「完成」。截断 / `aborted` 走 `TextDelta` 提示（正文仍有效，判成整轮失败会连带丢掉已流出的内容）；认证 / 计费 / 上游故障走 `Error` → `errors::classify`（第 5 条）。同一失败会在多条 assistant 帧上重复，需 per-turn 去重。注：`authentication_failed` / `oauth_org_not_allowed` 是**机器码而非自然语言**，`errors::detect_kind` 必须显式认它们，否则落进 Protocol、用户拿不到 `claude /login`。
   - **`system/status` 的 `compact_result` / `compact_error`**（schema：`{status, permissionMode?, compact_result:"success"|"failed", compact_error?}`）：压缩**失败**时 claude 发的是 `compact_result:"failed"`，**不会**发 `compact_boundary` —— 不接就让「⏳ 正在压缩上下文…」永远停在那里，用户以为还在跑。同一分支里 `compact_result` 优先于 `status:"compacting"`（一条 status 不能既报正在压缩又报失败）。`status` 仍**按开放字符串处理、不做白名单**，但理由是「对未来新增取值的防御」——不再是此前注释所称的「`requesting` 不在官方类型里」（2.1.220 的 `SDKStatus` 里有 `requesting`）。
   - **CLI 自压必须落盘，不只是发事件**：`compact_boundary` → `CliCompacted` 事件 → `run.rs` 既要发 `chat-compaction`（前端立刻插分隔线）**又要**把同一条 `CompactionBoundaryRecord`（同一个 id！）写进 `context_state.compaction_boundaries` + `compression_count += 1` + `last_compressed_at`，写在 `push_assistant_message` **之前**（它会整体重算 `context_state`，而外部路径的重算会把这三项原样带过去）。此前只发事件不落盘 ⇒「已压缩 N 次」永远不涨、刷新后分隔线消失。另外 `display_after_message_id` **必须是个能解析到的 message id**（取触发时刻的最后一条消息，与内置 `compaction.rs` 同语义）—— 前端 `resolveCompactionBoundaries` 对空锚点直接 `continue`，此前发的是空串，于是这条分隔线**一次都没渲染过**。

23. **前端工具卡的名字与字段名是两处各自独立的错配，改一处等于没改**（07-29，`ToolCallBlock.tsx`）。`toolRecordRawName` 原样返回工具名不归一化，而卡片里所有展示映射的 switch 分支写的是小写 snake_case —— claude 的内置工具是 PascalCase 的 `Read` / `Bash` / `Grep` / `Glob` / `Edit` / `WebFetch` / `TodoWrite` / `Task`，全部落进 `default`；即便名字对上了，我们读 `args.path` 而 claude 用 `file_path`（`NotebookEdit` 用 `notebook_path`）。结果 `getToolTarget` 返回 `''`，折叠行退到 `previewValue(arguments)` —— 一坨 220 字符截断的 JSON。修法是文件内的两个小前置：`toolRawName`（小写 + 去分隔符后查 `CLI_TOOL_NAME_ALIASES`，未命中返回小写原名）与 `toolPathArgument`（多字段兜底）。两条约束：**归一化只用于 switch 匹配**，`getToolName` 的 default 分支必须回落 `toolRecordRawName` 的原名（MCP 工具名 `mcp__server__toolName` 的大小写有意义）；别名表**只列本文件确实有对应分支**的工具（`Task` / `Skill` 之类保持原名走 default，显示工具本名比映射到语义不符的分支好）。**不要为此引入结构化 tool-schema 层** —— 没有那套契约，加了是纯负担。

## 子进程启动与二进制解析

> 来源：任务 07-26-local-cli-context-usage 的 G3 对齐。数字均为本机实测。

16. **拉起外部 CLI 一律走 `spawn::cli_command`，不要用 `Command::new`**。它会剥掉父会话身份变量（`CLAUDECODE` / `CLAUDE_CODE_ENTRYPOINT` / `CLAUDE_CODE_SSE_PORT` / `CLAUDE_AGENT_SDK_VERSION`）。Kivio 若从 Claude Code 内启动（开发时 `npm run dev`，或用户把 Kivio 挂在某个 agent 下），这些变量会**继承**进 Kivio 再泄漏给 CLI 子进程，让子进程以为自己嵌套在另一个会话里而拒绝启动（claude 报 "cannot be launched inside another session"）。本机实测这三个变量在 Claude Code 环境下确实都是 set 的。**探测路径同样要剥**——探测也是子进程，且失败会被 `AVAILABILITY_CACHE_TTL`(600s) 缓存，一次误判要 10 分钟才自愈。只清会话身份变量，**不动** `ANTHROPIC_*` 等凭据（子进程要用它们认证）；忘记剥不会编译报错、也不会立刻出错，只在特定启动场景炸，极难排查。

17. **二进制解析要遍历全部同名候选并逐个探活，且「能启动」就算存在**。`which` 的第一行可能是坏 shim（版本管理器切换残留、断链、丢执行权限），选中它不会立刻失败而是**等到跑轮次时**才炸，用户看到运行时报错而非「未安装」。故用 `which -a` 取全部候选、逐个探活取第一个能起来的。判据刻意宽松：**只有 spawn 阶段失败才算不存在**。本机实测的失败分类：丢执行权限 → `EACCES`（不存在）；断链/文件缺失 → `ENOENT`（不存在）；**装了但未登录 → spawn 成功、exit 非零（算存在）**；**空文件挂执行位 → spawn 成功、内核回退 `/bin/sh` exit 0（算存在）**；超时 → 算存在（进程都起来了）。只认零退出码会把未登录的 CLI 误判成未安装，**比原来的 bug 更糟**。（注：用 Python `subprocess` 试空文件会看到 `ENOEXEC`，那是 CPython 自己先拦的，内核/Rust 行为以 `probe_*` 单测为准。）

18. **探活结果必须缓存，key 用「路径 + mtime + size」**。`resolve_binary` 是回复热路径上唯一允许的前置调用，`run.rs` 的预算是**第 2+ 轮 <500ms**。单次 `--version` 实测开销差异极大：grok 6.6ms、claude 40ms，但 **kimi 328ms、pi 302ms**（Node 包装脚本冷启动）——不缓存的话 kimi 单这一步就 538ms，直接击穿预算。实测加缓存后 warm 路径全部 ≤4ms。用文件身份而非 TTL 做 key：换版本/重装/切版本管理器都会改 mtime 或 size ⇒ 自动失效重探，既没有「刚装好却要等」也没有「删了还认为在」的窗口。

## 测试约定

13. external_agents 的行为修复必须带可红→绿的单测（本轮新增 ~40 组均遵守）；持久路径优先抽纯函数测（assembler / classify / failure_action / build_*_params），live 测试一律 `#[ignore]` 门控。

15. **用量/窗口这类「数字对不对」的修复，单测挡不住形态错配——必须补真机 `#[ignore]` 测试**。本轮 cursor 的窗口解析单测全绿、生产 0/33，就是因为单测喂的是 `models.availableModels` 形态而真实 cursor 走 `configOptions`（见 14f）。真机测试要断言**可证伪的量**（如「cache 计入后 total > input+output」「窗口非空」），不要只断言「没崩」。CLI 认证失效/网络挂起属**环境**问题，应诚实 skip 并打印排查提示（如 `opencode run hi` 先自检），不要 fail——否则一个过期的 API key 会伪装成代码回归。

15b. **app 层（而非模块层）的真机验收走无头 probe 通道**：`scripts/probe-e2e.mjs`（`npm run probe:e2e`，需另一个终端 `npm run dev`）驱动 `chat/probe.rs`，走**与聊天窗口完全相同**的生成路径。第 28 条那三条硬判据在模块级 `live_tests` 里已有，但「同一个进程服了两轮」「取消之后会话还在」这类在 app 层此前只能靠手动数系统进程 / 翻磁盘会话 JSON。probe 的探点与它们的用途：
   - `cancelAfterMs` —— **必须复用 `AppState::cancel_chat_generation`**（`chat_cancel_stream` 背后那一个调用），另造一条取消路径测到的就不是生产行为。计时从「本轮 generation 真正登记」起算：该函数只是清空活跃集合，早于登记调用是空操作、取消静默失效（表现为一轮正常完成的假绿）。
   - `liveSession.{childPid,turnsServed,idleMs,registrySize}` —— 注册表**只读**自省。`LiveSession.child_pid` / `turns_served` 是**纯元数据**，注册表不拿它们做任何决策（关停一律走 actor 的 `Close`，**绝不按 pid 杀**）；自省不得改 `last_activity`（改了等于把空闲回收的时钟拨回去），也不得跨 await 持锁。
   - `usage`（全字段）+ `computeContextStats` → `contextState` —— 后者**必须复用 `chat_get_context_stats`**。`context_state` 平时只在用户点开用量条时才算，不主动算的话分子/分母全是空的（踩过）。这两项让第 14 系列的口径修复第一次能在 app 层断言：分母是 CLI 实报的 1000000（不是猜的 200K）、分子把 cache 算进去（实测 `input 2 + output 1 + cache 41865 = total 41868`）、`/cost` 那种零用量轮不把分子清零。
   - `liveUsageTicks` —— 生成**过程中**收到的实时占用推送（14i）。轮末的 `contextState` 只能证明结束后的数对不对；「用量条有没有在过程中跟着走」此前在 app 层完全断言不了。实现是 probe 侧 `app.listen("chat-context")`（收的就是聊天窗口收的那一条事件），**生产路径零旁路**。
   - `durationMs` / `segments` / `reasoning` —— 常驻收益（实测首轮约 7-10s、复用轮约 2-4s，差值就是那 3.2s 冷启动）与解析层修复的定位锚点。**「第 2 轮更快」只能软断言**：一轮的总耗时被模型延迟支配，硬断言必然偶发假红；硬判据是 `childPid` 相同 + `turnsServed` 递增。
   - **不要加「注入合成流帧」的能力**：那测的是解析器，解析器已有单测，加了只是维护负担。
   - probe 相关代码全部 `#[cfg(debug_assertions)]`，release 不含；probe 概念不得泄进生产路径。

