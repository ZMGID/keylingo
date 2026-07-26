# External CLI Agents — 执行契约与约定

> 来源：任务 07-20-external-cli-overhaul（子任务 ce76f60 / 4214956 / 3487e05 / 3456997 的审计与修复）。
> 适用：`src-tauri/src/external_agents/**` 及其前端对接面（`src/chat/RuntimePicker.tsx`、`src/chat/api.ts`、`src/chat/Chat.tsx` 的运行时切换）。

## 消息链路（prompt.rs / session/acp.rs）

1. **只发最新消息，历史归 CLI 原生会话**（3456997 起）：`compose_external_prompt` 不重放 transcript——首轮 = instructions + 最新消息，resume 轮 = 仅最新消息。全部 9 个 CLI 均有原生会话（claude `--resume` / codex thread / ACP `session/load` 含 kimi / pi `--session-id`）。禁止任何形式的历史重放回归；fresh 重连丢上下文时必须发可见提示（TextDelta blockquote，cancelled 不发）。
2. **禁止全局文本前缀去重**：ACP assistant/thought 输出的去重走 `AcpTextAssembler`（按消息边界维护累积游标，`on_boundary` 只置位、`push_chunk` 的 starts_with 决定是否重置）。一次性驱动 `run_acp_session` 与持久驱动 `AcpSession::run_turn` 必须共用 `acp_apply_session_update`——不要再出现两份拷贝。
3. **流 parser 的 per-message 状态**：类似 `text_streamed` 的"已流式"标志必须在新消息开始（message_start / 新 message id）时复位，不能是整轮全局 bool。
3b. **会话-CLI 绑定**：有消息的外部会话禁切 kind/external_agent_id（后端 `check_runtime_switch_allowed` 纯函数 + 前端 locked）；model/reasoning/sandbox 放行。前端任何"回写运行时"的路径（如 draft 落地）必须与后端放行条件一致——只对空会话生效，否则被校验拒绝卡死发送。

## 会话生命周期（session/*、run.rs、errors.rs）

4. **持久会话必须排空 stderr**：任何 `Stdio::piped()` 的长活子进程都要 `spawn_stderr_tail`（环形 8KB），close/错误路径 join 取尾部。不排空会因管道写满挂死子进程。
5. **错误出口统一走 `errors::classify`**：气泡主文案 = 分类后的可操作中文（Auth 附 per-agent 登录命令表），原始错误 + 退出码 + stderr 尾部折叠进 `<details>`。新增错误路径不得把裸协议错误串直接落气泡。"401" 等数字判据必须 token 边界匹配。
6. **重试策略是纯函数**：`persistent_failure_action` —— cancelled 保留 handle；Auth 永不自动重试；transient 失败 fresh 重连恰好一次；NEEDS_RECONNECT（launch-flag 配置变更）重连恰好一次。改动重连逻辑先改这个纯函数和它的单测。
7. **handshake 错误带阶段前缀**（`spawn:`/`initialize:`/`session-new:`），超时常量 30s 起步，集中在文件顶部。
8. **中途换模型**：ACP 会话轮前 `session/set_config_option`/`set_model`（best-effort ack）；无对应 config 项的（reasoning 为启动 flag，如 grok）走 NEEDS_RECONNECT 重连。UI 所见必须与会话实际配置一致。
8b. **pi 轮次收尾（07-22 真机验收修复）**：`agent_end` 后进入 3s 宽限排空（到点主动 break，不无限等 EOF——pi 带 `--session-id` 收尾落盘时可能不因 stdin EOF 退出）；drain 返回 Ok 即 `start_kill` 子进程——pi 的会话落盘发生在每条 `message_end` 时（同步 `appendFileSync`），严格先于 `agent_end` 上线，kill 不丢会话、不影响下轮 resume。Unix 下信号退出 `status.code()=None`，不触发出口「非零退出+stderr」规则；**已知边界**：Windows `TerminateProcess` 退出码恒为 1，若该轮 pi 有任何 stderr 输出（如 node 弃用警告）仍可能被误判为 error——Windows 真机复现后需改为「协议层完成标志豁免非零退出规则」而非依赖退出码形态。

## 检测与模型探测（detection.rs、commands.rs、state.rs）

9. **回复路径零探测**：`run_external_cli_reply` 前置只允许 `resolve_binary`（毫秒级）。version/auth/模型探测只属于列表阶段和懒查命令。任何人不得把 `detect_single_agent`/`probe_models` 加回回复路径（audit N2：曾造成每轮 10-25s 延迟）。
10. **流式 reader 对非 JSON 行一律 `continue`**：任何逐行读子进程 stdout 的解析（探测/会话/命令发现）遇到 banner/日志行只跳过，绝不放弃整条流（audit 缺陷 3 的教训；detect_acp_models 曾用 `?` 硬退）。
11. **探测结果必须带来源**：`chat_detect_external_agent_models` 返回 `source: probed|fallback` + `probeError`；fallback 走 30s 负缓存（probed 300s），force 绕过。前端降级必须可见（角标 + 重试），禁止静默降级到静态表。**负缓存同样适用于斜杠命令探测**（07-22 probe-hygiene）：空列表按 `SLASH_COMMANDS_EMPTY_CACHE_TTL`（30s）负缓存、非空 300s（get 侧按 entry 空/非空裁定 TTL）——空结果不写缓存会导致切会话/切 agent 每次重探（kimi 每次探测落一个空壳会话）。
11b. **探测 cwd 统一走 `resolve_detection_cwd`**（非项目会话 = `chat-workspaces/__global__`，绑定项目的会话 = 项目根）：模型探测、斜杠命令探测，以及**所有读写探测缓存的 key**（`slash::cache_key(agent_id, detection_cwd)`）必须用同一 cwd——run.rs 运行时学到的斜杠列表写入、context.rs 的模型缓存读取都曾因 key 用执行 cwd 而与探测 key 分叉（恒 miss / 永远覆盖不了）。执行 cwd（`resolve_effective_cwd`，每会话独立 workspace）只用于真正跑轮次，不得用于缓存 key。斜杠探测本身也用 `detect_availability_single`（不连带 `probe_models`），探测残渣（CLI 落盘的空壳会话）只进 `__global__`。
12. **defs 静态表只是 fallback**：`fallback_models` 首项恒为 `default`（前端 `agent.models[0]` 依赖此契约）；运行时探测到的才是模型事实源。给 CLI 传 flag 前先 `--help` 核实语义（audit N5：pi 曾把目录塞进 `--append-system-prompt`）。**CLI 当前配置（current_model/current_reasoning）的本地配置读取**（codex `~/.codex/config.toml`、pi `~/.pi/agent/settings.json`、kimi `~/.kimi-code/config.toml`）只允许挂在 `detect_agent_models`（懒查模型探测路径，probe 成功分支），禁止进回复热路径；配置缺失/解析失败一律 `None`（前端显示「自动」），解析器必须有非法输入不 panic 的单测。

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

14e. **窗口拿不到时外部 CLI 路径返回 `None`，不得兜底 200K**。`context_window_for_external_model` 的优先级链：CLI 本轮实报（`usage_update.size`，最高——模型可能中途切换，比任何静态表都准）> 探测上报（`RuntimeModelOption.context_window_tokens`）> claude 别名表 > kimi 静态映射 > 数据库/关键词 > `None`。假分母比没有分母更有害：它让 `usage_ratio` 算出假百分比，进而在**错误的点**触发压缩阈值。`context_window_for_model` 自带的 200K 兜底对**内置**路径是合理的（那里 provider 元数据可靠）——**只在外部 CLI 这条路剥掉**，内置分支保持不变。前端 `ContextIndicator` 已能渲染窗口未知（`? Token` + `满度未知`，不显示百分比与阈值刻度）；注意区分「窗口永远拿不到」与「用量待上报」两种文案，前者用 `contextFullnessWindowUnknown`。

14f. **窗口来源是 per-CLI 的，别假设统一形态**：codex 走 `debug models` 的 `context_window`（**不是** app-server 的 `model/list`，实测那里没有窗口字段）；grok 走 ACP `_meta.totalContextTokens`；pi 走 `--list-models` 定宽表第 3 列；**cursor 把窗口写在 modelId 的方括号里**（`claude-opus-5[thinking=true,context=300k,effort=high]`，实测 33 个模型 14 个带 `context=`，且**全都没有** `_meta`），且它的模型走 **`configOptions` 分支**而不是 `models.availableModels`——那条分支命中后会提前 return，只给 availableModels 加解析在生产里等于没做（实测 0/33，单测却是绿的，只有真机 `#[ignore]` 测试才抓到）。kimi 的 ACP 上游什么都不给（`session/new` 无 token 字段、`session/prompt` 无 usage、不发 `usage_update`），只能静态映射 + 读其 `wire.jsonl`；后者按 **workDir** 关联（kimi `session_index.jsonl` 的 `workDir` 恰等于 `resolve_effective_cwd()`，**不能**用 session id——kimi 走 ACP，id 由它自己生成，Kivio 根本没存），且必须**跳过空壳会话**（实测某 workDir 下 53 个 session 有 52 个是第 11b 条所述的 slash 探测残渣，判据：存在 `type=="usage.record"` 且 `usageScope=="turn"`）。

## 测试约定

13. external_agents 的行为修复必须带可红→绿的单测（本轮新增 ~40 组均遵守）；持久路径优先抽纯函数测（assembler / classify / failure_action / build_*_params），live 测试一律 `#[ignore]` 门控。

15. **用量/窗口这类「数字对不对」的修复，单测挡不住形态错配——必须补真机 `#[ignore]` 测试**。本轮 cursor 的窗口解析单测全绿、生产 0/33，就是因为单测喂的是 `models.availableModels` 形态而真实 cursor 走 `configOptions`（见 14f）。真机测试要断言**可证伪的量**（如「cache 计入后 total > input+output」「窗口非空」），不要只断言「没崩」。CLI 认证失效/网络挂起属**环境**问题，应诚实 skip 并打印排查提示（如 `opencode run hi` 先自检），不要 fail——否则一个过期的 API key 会伪装成代码回归。
