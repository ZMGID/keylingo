# 本地 CLI 上下文用量与检测修复

## Goal

把 Kivio 的**全部 9 个外部 CLI** 的上下文用量做对：分子（已用 token）不再系统性低估，
分母（窗口）不再编造。逐个 CLI 对照 paseo 的处理方式核查，但按各 CLI 的**实测协议事实**
自行实现 —— 拿得到真数就显示真数，拿不到就诚实地不显示。

## Background

用户实测：kimi 会话在 Kivio 显示 `~24 / 200.0K Token · 0% 已满`，
kimi CLI 自己显示 `context: 0% (0/256k)`。

顺着这条线逐 CLI 对照 paseo 排查（`research/cli-wire-facts.md`，全部本机实测），
发现这不是 kimi 单点问题，而是**每个 CLI 都在漏**，且有一个共同根因。

### 共同根因

`external_agents/stream/mod.rs:38`：

```rust
pub fn usage_from_numbers(input: u64, output: u64) -> ModelUsage
```

这是**所有外部 CLI 的唯一 usage 构造入口**，签名本身就把 cache token 挡在门外。
四个调用点（claude / codex / pi / acp）全部受限于它。

而 cache token **照样占上下文窗口**（只是不重复计费）。实测占比：
kimi `inputCacheRead` 占 97.6%、pi `cacheRead` 占 62%、opencode `cachedReadTokens` 占 13%。
漏掉它就是漏一个数量级。

### 最大发现：ACP 有官方 usage 通道，Kivio 完全没接

ACP 官方 RFD 定义了 `session/update` 的 `usage_update` 变体，
**同时给出分子和分母**（字段平铺在 `update` 下）：

```json
{"sessionUpdate":"usage_update","used":53000,"size":200000,
 "cost":{"amount":0.045,"currency":"USD"}}
```

**opencode 实测正在发**：`{"used":13477,"size":200000,...}`。
Kivio 的两处 `sessionUpdate` 分发都没有这个分支，落到 `_` 被丢弃。

paseo 在这一点上也没接（其 `handleUsageUpdate` 是空实现），
所以这是 Kivio 能做得比参考实现更好的地方。

## Requirements

### R0 — usage 构造入口支持 cache（共同根因，其余各项的前置）

- 提供能带 `cache_read` / `cache_creation` 的 usage 构造方式。
- 口径必须与内置路径一致 —— 复用 `chat/agent/context_estimate.rs:24`
  `anchor_total_tokens` 的 anthropic 分支语义：
  `input + output + cached_input_tokens + cache_creation_input_tokens`。
- `ModelUsage`（`chat/model/types.rs:210`）已有这两个字段，**不改结构**。
- 现有 `usage_from_numbers` 的调用方若确实只有两个数，可继续用，不强制迁移。

### R1 — ACP `usage_update` 通用通道（覆盖 opencode/cursor/kimi/gemini/hermes/grok 六个）

- 解析 `sessionUpdate == "usage_update"`，取平铺的 `used` 与 `size`。
- 两处 `sessionUpdate` 分发（`apply_acp_session_update`、`acp_apply_session_update`）
  必须**共用同一份**解析 —— 现有约定（spec 第 2 条）禁止两份拷贝。
- `size` 作为该会话的窗口来源，优先级高于静态映射与关键词兜底。
- `used` 作为分子，优先级高于字符估算。
- 不发这个通知的 CLI（实测 kimi / cursor 不发）行为不变，各自走其它来源。

### R2 — ACP `PromptResponse.usage` 补全字段

- `format_acp_usage` 现只读 `inputTokens` + `outputTokens`，需补
  `cachedReadTokens` / `cachedWriteTokens` / `thoughtTokens`。
- 注意 `thoughtTokens` 是否已含在 `outputTokens` 内需按实测判断
  （opencode 样本：11685+4+11+1792 = 13492 = `totalTokens`，说明四者并列不重叠）。
- 该字段在 ACP 中标记 UNSTABLE，缺失时不得报错。

### R3 — claude usage 口径修正

- `result.usage` 补 `cache_read_input_tokens` 与 `cache_creation_input_tokens`。
- `usage.iterations[]` 非空时取**最后一项**作为当前上下文占用；
  不得累加（那是计费口径），不得取首项。为空时退回顶层字段。
- `message_start` 的 `message.usage` 作为流式实时用量来源，
  使进度条在回答生成过程中即准确，而非等 turn 结束。
- `message_start` 现有逻辑（复位 `text_streamed`、记 `current_message_id`）
  不得改动 —— 那是 spec 第 3 条要求的 per-message 状态复位。

### R4 — codex 用 `last` 而非 `total`

- `thread/tokenUsage/updated` 改读 `tokenUsage.last`（最近一次请求的上下文快照），
  `total` 是整个 thread 累计消耗（计费口径），当已用上下文会持续虚高。
- `last` 缺失时才退回 `total`（旧版兼容）。
- 补 `cachedInputTokens`。
- 不读 `reasoningOutputTokens`（实测样本 5+7=12，已含在 output 内）。

### R5 — pi 补 cache 字段

- `turn_end.message.usage` 补 `cacheRead` / `cacheWrite`（实测 `cacheRead` 占 62%）。
- 不读 `reasoning`（实测 6571+1578+4096 = 12245 = `totalTokens`，说明已含在 output 内）。
- pi 的窗口来源（`--list-models` 第 3 列）已正确，不动。

### R6 — cursor 窗口从 modelId 解析

- cursor 的 ACP 把窗口写在 modelId 的方括号参数里：
  `claude-opus-5[thinking=true,context=300k,effort=high]`。
  实测 32 个模型中 13 个带 `context=` 提示，且无 `_meta.totalContextTokens`。
- 复用现成的 `context.rs:22` `parse_context_window_label`（已能解析 `"300k"`）。
- 无该提示的模型窗口留空，不猜。

### R7 — kimi 窗口静态映射 + 真实用量

**窗口**（上游 ACP 确实不提供，静态映射是唯一手段）：
- `kimi-code/k3` → 1048576；`k3-256k` / `kimi-for-coding[-highspeed]` → 262144
- 容忍带/不带 `kimi-code/` 前缀
- 必须标注数据来源与核对日期
- 不得为此放宽 `model_database_entry` 的通用匹配算法（服务所有 provider，有误命中风险）

**分子**：读 kimi 的 `wire.jsonl` 的 `usage.record`，
input = `inputOther + inputCacheRead + inputCacheCreation`。
- 关联方式：**workDir**（kimi 的 `session_index.jsonl` 的 `workDir` 恰好等于
  Kivio 的 `resolve_effective_cwd()`）。不能用 session id —— kimi 走 ACP，
  id 由 kimi 生成，Kivio 没存（实测 0 个 kimi 记录）。
- 必须跳过空壳会话（实测 52/53 是 slash 探测残渣，见 spec 第 11b 条）。
- `KIMI_CODE_HOME` 优先于 `~/.kimi-code`。
- 只读，任何失败静默退回估算。

### R8 — 关键词兜底表补 `256k`

- `chat/model_metadata.rs` 的 `known` 表缺 `256k`，导致名字里明写 `256k` 的模型
  都识别不出。补 `("256k", 262_144)`，保持降序。

### R9 — 外部 CLI 不再编造窗口

- 所有来源都拿不到窗口时，`context_window_tokens` 返回 `None`，
  而非 `FALLBACK_CONTEXT_WINDOW_TOKENS`（200K）。
- 假窗口会让 `usage_ratio` 算出假百分比，进而在错误的点触发压缩阈值。
- 前端在窗口未知时不显示百分比与阈值刻度。现有 `ContextIndicator` 已具备该能力
  （`windowLabel`→`contextTokensUnknown`、`showThresholdMarkers`→false），需验证生效。
- **内置（非外部 CLI）路径的兜底行为不变** —— 那里 provider 元数据可靠。

### R10 — 许可与协议边界

- 参考项目 paseo 为 **AGPL-3.0-or-later**，Kivio 为 GPL-3.0-or-later，
  **不得**并入其代码、注释、命名或结构。
- 只复用两类：(a) 各 CLI 实际输出什么字段 —— 已由本任务自行实测复现；
  (b) ACP 官方公开规范（agentclientprotocol.com 的 RFD）。
- 实现结构自行设计，复用 Kivio 既有约定。

### R11 — 遵守 external-cli-agents 既有契约

改动必须不违反 `.trellis/spec/guides/external-cli-agents.md`：
- 第 2 条：两处 ACP 分发共用同一份逻辑，不得出现两份拷贝
- 第 3 条：per-message 状态在新消息开始时复位
- 第 9 条：回复热路径零探测 —— 新增的窗口/用量获取不得引入子进程探测
- 第 10 条：流式 reader 遇非 JSON 行一律 continue，不放弃整条流
- 第 13 条：行为修复必须带可红→绿的单测

## Non-Goals

- 不为 opencode 引入常驻 HTTP 服务进程（paseo 走 `opencode serve` + SDK；
  实测 ACP 的 `usage_update` 已够用）。
- 不把 codex 的模型探测改成 app-server（实测其 `model/list` 不含窗口字段，
  现有 `debug models` 路径才有）。
- 不改 claude 的 `modelUsage[*].contextWindow` 窗口校正 ——
  claude 窗口经别名表已正确，且需新增事件通道牵动所有 CLI 的 match 分支，收益不抵改动面。
- 不动内置（非外部 CLI）聊天路径的上下文计算。
- 不重写可执行文件检测逻辑（逐候选 `--version` 探活、非零退出码视为「存在」、
  剥离 `CLAUDECODE` 等父会话 env）—— 独立议题，见 Notes。
- gemini / hermes 本机未安装，只通过 R1 的通用通道覆盖，不做专属处理。

## Acceptance Criteria

### 单测（每项都要可红→绿）

- [ ] AC1 `usage_update` 的 `used`/`size` 能被解析；两处分发都生效（不是只改一处）
- [ ] AC2 ACP `PromptResponse.usage` 的 cache/thought 字段计入；缺失不报错
- [ ] AC3 claude `result.usage` 含 cache 时计入；多个 `iterations` 取末项；
      `iterations:[]` 退回顶层
- [ ] AC4 claude `message_start.message.usage` 产出用量事件；
      不带 usage 时不产出且不影响现有 message_id 逻辑
- [ ] AC5 codex 构造 `last ≠ total` 的样本，断言取到 `last`；只有 `total` 时退回
- [ ] AC6 pi 的 `cacheRead`/`cacheWrite` 计入（造 cache 占大头的样本断言量级）
- [ ] AC7 cursor 的 `claude-opus-5[...,context=300k,...]` → 300000；无提示 → `None`
- [ ] AC8 `kimi-code/k3` → 1048576、`k3-256k` → 262144、不带前缀也对
- [ ] AC9 名字含 `256k` 的模型 → 262144
- [ ] AC10 外部 CLI 无任何窗口来源时 `context_window_tokens` 为 `None`、
      `usage_ratio` 为 `None`；claude/codex/pi（有窗口来源）不受影响
- [ ] AC11 kimi wire.jsonl 能解析出真实 usage；空壳会话被跳过；
      文件缺失/JSON 损坏 → 退回估算不 panic；cache 占大头的样本断言结果远大于 `inputOther`

### 全量

- [ ] AC12 `cargo test --manifest-path src-tauri/Cargo.toml --lib`
      对照基线 **1135 passed / 0 failed / 12 ignored**，不得新增失败
- [ ] AC13 `cargo test --manifest-path src-tauri/Cargo.toml`（含集成测试）
- [ ] AC14 涉及前端时：`npm run lint` + `npm run typecheck` + `npm test`

### 实机验证（必做，每个能装的 CLI 都要过）

- [ ] AC15 **opencode**：发一轮，窗口显示 200K（来自 `usage_update.size`），
      已用 token 与 `used` 一致（不是 input+output 的 11689）
- [ ] AC16 **kimi**：窗口显示 256K；已用为万级而非 `~24`；
      对照该会话 workspace 的 wire.jsonl 末条手算比对
- [ ] AC17 **claude**：长历史一轮，用量明显高于改动前（cache 计入后有量级变化）
- [ ] AC18 **codex**：连发 3 轮，用量**不是**单调累加
- [ ] AC19 **pi**：发一轮，用量高于改动前（cacheRead 计入）
- [ ] AC20 **cursor**：选一个带 `context=300k` 的模型，窗口显示 300K；
      选无提示的模型显示「未知」而非 200K
- [ ] AC21 回归：内置（非外部 CLI）聊天的上下文条行为无变化

## Notes

- 基线（改动前实测）：`cargo test --lib` → 1135 passed / 0 failed / 12 ignored。
  CLAUDE.md 提到 Windows 上 `--lib` 有 ~14 个预存 env/locale 失败；本机 macOS 是 0 失败。
- 可执行文件检测健壮性是**另一个议题**，本任务不含，记录备后续开任务：
  逐候选 `--version` 探活（现在信 PATH 第一个匹配）、
  非零退出码视为「存在」（装了但未登录的 CLI `--version` 可能非零）、
  剥离 `CLAUDECODE`/`CLAUDE_CODE_ENTRYPOINT` 等父会话 env（从 Claude Code 里启动时会泄漏）。
- gemini / hermes 未安装，R1 通用通道覆盖后若将来实测发现其行为不同，另开跟进。
- 全部实测事实与复现命令在 `research/cli-wire-facts.md`。
