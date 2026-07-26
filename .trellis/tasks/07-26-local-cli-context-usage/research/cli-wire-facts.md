# 本地 CLI 上下文与用量：逐 CLI 对照 paseo（2026-07-26）

全部结论本机实测，非文档推断。每条都标了复现方式。

## 许可边界（先说清楚）

参考项目 **paseo 是 AGPL-3.0-or-later**（`packages/../package.json: "license": "AGPL-3.0-or-later"`），
Kivio 是 **GPL-3.0-or-later**。AGPL 代码**不可**并入 GPL-3.0 项目。

本任务只复用两类东西，两类都不构成代码借用：
1. **协议事实** —— 各 CLI 实际吐什么字段。已由本文档自行实测复现。
2. **公开规范** —— ACP 官方 RFD（agentclientprotocol.com）定义的 `usage_update`。

**不得**搬运 paseo 的代码、注释、命名、文件结构。实现自行设计。

---

## 0. 最重要的发现：ACP 有官方 usage 通道，Kivio 完全没接

ACP 官方 RFD「Session Usage and Context Status」定义了 `session/update` 的
`usage_update` 变体，**同时给出分子和分母**：

```json
{"jsonrpc":"2.0","method":"session/update","params":{
  "sessionId":"...",
  "update":{"sessionUpdate":"usage_update",
            "used":53000, "size":200000,
            "cost":{"amount":0.045,"currency":"USD"}}}}
```

- `used` (number, required) — 当前上下文里的 token
- `size` (number, required) — 上下文窗口总大小
- `cost` (object, optional) — 累计成本

字段是**平铺在 `update` 下**，不嵌套在 `usage` 对象里。

另有 `PromptResponse.usage`（标记 UNSTABLE）：
`inputTokens` / `outputTokens` / `cachedReadTokens` / `cachedWriteTokens` /
`thoughtTokens` / `totalTokens`。
（注：`reasoning_tokens` 于 2025-12-17 更名为 `thought_tokens`。）

**Kivio 现状**：`session/acp.rs` 的两处 `sessionUpdate` 分发
（`apply_acp_session_update` :719、`acp_apply_session_update` :828）
都**没有** `usage_update` 分支 —— 落到 `_` 被丢弃。
`format_acp_usage`（:642）只读 `inputTokens` + `outputTokens`。

**paseo 也没接**（其 `handleUsageUpdate` 是空实现 `void update`），
所以这一点上 Kivio 有机会做得比参考实现更好。

### 实测：哪些 CLI 真的发

| CLI | `usage_update` | `PromptResponse.usage` |
|---|---|---|
| **opencode** | ✅ **发** | ✅ 发，含 `cachedReadTokens` |
| kimi | ❌ 不发 | ❌ 只有 `{"stopReason":"end_turn"}` |
| cursor | ❌ 不发 | ❌ 同上 |

**opencode 实测原始数据**（复现：`/tmp/acp_usage_probe.py opencode opencode acp`）：
```
prompt result: {"stopReason":"end_turn","usage":{"inputTokens":11685,"outputTokens":4,
                "totalTokens":13492,"thoughtTokens":11,"cachedReadTokens":1792}}
usage_update : {"sessionUpdate":"usage_update","used":13477,"size":200000,
                "cost":{"amount":0,"currency":"USD"}}
```

对账：
```
used 报 13477
input+output          = 11689   ← Kivio 当前口径，偏低 13%
input+cache+out+thght = 13492   = totalTokens
```
`used`(13477) 与 `totalTokens`(13492) 差 15，量级一致（`used` 是上下文占用，
`totalTokens` 是本轮计费总量，口径微差合理）。

**结论：opencode 的分子分母都能从 ACP 直接拿到，`size:200000` 就是窗口。**
不需要 `opencode models --verbose`，也不需要 paseo 那套 `opencode serve` HTTP 服务。

---

## 1. claude — `claude -p --output-format stream-json`

Kivio 已传 `--include-partial-messages`（`defs/claude.rs:31`），
`stream_event` 已解析（`stream/claude.rs:41`）。

**`result.usage` 真实形状**（实测 `claude -p ... --output-format stream-json --verbose`）：
```json
{"input_tokens":0,"cache_creation_input_tokens":0,"cache_read_input_tokens":0,
 "output_tokens":0,"iterations":[],"server_tool_use":{...},"service_tier":"standard"}
```
同层另有 `modelUsage`，形状 `{"<model-id>":{"contextWindow":200000,...}}`。

**Kivio 漏的**（`stream/claude.rs:127` 只取 `input_tokens` + `output_tokens`）：
- `cache_read_input_tokens` —— 缓存命中照样占窗口。长会话里是大头。
- `cache_creation_input_tokens` —— 同理。
- `iterations[]` —— 一轮内多次 LLM 往返，每次 usage 是**独立快照**。
  当前上下文占用 = **最后一项**。累加得到的是计费总量，不是上下文占用。
- `modelUsage[*].contextWindow` —— CLI 自报窗口。

**`message_start` 分支**（:170）只取 `message.id`，`message.usage` 整个跳过。
该字段是服务端算出的本次请求真实上下文（系统提示 + 工具定义 + 全历史 + cache），
且在**回答开始前**就到，能让进度条在生成过程中就准确。

> 本机嵌套 claude 未登录，取不到非零样本；字段名结构由上面那条真实 `result` 行确认（键都在，值为 0）。

**paseo 做法**：`includePartialMessages: true` + `ClaudeContextUsageState`，
`message_start` 累加 input 三件套、`message_delta` 刷 output，
`extractContextWindowSize` 从 `modelUsage` 取各模型 contextWindow 的 **max**（一轮可能混用多模型）。

---

## 2. codex — `codex app-server` JSON-RPC

**`thread/tokenUsage/updated` 形状**（现有单测样本即此形状）：
```json
{"tokenUsage":{
  "last": {"cachedInputTokens":0,"inputTokens":5,"outputTokens":7,
           "reasoningOutputTokens":0,"totalTokens":12},
  "total":{...同结构...}}}
```

**Kivio 现状**（`session/codex_app_server.rs:117`）：读 `tokenUsage.total`。
- `total` = 整个 thread **累计**消耗，随轮次单调增长（计费口径）
- `last` = 最近一次请求的快照（上下文占用口径）

用 total 当「已用上下文」会持续虚高，最终把进度条推满而实际远未满。

`reasoningOutputTokens` **不额外计入** —— 样本 5+7=12 说明已含在 output 内。

**窗口**：`codex debug models` 的 `context_window`（**实测 272000**），
Kivio 已正确解析（`detection.rs:519`）。**codex 是目前唯一分母正确的 CLI。**

**实测确认**：`codex app-server` 的 `model/list` 结果**不含**窗口字段
（keys: `id, model, upgrade, upgradeInfo, availabilityNux, displayName, description,
hidden, supportedReasoningEfforts, defaultReasoningEffort, inputModalities,
supportsPersonality, additionalSpeedTiers, serviceTiers, defaultServiceTier, isDefault`）。
→ 保持走 `debug models` 是对的，**不要**改成 app-server（paseo 走 app-server，
它另有 `usage.model_context_window` 来源）。

---

## 3. opencode

见第 0 节 —— ACP 直接给 `used` + `size`，这是最优解。

**备选（不需要了，留档）**：`opencode models --verbose` 每个模型跟一段 JSON，
含 `limit.context`。实测 20/20 有值：`opencode/big-pickle 200000`、
`nemotron-3-ultra-free 1000000`、`kimi-for-coding/k3 1048576`、`ling-3.0-flash-free 262144`。
若将来要在**选模型时**（还没发消息、拿不到 usage_update）显示窗口，可用这条。

---

## 4. pi — `pi --mode rpc`

**我之前判断错了**。原以为 pi 无需改动，实测发现 pi 也有 cache 字段。

**pi 会话文件**（`~/.pi/agent/sessions/<slug>/<ts>_<uuid>.jsonl`）里的 usage：
```json
{"input":6571,"output":1578,"cacheRead":4096,"cacheWrite":0,"reasoning":26,
 "totalTokens":12245,"cost":{...}}
```

**Kivio 现状**（`session/pi_rpc.rs:167`）：只读 `input` + `output`。
漏 `cacheRead` / `cacheWrite`。实测样本里 `cacheRead=4096` 占 input 的 62%。

注意 pi 的 `totalTokens`(12245) ≠ input+output+cacheRead(12245)... 实际
6571+1578+4096 = 12245 ✓ 正好吻合，说明 `reasoning` 已含在 output 内。

**窗口**：`pi --list-models` 定宽文本表第 3 列（`128K`），
Kivio 已正确解析（`pi_rpc.rs:136` → `parse_context_window_label`）。**分母 OK。**

---

## 5. cursor — ACP，窗口藏在 modelId 字符串里

**实测 `session/new` 返回**：
```json
{"models":{"currentModelId":"composer-2.5[fast=true]","availableModels":[
  {"modelId":"claude-opus-5[thinking=true,context=300k,effort=high,fast=false]","name":"claude-opus-5"},
  {"modelId":"gpt-5.6-sol[context=272k,reasoning=medium,fast=false]","name":"gpt-5.6-sol"},
  ...]}}
```

**窗口写在 modelId 的方括号参数里**：`context=300k` / `context=272k` / `context=200k`。
实测 32 个模型中 **13 个**带 `context=` 提示。无 `_meta.totalContextTokens`。

Kivio 的 `normalize_models`（`session/acp.rs:168`）只认
`_meta.totalContextTokens`（grok 那种），完全忽略 modelId 里的这个提示。

现成工具：`external_agents/context.rs:22` 的 `parse_context_window_label`
已能解析 `"300k"` → 300000。只差把它从 modelId 里抠出来。

cursor 不发 `usage_update`（实测），所以分子仍无来源。

---

## 6. kimi — ACP 上游确实什么都不给

**实测 `session/new`** 返回 `configOptions` 形态：
```json
{"id":"model","currentValue":"kimi-code/k3-256k","options":[
  {"value":"kimi-code/kimi-for-coding","name":"K2.7 Coding"},
  {"value":"kimi-code/k3","name":"K3"},
  {"value":"kimi-code/k3-256k","name":"K3-256k"}]}
```
**无任何 token/窗口字段。** `session/prompt` 结果只有 `{"stopReason":"end_turn"}`，
**无 usage**，**无 usage_update**（实测）。

`acp.rs:151` 的 `configOptions` 分支写死 `context_window_tokens: None` 是忠实反映上游。

### 替代数据源：wire.jsonl（实测可用）

`~/.kimi-code/sessions/<wd_hash>/<session_id>/agents/main/wire.jsonl`：
```json
{"type":"usage.record","model":"kimi-code/k3-256k",
 "usage":{"inputOther":565,"output":228,"inputCacheRead":23040,"inputCacheCreation":0},
 "usageScope":"turn","time":1784987956825}
```
真实 input = `inputOther + inputCacheRead + inputCacheCreation` = 23605。
对照 Kivio 当前显示的 `~24` —— 差三个数量级。
`inputCacheRead` 占 97.6%（23040/23605），**再次印证漏 cache 就是漏一个数量级**。

**关联方式（关键，已实测验证）**
不能用 Kivio 存的 session id —— kimi 走 ACP，session id 由 kimi 生成，Kivio 没存
（实测 `external-agent-sessions/` 里 18 个 claude + 3 个 pi，**0 个 kimi**）。

改用 **workDir 关联**：`~/.kimi-code/session_index.jsonl` 每行
`{"sessionId","sessionDir","workDir"}`，其 `workDir` 恰好等于 Kivio 的
`resolve_effective_cwd()`（`chat-workspaces/<conversation_id>`）。

**必须跳过空壳会话**：实测某会话目录下 53 个 kimi session，**52 个是空壳** ——
Kivio 的 slash 探测每次 `session/new` 会在 kimi 侧留一个无 turn 的会话
（`.trellis/spec/guides/external-cli-agents.md` 第 11b 条已记录这个残渣来源）。
判据：wire.jsonl 里存在 `type=="usage.record"` 且 `usageScope=="turn"`。
有效候选按 wire.jsonl 的 mtime 取最新。

**三个 workDir 实测跑通**：
```
conv_2c0108ea.../  → session_08e3a7c8  input=23605  out=228  model=kimi-code/k3-256k
kivio(项目会话)     → session_f606e3a4  input=67728  out=519  model=kimi-code/k3-256k
__global__         → (无 usage，正确退回估算)
```

`usage.record` 自带 `model` 字段，可用于窗口映射。
`KIMI_CODE_HOME` 环境变量优先于 `~/.kimi-code`。

---

## 7. gemini / hermes / grok

- **gemini / hermes**：本机未安装，无法实测。同走 ACP，
  按第 0 节的通用 `usage_update` 处理即可 —— 发就用，不发就退回。
- **grok**：ACP，`models.availableModels[]._meta.totalContextTokens`
  已被 Kivio 正确解析（现有单测 `acp_models_default_slot_enriched_by_current_model_id`
  用 500000 覆盖）。**分母 OK。**未实测其是否发 `usage_update`。

---

## 8. 分母兜底的现状缺陷

`chat/model_metadata.rs:360` `context_window_for_model` 的关键词表：
`1m / 200k / 128k / 100k / 64k / 32k / 16k / 8k` —— **没有 `256k`**。
`k3-256k` 连名字里明写的都捞不到，掉到 `FALLBACK_CONTEXT_WINDOW_TOKENS = 200_000`。

`modelDatabase.json` 键是 `kimi-k3`，kimi ACP 报 `kimi-code/k3`；
`model_metadata.rs:52` 按 `/` 切出 `k3` 后前缀/包含匹配全落空。

**外部 CLI 场景下这个兜底有害**：假 200K 会让 `usage_ratio` 算出假百分比，
进而在错误的点触发压缩阈值。

**paseo 的处理**：前端 `resolveContextWindowValues` 在 max/used 缺任一时
两个都置 null，`renderContextWindowMeter` 直接 `return null` —— **整个上下文条不渲染**。
不编造。Kivio 的 `ContextIndicator` 已具备等价能力
（`windowLabel`→`contextTokensUnknown`、`showThresholdMarkers`→false），只是后端没送 `None`。

---

## 9. 分子估算兜底的现状缺陷

`external_agents/context.rs:82` `collect_external_session_usage` 二级兜底：
拼所有 `message.content` 跑 `estimate_tokens`（ASCII 4 字符/token，CJK 1 字符/token）。

不计：CLI 自己的 system prompt、工具定义、tool call 参数与返回、图片附件。
→ 系统性偏低，不是几个百分点的误差。

---

## 10. 汇总：每个 CLI 缺什么

| CLI | 分子现状 | 分子应改 | 分母现状 | 分母应改 |
|---|---|---|---|---|
| claude | 漏 cache + 漏 iterations 末项 | 补四字段 + 末项 + `message_start` 实时 | 别名表 ✅ | （`modelUsage` 可选校正） |
| codex | 读 `total`（累计，虚高） | 改读 `last` + `cachedInputTokens` | `debug models` ✅ | — |
| opencode | 只读 input+output（偏低 13%） | 接 `usage_update.used` | ❌ 兜底 200K | 接 `usage_update.size` |
| pi | 漏 `cacheRead`/`cacheWrite` | 补两字段 | `--list-models` ✅ | — |
| cursor | 无来源 | （ACP 不发，只能估算） | ❌ 兜底 200K | 解析 modelId 的 `context=300k` |
| kimi | 估算，低三个数量级 | 读 wire.jsonl | ❌ 兜底 200K | 静态映射表 |
| grok | 未测 | 通用 `usage_update` | `_meta` ✅ | — |
| gemini/hermes | 未装 | 通用 `usage_update` | 未知 | 通用 `usage_update.size` |

**共同根因**：`stream/mod.rs:38` 的 `usage_from_numbers(input, output)`
是所有外部 CLI 的唯一 usage 构造入口，签名本身把 cache 挡在门外。
四个调用点（claude / codex / pi / acp）全部受限于它。

## 复用既有口径（不要新造）

Kivio 内置路径已有正确的 cache 求和 —— `chat/agent/context_estimate.rs:24`
`anchor_total_tokens` 的 anthropic 分支：
```rust
input + output + cached_input_tokens + cache_creation_input_tokens
```
外部 CLI 应与之同口径。`ModelUsage`（`chat/model/types.rs:210`）
已有 `cached_input_tokens` / `cache_creation_input_tokens` 字段 ——
**不需要改结构**，只是外部侧没填。

## 复现脚本

- `/tmp/acp_usage_probe.py <name> <argv...>` —— ACP 通用 usage 探测
- `codex debug models | python3 -c "import json,sys; ..."` —— codex 窗口
- `opencode models --verbose` —— opencode 静态窗口（备选）
- `pi --list-models` —— pi 窗口表
- `claude -p "x" --output-format stream-json --include-partial-messages --verbose`
