# 执行计划：本地 CLI 上下文用量与检测修复

## 基线（改动前实测）

```
cargo test --manifest-path src-tauri/Cargo.toml --lib
→ 1135 passed; 0 failed; 12 ignored
```
**干净基线，收尾不得新增失败。**（CLAUDE.md 提到 Windows 上 `--lib` 有 ~14 个预存
env/locale 失败；本机 macOS 是 0，以本机为准。）

## 实施序

```
L0 → L1 → L2 → 跑测 → L3 → L4 → L5 → 跑测 → L6 → L7 → L8 → L9 → 全量 + 实机
```

L0 是 L1-L5 前置。L9 最后收口，避免中间态大面积变「未知」。

---

## L0 — usage 构造入口

**文件**：`external_agents/stream/mod.rs`

- [ ] 新增 `CliUsageParts { input, output, cache_read, cache_creation, context_window }`
      + `usage_from_parts(parts) -> ModelUsage`
      （用 struct 而非 5 个位置参数 —— 同类型 `u64` 连排易传错序）
- [ ] `total_tokens = input + output + cache_read + cache_creation`
- [ ] **注释写明**：与 `chat/agent/context_estimate.rs:24` `anchor_total_tokens`
      的 anthropic 分支同口径，**不是笔误**（否则后人会"修"掉）
- [ ] `usage_from_numbers` 保留，内部改为委托 `usage_from_parts`（避免两份求和）

**文件**：`chat/model/types.rs`

- [ ] `ModelUsage` 追加 `#[serde(default)] pub context_window_tokens: Option<u64>`
      + 注释：ACP `usage_update.size`，仅外部 CLI 填，内置恒 `None`

**验证**：`cargo test --lib` 现有 usage 相关测试全绿（应无影响）

---

## L1 — ACP `usage_update`

**文件**：`external_agents/session/acp.rs`

- [ ] 抽 `parse_acp_usage_update(update) -> Option<ModelUsage>`：
      读**平铺**的 `used` / `size`（官方规范：不嵌套在 `usage` 对象里）
- [ ] `used` → `input_tokens`，`size` → `context_window_tokens`
- [ ] **注释写明** `used` 的语义：它不是 prompt input，是"上下文里现有的全部 token"；
      放进 `input_tokens` 是因为下游 `collect_external_session_usage` 读这个字段
- [ ] `cost` 暂不用（Kivio 无成本展示位）
- [ ] `acp_apply_session_update`（:828）**先单独匹配** `"usage_update"` 发事件，
      **不进 `_` 分支** —— 关键：usage 通知不是消息边界，
      不能触发 `text.on_boundary()` / `thought.on_boundary()`，否则正文游标被重置会文本重复
- [ ] `apply_acp_session_update`（:719）也加 `"usage_update"` 分支（一次性驱动路径）
- [ ] 两处**共用** `parse_acp_usage_update`（spec 第 2 条：禁止两份拷贝）

### 单测
- [ ] `usage_update` 的 `used`/`size` 被正确解析
- [ ] 走 `acp_apply_session_update` 时**不重置游标**：
      构造 `agent_message_chunk("abc")` → `usage_update` → `agent_message_chunk("abcdef")`，
      断言第二个 chunk 只产出增量 `"def"` 而非重复 `"abcdef"`（**这条最重要**）
- [ ] 走 `apply_acp_session_update`（一次性驱动）也产出 Usage 事件
- [ ] 缺 `size` 或缺 `used` → 不 panic

---

## L2 — ACP `PromptResponse.usage`

**文件**：`external_agents/session/acp.rs::format_acp_usage`（:642）

- [ ] 补 `cachedReadTokens` → `cache_read`、`cachedWriteTokens` → `cache_creation`、
      `thoughtTokens` → `reasoning_tokens`
- [ ] 实测对账：opencode `11685+4+11+1792 = 13492 = totalTokens` → 四者并列不重叠
- [ ] 全零仍返回 `None`（保留现有行为）
- [ ] 该字段 ACP 标记 UNSTABLE，缺失不得报错

**文件**：`external_agents/run.rs:1197`

- [ ] **窗口字段粘滞**：`*usage = Some(u)` 改为「新值 `context_window_tokens` 为 `None`
      时保留旧值」。原因：`usage_update`（带 size）通常先到，
      `PromptResponse.usage`（无 size）后到覆盖会丢窗口
- [ ] 逻辑三行，这是唯一需要碰 `run.rs` 的地方

### 单测
- [ ] cache/thought 字段计入
- [ ] 先收带 size 的 usage、再收不带 size 的 → 窗口保留（粘滞生效）

---

## L3 — claude

**文件**：`external_agents/stream/claude.rs`

- [ ] 抽共用的四字段提取函数（result 与 message_start 共用）
- [ ] **result 分支**（:127）：`usage.iterations` 非空 → 取**最后一个**能解析成对象的元素；
      否则用 `usage` 顶层。四字段：`input_tokens` / `output_tokens` /
      `cache_read_input_tokens` / `cache_creation_input_tokens`
- [ ] **message_start 分支**（:170）：现有两行**不动**
      （`text_streamed = false`、`current_message_id` —— spec 第 3 条的 per-message 复位）；
      追加 `message.usage` 非全零时发 Usage 事件
- [ ] 不做 `modelUsage.contextWindow`（见 prd Non-Goals）

### 单测
- [ ] `result` 含 cache 字段 → input 计入 cache read + creation
- [ ] `result` 有多个 `iterations` → 取**末项**，不累加
- [ ] `result` 的 `iterations: []` → 退回顶层
- [ ] `message_start` 带 usage → 产出 Usage 事件
- [ ] `message_start` 不带 usage → 不产出，且现有 message_id / text_streamed 逻辑不变
      （现有测试 `parses_text_delta_from_stream_event` 等必须保持绿）

> 本机嵌套 claude 未登录，无非零实跑样本；单测按 `research/` 记录的真实字段结构手工构造。

---

## L4 — codex

**文件**：`external_agents/session/codex_app_server.rs:117`

- [ ] `tokenUsage.last` 优先，缺失退 `tokenUsage.total`
- [ ] 取 `inputTokens` / `outputTokens` / `cachedInputTokens`（camelCase）
- [ ] **不读** `reasoningOutputTokens`（实测 5+7=12，已含在 output 内）
- [ ] codex 无 cache_creation 概念 → 传 0

### 单测
- [ ] 构造 `last ≠ total` 的样本，断言取到 **last**
- [ ] 只有 `total` 无 `last` → 退回 `total`
- [ ] 现有 `token_usage_emits_usage` 保持绿（其样本 last==total）

---

## L5 — pi

**文件**：`external_agents/session/pi_rpc.rs:167`

- [ ] 补 `cacheRead` → `cache_read`、`cacheWrite` → `cache_creation`
- [ ] **不读** `reasoning`（实测 `6571+1578+4096 = 12245 = totalTokens`，已含在 output 内）
- [ ] pi 窗口来源（`--list-models` 第 3 列）不动

### 单测
- [ ] `cacheRead` 占大头的样本 → 结果远大于 `input` 单值（实测占 62%）

---

## L6 — cursor 窗口从 modelId

**文件**：`external_agents/session/acp.rs::normalize_models`（:168）

- [ ] 从 modelId 的方括号参数抠 `context=<value>`，
      复用现成的 `external_agents/context.rs:22` `parse_context_window_label`
      （已能解析 `"300k"` → 300000）
- [ ] 优先级：`_meta.totalContextTokens` > modelId 的 `context=` > `None`
      （`_meta` 是显式字段，比字符串提示可靠）
- [ ] 无提示的模型窗口留 `None`，**不猜**

### 单测
- [ ] `claude-opus-5[thinking=true,context=300k,effort=high,fast=false]` → 300000
- [ ] `gpt-5.6-sol[context=272k,reasoning=medium,fast=false]` → 272000
- [ ] `composer-2.5[fast=true]`（无 context） → `None`
- [ ] 同时有 `_meta.totalContextTokens` 与 modelId `context=` → 取 `_meta`
- [ ] 现有 grok 测试 `acp_models_default_slot_enriched_by_current_model_id` 保持绿

---

## L7 — kimi

### (a) 窗口静态映射

**文件**：`external_agents/context.rs`（紧邻 claude 别名表）

- [ ] 新增 kimi 映射函数，与 `context_window_from_claude_model_alias` 同构
- [ ] 表 + 来源注释（opencode `models --verbose` 实测 + kimi 官方文档，2026-07-26 核对）：
      `k3→1_048_576`、`k3-256k / kimi-for-coding[-highspeed]→262_144`
- [ ] 容忍带/不带 `kimi-code/` 前缀
- [ ] **不改** `model_database_entry` 的通用匹配算法（服务所有 provider，有误命中风险）

### (b) wire.jsonl 真实用量

**新文件**：`external_agents/kimi_usage.rs`

- [ ] kimi home：`KIMI_CODE_HOME` 优先，回落 `~/.kimi-code`
- [ ] 读 `session_index.jsonl`，按 `workDir == resolve_effective_cwd()` 筛候选
      （**不能**用 Kivio 存的 session id —— kimi 走 ACP，id 由 kimi 生成，
      Kivio 没存，实测 0 个 kimi 记录）
- [ ] **跳过空壳会话**：判据 wire.jsonl 里存在 `type=="usage.record"`
      且 `usageScope=="turn"`（实测 52/53 是 slash 探测残渣，spec 第 11b 条）
- [ ] 有效候选按 wire.jsonl mtime 取最新
- [ ] 取最后一条 `usage.record`：
      input = `inputOther + inputCacheRead + inputCacheCreation`，output = `output`
- [ ] 8MB 文件上限防御
- [ ] **全程只读**，任何失败静默 `None`

**接入**：`external_agents/context.rs::collect_external_session_usage`，
在「CLI 实报」与「字符估算」之间插一级，只对 `agent_id == "kimi"` 生效。

- [ ] 确认不在回复热路径（spec 第 9 条）：该函数由 `chat_get_context_stats` 调用
      （用户点开用量条），且是读文件不是起子进程 ✓

### 单测
- [ ] 临时目录造 index + wire.jsonl → 解析出正确 input/output
- [ ] 空壳会话（无 usage.record）被跳过，选到有 usage 的那个
- [ ] index 不存在 / wire 不存在 / JSON 损坏 → `None`，不 panic
- [ ] `inputCacheRead` 占大头的样本 → 结果远大于 `inputOther`（实测占 97.6%）
- [ ] `kimi-code/k3` → 1048576、`k3-256k` → 262144、不带前缀也对

---

## L8 — 关键词表补 256k

**文件**：`chat/model_metadata.rs`（`context_window_for_model` 的 `known`）

- [ ] 插 `("256k", 262_144)`，位置在 `1m` 与 `200k` 之间（保持降序）
- [ ] 单测：名字含 `256k` 的模型 → 262144

---

## L9 — 窗口优先级 + 不编造

**文件**：`external_agents/context.rs`

- [ ] `context_window_for_external_model` 返回 `(usize, bool)` → `(Option<usize>, bool)`
- [ ] 优先级链（新增第 1 级最高）：
      1. CLI 本轮实报 `message.usage.context_window_tokens`（L1 的 `usage_update.size`）
      2. 探测上报 `RuntimeModelOption.context_window_tokens`
      3. claude 别名表
      4. kimi 静态映射（L7a）
      5. 数据库 + 关键词（含 L8）
      6. `None` ← 不再兜底 200K
- [ ] `compute_external_context_state`：窗口 `None` → `usage_ratio = None`
- [ ] `external_context_status(None)` 已返回 `"unknown"`（确认即可）
- [ ] **内置路径 `chat/commands/context.rs` 完全不动**
- [ ] 编译器抓全调用点，逐个处理

### 前端确认（预计不改代码）
- [ ] `windowLabel(null)` → `contextTokensUnknown`（已有）
- [ ] `showThresholdMarkers` → false（已有）
- [ ] **实机看渲染**：`contextFullnessCliPending` 字面是"等待 CLI 上报"，
      用在"窗口永远拿不到"的场景可能误导 → 若误导则调文案

### 单测
- [ ] 未知 agent + 未知模型 → 窗口 `None`、ratio `None`、status `"unknown"`
- [ ] CLI 实报窗口存在时，优先级高于静态表
- [ ] claude / codex / pi（有窗口来源）不受影响

---

## 收尾验证

### 自动化
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml --lib`
      → 对照基线 **1135 passed / 0 failed**，只准增不准减
- [ ] `cargo test --manifest-path src-tauri/Cargo.toml`（含集成）
- [ ] `npm run lint` / `npm run typecheck` / `npm test`
      （前三项在动了前端时是硬门槛；L9 若只确认不改，仍建议跑）

### 实机（必做，`npm run dev`）

装好的 CLI 逐个过：

- [ ] **opencode**（AC15）：发一轮 → 窗口 200K（来自 `usage_update.size`），
      已用与 `used` 一致（不是 input+output 的 11689）
- [ ] **kimi**（AC16）：窗口 **256K**，已用万级而非 `~24`；
      对照该会话 workspace 的 wire.jsonl 末条手算比对
- [ ] **claude**（AC17）：长历史一轮 → 用量明显高于改动前（cache 计入有量级变化）
- [ ] **codex**（AC18）：连发 3 轮 → 用量**不是**单调累加
- [ ] **pi**（AC19）：发一轮 → 用量高于改动前（cacheRead 计入）
- [ ] **cursor**（AC20）：选带 `context=300k` 的模型 → 显示 300K；
      选无提示的模型 → 显示「未知」而非 200K
- [ ] **回归**（AC21）：内置（非外部 CLI）聊天的上下文条行为无变化

未安装（gemini / hermes）：由 L1 通用通道覆盖，无法实机，标注即可。

### 重点回归项

- [ ] **ACP 文本不重复**（L1 最大风险）：opencode 发一轮较长回答，
      确认正文没有重复段落 —— `usage_update` 若误触发游标重置就会重复
- [ ] pi 轮次收尾正常（spec 第 8b 条的 3s 宽限逻辑未动，但 L5 改了同文件）

---

## 回滚点

| 层 | 单独回滚影响 |
|---|---|
| L0 | 需连带 L1-L5 |
| L1 | ACP 分子分母都回到无来源（opencode 退回估算 + 兜底） |
| L2 | ACP cache 字段丢失，偏低 13% |
| L3 | claude 回到低估 |
| L4 | codex 回到累计口径（虚高） |
| L5 | pi 回到低估 62% |
| L6 | cursor 窗口回到未知 |
| L7 | kimi 窗口 + 用量都回到现状 |
| L8 | 含 256k 的模型名识别不出 |
| L9 | 回到编造 200K（若 L1/L6/L7 已上，单独回滚意义不大） |

L6 / L7 / L8 是「在链上加一级」，单独 revert 即回到现状，风险最低。
