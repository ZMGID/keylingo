# claude stream-json 协议事实（2026-07-27 实测）

## 结论先行：SDK 与 Kivio 跑的是**同一条命令行**

反编 `@anthropic-ai/claude-agent-sdk` 的 `sdk.mjs`（本机
`~/.npm/_npx/4c83e1a162637933/node_modules/@anthropic-ai/claude-agent-sdk/`）：

```
["--output-format","stream-json","--verbose","--input-format","stream-json"
```
外加 `--print` / `--include-partial-messages`。

Kivio 的 `defs/claude.rs::build_claude_args` 拼的是
`-p --input-format stream-json --output-format stream-json --verbose`
（+ `--include-partial-messages`）—— **完全一致**。

paseo 的 `providers/claude/query.ts` 也只是劫持 SDK 的 `spawnClaudeCodeProcess`
钩子换掉可执行文件，没有改变协议。

**所以「对齐 paseo 的 claude 连接方式」不是换传输层**，而是把同一条流里
Kivio 尚未消费的消息变体补齐 —— SDK 的 `sdk.d.ts` 就是这条流的权威类型定义，
可以当**文档**用（不必当依赖用）。

## 许可

paseo = AGPL-3.0-or-later，Kivio = GPL-3.0-or-later，**不可并入其代码**。
`sdk.d.ts` 是 Anthropic 官方发布的类型声明（MIT，见同目录 LICENSE.md），
作为协议文档参考、按其字段名实现自有解析器，不构成代码借用。

## SDKMessage 联合体全集（sdk.d.ts:1476）

```
SDKAssistantMessage | SDKUserMessage | SDKUserMessageReplay | SDKResultMessage
| SDKSystemMessage | SDKPartialAssistantMessage | SDKCompactBoundaryMessage
| SDKStatusMessage | SDKHookStartedMessage | SDKHookProgressMessage
| SDKHookResponseMessage | SDKToolProgressMessage | SDKAuthStatusMessage
| SDKTaskNotificationMessage | SDKFilesPersistedEvent | SDKToolUseSummaryMessage
```

## Kivio 现状（`stream/claude.rs::handle_value`）

顶层 `type` 分支只有六个：
`system`（仅 `init` + 上一轮加的 `compact_boundary`）/ `stream_event` /
`assistant` / `user` / `result` / `error`。

## 差距逐条（按严重度）

### D1 — `result` 完全忽略 `subtype` 与 `is_error` 【会看到错误数据 / 静默失败】

sdk.d.ts:1506 `SDKResultError` 的 subtype 有四个：
```
error_during_execution | error_max_turns | error_max_budget_usd
| error_max_structured_output_retries
```
且带 `errors: string[]`。`SDKResultSuccess`（:1525）另有
`is_error: boolean`、`result: string`、`stop_reason`、`permission_denials[]`。

Kivio 的 `result` 分支（`stream/claude.rs:192`）**只取 `usage`**，
`subtype` / `is_error` / `errors` / `result` 一律不看。

**实测证据**（本机 `claude -p "..." --output-format stream-json --verbose`，
嵌套环境未登录）：
```json
{"type":"result","subtype":"success","is_error":true,
 "result":"Not logged in · Please run /login","stop_reason":"stop_sequence",
 "total_cost_usd":0,...}
```
注意 **`subtype` 是 `success` 但 `is_error` 为 true**，错误文案在 `result` 字段里。
Kivio 两个字段都不读 ⇒ 这一轮静默产出空回复，用户看不到任何错误提示。

这与 spec 第 5 条（错误出口统一走 `errors::classify`）直接冲突 ——
一条本该分类的错误从来没进过分类器。

### D2 — `system` 的其余 subtype 全落空 【体验缺失】

实测一次最简调用就出现了这些 subtype：
```
hook_started / hook_response / init / status
```
sdk.d.ts 另有 `task_notification`（:1659，带 `status: completed|failed|stopped`
与 `summary`）、`compact_boundary`（上一轮已接）。

Kivio 只认 `init`（+ `compact_boundary`），其余静默丢弃。
`status`（`SDKStatusMessage`:1621）带 `SDKStatus` 与 `permissionMode`，
是「CLI 正在做什么」的实时信号 —— 目前 Kivio 完全看不到。

### D3 — `tool_progress` / `tool_use_summary` 未接 【体验缺失】

- `SDKToolProgressMessage`（:1670）：`tool_use_id` / `tool_name` /
  `elapsed_time_seconds` —— 长时间工具调用的进度心跳。
- `SDKToolUseSummaryMessage`（:1680）：`summary`。

Kivio 的工具卡目前只有「开始 → 结束」两态，中间没有进度。

### D4 — `permission_denials` 未消费 【体验缺失】

`SDKResultSuccess` / `SDKResultError` 都带
`permission_denials: SDKPermissionDenial[]`（`tool_name` / `tool_use_id` /
`tool_input`）。被拒的工具调用对用户完全不可见。

### D5 — `total_cost_usd` 未消费 【可选】

两个 result 子型都有。Kivio 无成本展示位，接了也没地方放。**本轮不做**。

## 复现命令

```bash
claude -p "say hi" --output-format stream-json --include-partial-messages --verbose
```
（未登录时即可复现 D1 的 `is_error: true` 样本。）

类型定义查阅：
```bash
D=~/.npm/_npx/*/node_modules/@anthropic-ai/claude-agent-sdk
grep -n "export declare type SDKResultError" -A 16 $D/sdk.d.ts
```
