# 设计：本地 CLI 上下文用量与检测修复

## 边界

### 许可

paseo 是 **AGPL-3.0-or-later**，Kivio 是 GPL-3.0-or-later —— 不可并入。
本设计只用两类外部输入，两类都不是代码借用：

1. **协议事实** —— 各 CLI 实际吐什么字段，已由 `research/cli-wire-facts.md` 自行实测复现
2. **ACP 官方公开规范** —— agentclientprotocol.com 的 `usage_update` RFD

结构、命名、注释全部自行设计，复用 Kivio 既有约定。

### 必须遵守的既有契约

`.trellis/spec/guides/external-cli-agents.md`：
- 第 2 条：两处 ACP `sessionUpdate` 分发共用同一份逻辑，**禁止两份拷贝**
- 第 3 条：per-message 状态在新消息开始时复位（`message_start` 现有两行不能动）
- 第 9 条：回复热路径零探测 —— 新增来源不得引入子进程
- 第 10 条：流式 reader 遇非 JSON 行 continue，不放弃整条流
- 第 13 条：行为修复必须带可红→绿单测

## 核心设计决策：窗口怎么从流传到上下文状态

`usage_update` 同时给 `used`（分子）和 `size`（分母）。分子有现成通道
（`UnifiedAgentEvent::Usage` → `run.rs:1197` → `message.usage`），**分母没有**。

三个选项：

| 方案 | 代价 |
|---|---|
| A. 新增 `UnifiedAgentEvent` 变体 | 牵动 4 处 `match`（含两个测试里的 `event_variant` 穷举），且要新开一条到 context 的路 |
| B. 存进 `ConversationContextState` | 流解析层拿不到 conversation，要穿透多层 |
| **C. 给 `ModelUsage` 加可选字段** | 一个 `#[serde(default)] Option<u64>`，**零 match 改动**，随 message 持久化 |

**选 C。** 理由：
- `ModelUsage` 已经是「CLI 报什么就存什么」的载体，窗口是同一次上报的一部分
- 走现有 `Usage` 事件通道 → `run.rs` 的 `*usage = Some(u)` 后到覆盖先到（正好是「取最新快照」语义）
- `collect_external_session_usage` 本来就在读 `message.usage`，顺手能读到窗口
- 内置路径不填这个字段 → `None` → 行为完全不变
- 旧会话历史反序列化时 `serde(default)` 给 `None`，不 panic

```rust
// chat/model/types.rs ModelUsage 追加
/// CLI 自报的上下文窗口总大小（ACP `usage_update.size`）。
/// 仅外部 CLI 填；内置 provider 恒 None。
#[serde(default)]
pub context_window_tokens: Option<u64>,
```

## 分层

```
L0 usage 构造入口          ← 共同根因，全部前置
├─ L1 ACP usage_update     → opencode(实测发) + gemini/hermes/grok(未测,自动覆盖)
├─ L2 ACP PromptResponse   → 同上，补 cache/thought
├─ L3 claude               → cache + iterations 末项 + message_start 实时
├─ L4 codex                → last 而非 total + cachedInputTokens
├─ L5 pi                   → cacheRead/cacheWrite
├─ L6 cursor               → modelId 里的 context=300k
└─ L7 kimi                 → 静态窗口映射 + wire.jsonl 真实用量
L8 关键词表补 256k          (独立)
L9 外部不编造窗口 + 优先级   ← 收口，依赖 L1/L6/L7 先把能拿到的补齐
```

实施序：**L0 → L1 → L2 → 跑测 → L3 → L4 → L5 → 跑测 → L6 → L7 → L8 → L9 → 全量**

L9 最后，避免中间态大面积变「未知」。

---

## L0 — usage 构造入口（`external_agents/stream/mod.rs`）

现状 `usage_from_numbers(input, output)` 把 cache 挡在门外，四个调用点全受限。

新增一个字段完整的构造器。**口径必须与内置一致** ——
`chat/agent/context_estimate.rs:24` `anchor_total_tokens` 的 anthropic 分支是
`input + output + cached + cache_creation`，外部 CLI 同理。

```rust
pub struct CliUsageParts {
    pub input: u64,
    pub output: u64,
    pub cache_read: u64,
    pub cache_creation: u64,
    pub context_window: Option<u64>,
}
pub fn usage_from_parts(parts: CliUsageParts) -> ModelUsage
```

用 struct 而非 5 个位置参数 —— 5 个同类型 `u64` 连排太容易传错序，
且各 CLI 只填其中几项（`..Default::default()` 更清晰）。

`total_tokens = input + output + cache_read + cache_creation`。
**注释必须写明这不是笔误**，与 `anchor_total_tokens` 同口径 —— 否则后人会"修"掉它。

`usage_from_numbers` 保留（现有单测在用），内部改为委托 `usage_from_parts`，避免两份求和。

---

## L1 — ACP `usage_update`（`session/acp.rs`）

官方规范：字段**平铺在 `update` 下**，不嵌套。

```json
{"sessionUpdate":"usage_update","used":13477,"size":200000,
 "cost":{"amount":0,"currency":"USD"}}
```

**分发点有两处**，spec 第 2 条禁止拷贝：
- `apply_acp_session_update`（:719，一次性驱动，match 到 `_ => false`）
- `acp_apply_session_update`（:828，持久驱动，`_` 分支委托前者）

看代码：`acp_apply_session_update` 的 `_` 分支**已经**在调 `apply_acp_session_update`，
返回 `true` 表示"这是消息边界"。所以只需在 `apply_acp_session_update` 加分支即可自动覆盖两处。

但要注意返回值语义：`usage_update` **不是**消息边界，不该触发
`state.text.on_boundary()` / `state.thought.on_boundary()` ——
否则一条 usage 通知会把正在流式的正文游标重置，导致文本重复。

**→ 加分支后返回 `false`**（已处理但非边界）。这与现有 `true = 边界` 的语义有冲突。

**解法**：把返回值从 `bool` 改成语义明确的三态，或者更简单 —— 在
`acp_apply_session_update` 里**先**单独匹配 `usage_update`，不进 `_` 分支。
后者改动更小，且两处仍共用同一个 usage 解析函数（不违反第 2 条 —— 共用的是解析逻辑，
不是分发骨架）。

```
acp_apply_session_update:
  "agent_thought_chunk" => ...
  "agent_message_chunk" => ...
  "usage_update"        => sink(Usage { parse_acp_usage_update(update) })   ← 新增，不动游标
  _                     => 委托 apply_acp_session_update（边界语义不变）

apply_acp_session_update（一次性驱动）:
  "usage_update"        => sink(...); true    ← 这里 true/false 都行,一次性驱动无游标
```

抽 `parse_acp_usage_update(update) -> Option<ModelUsage>` 供两处调用。
`used` → `input_tokens`（它就是"当前上下文占用"），`size` → 新增的 `context_window_tokens`。
`cost` 暂不用（Kivio 无成本展示位）。

**`used` 语义要写清**：它**不是** input，是"上下文里现有的全部 token"。
放进 `input_tokens` 是因为下游 `collect_external_session_usage` 读的就是这个字段，
且对外部 CLI 而言"上下文占用"正是它要的。注释必须说明，否则会被误当成 prompt input。

---

## L2 — ACP `PromptResponse.usage`（`format_acp_usage`，:642）

实测 opencode：
```json
{"inputTokens":11685,"outputTokens":4,"totalTokens":13492,
 "thoughtTokens":11,"cachedReadTokens":1792}
```
对账 `11685+4+11+1792 = 13492 = totalTokens` → **四者并列不重叠**。

- `cachedReadTokens` → `cache_read`
- `cachedWriteTokens` → `cache_creation`
- `thoughtTokens` → `reasoning_tokens`（`ModelUsage` 已有该字段）
- 该字段 ACP 标记 UNSTABLE，缺失时全 0 → 返回 `None`（现有行为保留）

L1 的 `usage_update` 与 L2 的 `PromptResponse.usage` 都会到。
`run.rs` 是后到覆盖先到，`usage_update` 通常先于 prompt result ——
所以最终留下的是 L2 的值，**但 L2 没有 `size`**，窗口会丢。

**→ 必须处理合并**：L2 构造时若不知窗口，`context_window_tokens` 传 `None`；
`run.rs:1197` 的 `*usage = Some(u)` 改为**窗口字段粘滞**（新值为 `None` 时保留旧值）。
这是唯一需要碰 `run.rs` 的地方，逻辑三行。

---

## L3 — claude（`stream/claude.rs`）

### result 分支（:127）

- `usage.iterations` 非空 → 取**最后一个能解析成对象的元素**；否则用 `usage` 顶层
- 四字段：`input_tokens` / `output_tokens` /
  `cache_read_input_tokens` / `cache_creation_input_tokens`

`iterations` 语义：一轮内多次 LLM 往返，每项是**独立快照**。
当前上下文 = 末项。累加得到的是本轮计费总量。

### message_start 分支（:170）

- 现有两行**不动**（spec 第 3 条的 per-message 复位）
- 追加：`message.usage` 存在且非全零 → 发 Usage 事件
- 抽一个共用的四字段提取函数，与 result 分支共用

`message_start` 一轮内会多次触发，`run.rs` 后到覆盖先到 = 取最新快照 ✓

### 不做

`result.modelUsage[*].contextWindow` 窗口校正 —— claude 窗口经
`context_window_from_claude_model_alias` 已正确，收益不抵改动面（见 prd Non-Goals）。

---

## L4 — codex（`session/codex_app_server.rs:117`）

```
tokenUsage: { last: {…}, total: {…} }
```
- `last` = 最近一次请求快照（上下文占用）
- `total` = thread 累计（计费口径，单调增长 → 进度条虚高至满格）

改为优先 `last`，缺失退 `total`（旧版兼容）。
补 `cachedInputTokens`。**不读** `reasoningOutputTokens`（实测 5+7=12，已含在 output 内）。
codex 无 cache_creation 概念 → 传 0。

---

## L5 — pi（`session/pi_rpc.rs:167`）

实测 pi 会话文件 usage：
```json
{"input":6571,"output":1578,"cacheRead":4096,"cacheWrite":0,"reasoning":26,"totalTokens":12245}
```
对账 `6571+1578+4096 = 12245 = totalTokens` → **`reasoning` 已含在 output 内，不额外计**。

补 `cacheRead` → `cache_read`、`cacheWrite` → `cache_creation`。
实测 `cacheRead` 占 input 的 62% —— 漏掉严重低估。

pi 的窗口（`--list-models` 第 3 列）已正确，不动。

---

## L6 — cursor 窗口从 modelId 解析（`session/acp.rs::normalize_models`，:168）

实测 cursor 把窗口写在 modelId 的方括号参数里：
```
claude-opus-5[thinking=true,context=300k,effort=high,fast=false]
gpt-5.6-sol[context=272k,reasoning=medium,fast=false]
```
32 个模型中 13 个带 `context=`，且**无** `_meta.totalContextTokens`。

现有代码只认 `_meta.totalContextTokens`（grok 那种），忽略这个。

**复用现成工具**：`external_agents/context.rs:22` `parse_context_window_label`
已能解析 `"300k"` → 300000（`M`/`K` 后缀 + 浮点）。只差从 modelId 抠出 `context=<v>`。

优先级：`_meta.totalContextTokens` > modelId 的 `context=` > `None`。
（`_meta` 是显式字段，比字符串里的提示可靠。）

无提示的模型窗口留 `None`，**不猜**。

---

## L7 — kimi

### (a) 窗口静态映射（`external_agents/context.rs`）

上游 ACP 确实什么都不给（实测 `session/new` 无 token 字段、
`session/prompt` 无 usage、无 `usage_update`）→ 静态映射是唯一手段。

与既有 `context_window_from_claude_model_alias` 同构，紧邻放置。

表（来源：opencode `models --verbose` 实测 + kimi 官方文档，2026-07-26 核对）：
```
k3                        → 1_048_576
k3-256k                   →   262_144
kimi-for-coding           →   262_144
kimi-for-coding-highspeed →   262_144
```
容忍带/不带 `kimi-code/` 前缀（ACP 报带前缀，手填可能不带）。

**不改** `model_database_entry` 的通用匹配算法 —— 它服务所有 provider，
放宽（让 `k3` 匹到 `kimi-k3`）有误命中风险。

### (b) wire.jsonl 真实用量（新模块）

数据源：`<kimi_home>/sessions/<wd_hash>/<session_id>/agents/main/wire.jsonl`
```json
{"type":"usage.record","model":"kimi-code/k3-256k",
 "usage":{"inputOther":565,"output":228,"inputCacheRead":23040,"inputCacheCreation":0},
 "usageScope":"turn"}
```
input = `inputOther + inputCacheRead + inputCacheCreation`。
实测 `inputCacheRead` 占 97.6% —— 漏 cache 就是漏一个数量级。

**关联方式（关键，已实测验证）**

不能用 Kivio 存的 session id —— kimi 走 ACP，id 由 kimi 生成，Kivio 没存
（实测 `external-agent-sessions/` 18 claude + 3 pi + **0 kimi**）。

用 **workDir**：`<kimi_home>/session_index.jsonl` 每行
`{"sessionId","sessionDir","workDir"}`，`workDir` 恰好等于 Kivio 的
`resolve_effective_cwd()`（`chat-workspaces/<conversation_id>`）。

**必须跳过空壳会话**：实测某 workDir 下 53 个 session，**52 个空壳** ——
slash 探测每次 `session/new` 在 kimi 侧留一个无 turn 的会话
（spec 第 11b 条已记录该残渣）。判据：wire.jsonl 里存在
`type=="usage.record"` 且 `usageScope=="turn"`。有效候选按 mtime 取最新。

实测三个 workDir 跑通：`23605` / `67728` / `(无→退回估算)`。

- `KIMI_CODE_HOME` 优先于 `~/.kimi-code`
- 8MB 文件上限防御（实测最大 116KB）
- **全程只读**，任何失败静默 `None`
- 接在 `collect_external_session_usage` 的「CLI 实报」与「字符估算」之间，
  只对 `agent_id == "kimi"` 生效

**热路径检查（spec 第 9 条）**：这是读文件，不是起子进程。
`collect_external_session_usage` 由 `chat_get_context_stats` 调用（用户点开用量条），
不在回复热路径。✓

---

## L8 — 关键词表补 256k（`chat/model_metadata.rs`）

`known` 表 `1m/200k/128k/100k/64k/32k/16k/8k` 缺 `256k`。
插 `("256k", 262_144)`，位置在 `1m` 与 `200k` 之间保持降序。

值取 `262_144`（2^18）—— 依据 `modelDatabase.json` 里 kimi-k2.7 系列均为此值，
opencode 实测 `kimi-for-coding/k3-256k` 同量级。

---

## L9 — 窗口优先级 + 不编造（`external_agents/context.rs:41`）

### 优先级链（从可靠到不可靠）

```
1. CLI 本轮实报        message.usage.context_window_tokens（L1 的 usage_update.size）
2. 模型探测上报        RuntimeModelOption.context_window_tokens
                       （codex debug models / grok _meta / cursor modelId / pi 表）
3. claude 别名表       context_window_from_claude_model_alias
4. kimi 静态映射       L7(a)
5. 数据库 + 关键词     context_window_for_model（含 L8 的 256k）
6. None                ← 不再兜底 200K
```

第 1 级是新增的最高优先级 —— CLI 自报当轮窗口比任何静态表都准（模型可能中途切换）。

### 签名变更

`context_window_for_external_model` 返回 `(usize, bool)` → `(Option<usize>, bool)`。
调用点只有 `compute_external_context_state`（同文件 :134），
那里已有 `if context_window_tokens == 0 { None }` 分支，改 `Option` 后更直白。
编译器会抓全调用点。

窗口 `None` → `usage_ratio = None` → `external_context_status(None)` 已返回 `"unknown"`（现有逻辑）。

**内置路径（`chat/commands/context.rs`）完全不动** —— 那里 provider 元数据可靠。

### 前端

`ConversationContextState.context_window_tokens` 本就是 `Option<usize>`，
`ContextIndicator` 已处理 `null`：
- `windowLabel(null)` → `t.contextTokensUnknown`
- `showThresholdMarkers = (contextWindowTokens ?? 0) > 0` → false
- `fullnessLabel(null, true)` → `t.contextFullnessCliPending`

**预计不改代码**，但要实机看渲染 —— 确认「窗口未知」与「用量待定」两种状态文案不混淆
（`contextFullnessCliPending` 字面是"等待 CLI 上报"，用在"窗口永远拿不到"的场景可能误导）。
若误导则调文案。

---

## 数据流总览（改动后）

```
分子（已用 token）
  opencode  → usage_update.used ─────────────────┐
  claude    → message_start.usage / result末项    │
  codex     → tokenUsage.last                     ├→ ModelUsage(含 cache)
  pi        → turn_end.usage(含 cacheRead)        │   → message.usage
  kimi      → wire.jsonl usage.record             │   → collect_external_session_usage
  cursor    → (无来源)                            │      空则字符估算兜底
  其余 ACP  → usage_update(若发)                  ┘

分母（窗口）
  ACP 实报   usage_update.size          ← 最高优先级(新增)
  探测上报   codex context_window / grok _meta / cursor modelId(新增) / pi 表
  静态表     claude 别名 / kimi 映射(新增) / 数据库+关键词(补 256k)
  拿不到     → None → 前端不显示百分比(不再编造 200K)
```

## 兼容性

- `ModelUsage` 新增字段带 `#[serde(default)]` → 旧会话反序列化为 `None`，不 panic
- `usage_from_numbers` 保留 → 现有单测与未迁移调用点不受影响
- `UnifiedAgentEvent` **不新增变体** → 4 处 `match`（含 2 个测试的 `event_variant` 穷举）零改动
- `context_window_for_external_model` 签名变更 → 编译期抓全
- 前端契约字段不变，只是外部 CLI 场景 `context_window_tokens` 可能为 `null`（前端已支持）
- 旧会话的 `message.usage` 缺 cache → 按 0 处理，仍偏低，新一轮即准（可接受）

## 回滚

L0 是 L1-L5 的前置。L6 / L7 / L8 各自独立，单独 revert 即回到现状。
L9 单独回滚会退回编造 200K —— 若 L1/L6/L7 已上，回滚 L9 意义不大。
