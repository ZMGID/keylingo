# 对照 paseo 的剩余差距（2026-07-26）

> 承接任务 07-26-local-cli-context-usage 完成后的复盘。参考项目 paseo 为
> **AGPL-3.0-or-later**，Kivio 为 GPL-3.0-or-later —— 只提取「协议事实」与「设计取舍」，
> 不搬运其代码/命名/结构。
>
> 每条都标了「已实测确认」还是「读代码推断」。实测的给出复现方式。

## 判断基准

不是「paseo 有而 Kivio 没有」就该补。两者产品形态不同：
paseo 是多用户 daemon + 移动端，Kivio 是单机桌面应用。
下面每条都单独判断「值不值得做」。

**也要说清 paseo 更差的地方**（见最后一节），否则会误以为对齐它就是进步。

---

## G1 — claude 的 `compact_boundary` 完全没解析 【严重度：会看到错误数据】

**状态：已实测确认（代码路径 + 事件链路两侧都验过）**

claude CLI 自动压缩后会发 `{"type":"system","subtype":"compact_boundary", "compact_metadata":{trigger, pre_tokens, post_tokens}}`。

Kivio 侧：`src-tauri/src/external_agents/stream/claude.rs:74-82` 的 `system` 分支
**只处理 `subtype == "init"`**，其余全部落空。全仓 grep `compact_boundary` 无任何命中。

paseo 侧：`packages/server/src/server/agent/providers/claude/agent.ts:3739` 有
`compact_boundary` 分支，配 `readCompactionMetadata`（:4982，读 trigger/preTokens/postTokens，
兼容 camel/snake 两种写法）与 `buildCompactionUsageEvent`（:1870，**清空流式累加器**
`streamRequestInputTokens/OutputTokens` 后换成 `postTokens`）。

**实际影响（已验证链路）**：

> **修正（实现 G1 时发现，原判断偏重）**：我最初写「用量条会残留压缩前的数字」——
> 这条**基本不成立**。L3 已经在 `message_start` 上报服务端算的 `input_tokens`，
> 压缩后紧随的下一条 `message_start` 就带着压缩后的真实占用，
> `run.rs::merge_cli_usage` 后到覆盖 ⇒ **分子自愈**。已加单测
> `usage_recovers_from_message_start_after_compaction` 钉住（152340 → 8900）。
>
> 另一处修正：paseo 的 `readCompactionMetadata` 读 `postTokens`，但查
> `@anthropic-ai/claude-agent-sdk` 的 `sdk.d.ts` 可知
> `SDKCompactBoundaryMessage.compact_metadata` **只有** `trigger` 与 `pre_tokens`
> （全文件 grep `post_tokens` 命中 0）。**paseo 在读一个不存在的字段**，
> Kivio 不该跟着做。
>
> 所以 G1 的真实缺陷只剩下面第 2 条 —— 压缩这件事**完全不可见**。

1. ~~用量条残留压缩前的数字~~ —— 见上方修正，分子会自愈。
2. **压缩事件完全不可见**（这才是真缺陷）。前端 `Chat.tsx:1812` 监听
   `chat-compaction` 的 `phase === 'completed'` 来插 `CompactionDivider`，
   但 grep 确认 `external_agents/run.rs` **从不发**该事件 —— 它只在内置路径的
   `chat/agent/compaction.rs:1674` 与 `chat/commands/context.rs:914` 发出。
   claude 自动压缩后用户只会看到「对话突然变短了」而没有任何解释。
3. `external_agents/compact.rs:41-47` 的**手动** `/compact` 只自增
   `compression_count` + 重算，没有 boundary 记录，`compaction_boundaries`
   （`context.rs:278`）永远原样克隆、从不新增。

**本机数据**：扫了 57185 行本地 claude 历史，`system` 的 subtype 实测有 7 种
（`turn_duration` / `stop_hook_summary` / `away_summary` / `local_command` /
`informational` / `api_error` / `scheduled_task_fire`），**`compact_boundary` 出现 0 次**
—— 本机从未触发过压缩，所以无法实测该 payload，字段结构以官方 SDK 类型为准。
这也意味着该分支的正确性只能靠单测 + SDK 类型保证，标注在此。

**值得做：是。** 这是「用户会看到错误数字」那一类，且 claude 是 Kivio 用得最多的外部 CLI。
改动面：`stream/claude.rs` 加一个 `compact_boundary` 分支 + 一条新事件（或复用
`Usage` 事件把 postTokens 当快照灌进去）；若要补分隔线还需在 `run.rs` 发
`chat-compaction`。前者小，后者中等。

---

## G2 — codex 的 `modelContextWindow` 没读，分母用了静态表 【严重度：会看到错误数据（偏差 5%）】

**状态：已实测确认（真机 payload）**

复现：在 `codex_app_server.rs` 的 `"thread/tokenUsage/updated"` 分支临时插一行
`eprintln!`，跑 `cargo test --lib codex_usage_uses_last_snapshot_not_cumulative_total -- --ignored`。
实测输出（codex-cli 0.145.0，2026-07-26）：

```json
{"threadId":"019f9da9-...","tokenUsage":{
  "last": {"cacheWriteInputTokens":0,"cachedInputTokens":3456,"inputTokens":16865,
           "outputTokens":7,"reasoningOutputTokens":0,"totalTokens":16872},
  "modelContextWindow": 258400,
  "total":{...}},"turnId":"..."}
```

`modelContextWindow: 258400` 就在 Kivio **已经在解析的那个 payload 里**，平级于
`last`/`total`，但 `session/codex_app_server.rs:116-143` 只取了 `last`，没碰它。

Kivio 当前的 codex 分母来自 `codex debug models` 的 `context_window`（实测 **272000**），
走 L9 优先级链的第 2 级。真实值是 **258400** —— 偏高 13600（5.3%）。

paseo 侧：`codex-app-server-agent.ts:879` 的 `toAgentUsage` 读
`usage.model_context_window ?? usage.modelContextWindow` 作为 `contextWindowMaxTokens`。

**值得做：是，且很便宜。** L9 已经有「CLI 本轮实报」这一最高优先级通道
（`ModelUsage.context_window_tokens`，L1 为 ACP 建的），codex 这条只需在
已有的解析处多读一个字段填进去即可，**不需要新管道**。约 5 行 + 1 个单测。

顺带：`cacheWriteInputTokens` 字段也在 payload 里，Kivio 目前传 0
（design.md 里写的「codex 无 cache_creation 概念」是错的，实测有该字段，只是这次为 0）。

---

## G3 — 可执行文件检测：信 `which` 首行，无探活 【严重度：体验缺失，低概率但难排查】

**状态：已实测确认（代码 + 本机环境变量）**

Kivio：`external_agents/spawn.rs:88-117`。`resolve_binary` 遍历
`bin` + `fallback_bins`，每个调 `which_binary`，后者取 `which` 输出的**第一行**就返回，
**不做任何验证**。

paseo：`packages/server/src/executable-resolution/executable-resolution.ts:107-135`。
`findExecutable` 用 `which -a` 拿**全部**候选，逐个 `probeExecutable`（跑 `--version`）
直到有一个通过。其 `classifyProbeError`（:76）的判定很讲究：
- **非零退出码 → 视为「存在」**（`typeof err.code === "number"` 返回 true）
- 被 kill（超时）→ 也视为存在
- 只有 `ENOENT` / `EACCES` / `ENOEXEC` / `UNKNOWN` 才算不存在

理由：一个装了但**没登录**的 CLI，`--version` 完全可能非零退出。

**实际风险**：PATH 里有同名但坏掉的 shim（版本管理器残留、断掉的 symlink、
权限不足的文件）时，Kivio 会选中它然后在真正跑轮次时才失败，错误信息落在
`errors::classify` 里，用户看到的是运行时报错而不是「未安装」。
概率不高，但排查成本高。

**另一条更值得注意**：paseo 有 `PARENT_SESSION_ENV_VARS`
（`provider-launch-config.ts:206`）在构造子进程 env 时把
`CLAUDECODE` / `CLAUDE_CODE_ENTRYPOINT` / `CLAUDE_CODE_SSE_PORT` /
`CLAUDE_AGENT_SDK_VERSION` 显式置 `undefined`，注释写明原因：
daemon 若从 Claude Code 内启动，这些会泄漏给子进程导致
"cannot be launched inside another session"。

Kivio 的 `spawn.rs:130-140` 只做加法（`command.env(k, v)`），**没有任何剥离**。

**本机实测**：当前 shell 里 `CLAUDECODE`、`CLAUDE_CODE_ENTRYPOINT`、
`CLAUDE_AGENT_SDK_VERSION` **三个都是 set 状态**。也就是说从 Claude Code 里
`npm run dev` 起 Kivio、再让它拉起 claude 子进程，就会踩到。

**值得做：env 剥离值得（便宜且有真实触发场景），逐候选探活可选。**
env 剥离约 10 行 + 单测。探活会给每次检测加几百毫秒（每个候选一次
`--version`），但 Kivio 的可用性检测有 600s 缓存（`AVAILABILITY_CACHE_TTL`），
摊薄后可接受。

---

## G4 — 前端呈现：Kivio 显示「? Token」，paseo 整条隐藏 【严重度：纯设计取舍】

**状态：已实测确认（两边代码都读过）**

paseo：`packages/app/src/composer/index.tsx:810` 的 `resolveContextWindowValues`
在 max/used **缺任一**时把两个都置 `null`，`renderContextWindowMeter`（:225）
`if (!hasData && !pending) return null` —— **整个上下文条不渲染**。

Kivio（本轮改后）：显示 `~1.2K / ? Token · 满度未知`，不显示百分比与阈值刻度。

**我的判断：Kivio 现在这样更好，不要对齐 paseo。**
- 分子是有意义的（用户想知道「我用了多少」），只是没有分母算不出比例
- 整条隐藏会让用户以为功能坏了，而不是「这个 CLI 不报窗口」
- Kivio 是单机应用，信息密度可以比移动端高

**不值得做。** 记录在此是为了防止后来者「对齐参考实现」时把它改掉。

---

## G5 — `context_window_estimated` 标志位前端无人读 【严重度：低，属清理】

**状态：读代码推断（grep 全仓）**

`ConversationContextState.context_window_estimated`（`chat/types.rs:92`）
在 Rust 侧有维护（L9 返回 `None` 时同时返回 `true`），前端
`src/chat/types.ts` 有声明、`src/chat/api.ts:227` 硬编码 `true`，
但 **grep 全仓没有任何组件读它**。

它本可以用来区分「按模型名猜出来的窗口（如关键词表匹到 `256k`）」与
「CLI 确认的真实窗口（`usage_update.size` / `modelContextWindow`）」——
前者应当带个「估算」提示。现在这个信号是死的。

**值得做：低优先级。** 不是 bug，是一个已存在但没接的能力。
若将来要做「窗口来源可信度」提示，通道现成。

---

## 未完成核查的维度

诚实标注 —— 下面这些我没查完，不做结论：

- **会话生命周期**（列表/导入/resume/fork/rewind）：paseo 有
  `listImportableSessions` / `importSession` / rewind 三套能力，
  Kivio 有 `resolve_agent_resume_context` 但没有导入与 rewind。
  **没有评估这些对 Kivio 的产品形态是否有意义**，不下判断。
- **错误分类与降级**：paseo 的 provider status（ready/loading/error/unavailable）
  vs Kivio 的 `errors::classify` + `probe_error`/`source`。两边设计思路不同，
  需要专门一轮对照才能说清，本次没做。
- **gemini / hermes**：本机未安装，无法实测。它们走 ACP 通用通道（L1），
  理论上覆盖，但没验证过其是否真发 `usage_update`。

---

## paseo 比 Kivio 差的地方（不要盲目对齐）

1. **ACP usage 通道是空的**。`acp-agent.ts:2629` 的 `handleUsageUpdate`
   实现就是 `void update;` —— 它导入了 `UsageUpdate` 类型、接了 case、
   然后什么都不做。`mapACPUsage`（:561）也只映射三个字段、不含窗口。
   **Kivio 本轮做的 L1 比它完整**：opencode 实测能拿到 `used=13477 / size=200000`，
   paseo 拿不到。
2. **claude 走 Agent SDK 反而少拿了东西**。paseo 用
   `@anthropic-ai/claude-agent-sdk` 的 `query()`，只能消费 SDK 暴露的
   `SDKMessage`；Kivio 直接解析 `claude -p --output-format stream-json`
   的原始输出，理论上能拿到的**只多不少**（本轮的 `iterations[]` 末项就是一例）。
3. **静态模型清单**。paseo 的 `claude/model-manifest.ts` 是硬编码表，
   新模型上线要改代码。Kivio 走真实探测（`ModelProbeStrategy::ClaudeInit`），
   代价是 25s 超时，但不会过期。这是取舍不是优劣。

---

## 后续任务建议（按性价比排序）

| # | 事项 | 严重度 | 改动面 | 理由 |
|---|---|---|---|---|
| 1 | **codex 读 `modelContextWindow`**（G2） | 错误数据（5%） | 极小，~5 行 + 1 单测 | 字段就在已解析的 payload 里，L9 通道现成 |
| 2 | **剥离 `CLAUDECODE` 等父会话 env**（G3 后半） | 体验缺失 | 小，~10 行 + 单测 | 本机实测这些变量确实是 set 的，有真实触发场景 |
| 3 | **claude `compact_boundary`**（G1） | 错误数据 | 中，跨流解析 + 事件链路 | claude 是主力 CLI；若只做用量修正不做分隔线，改动面可减半 |
| 4 | 逐候选 `--version` 探活（G3 前半） | 体验缺失 | 中 | 概率低但排查成本高；有 600s 缓存摊薄开销 |
| 5 | 接上 `context_window_estimated`（G5） | 低 | 小 | 通道现成，等有「窗口可信度」需求时再做 |
| — | 对齐 paseo 的「缺数据整条隐藏」（G4） | — | — | **不做**，Kivio 现方案更好 |

建议 1 + 2 合并成一个小任务（都是「读已有 payload 里没读的东西 / 少传点东西」，
且都有明确实测依据）；3 单独一个任务，因为它牵涉事件链路设计。
