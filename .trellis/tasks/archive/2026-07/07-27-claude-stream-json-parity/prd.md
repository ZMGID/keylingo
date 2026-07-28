# claude stream-json 协议完整度对齐

## Goal

把 Kivio 对 claude `stream-json` 输出的解析补齐到官方 `SDKMessage` 类型定义的覆盖度。
重点是**错误不再静默** —— 目前一条带 `is_error: true` 的 result 会被当成成功，
用户看到空回复且没有任何提示。

## Background

用户要求 claude 的连接方式「向 paseo 对齐，更通用一点」。

实测澄清（`research/stream-json-protocol.md`）：**paseo 的 SDK 与 Kivio 跑的是同一条命令行**
（`--output-format stream-json --verbose --input-format stream-json`）。反编 SDK 的
`sdk.mjs` 确认，paseo 的 `query.ts` 也只是劫持其 spawn 钩子换可执行文件。

所以「对齐」不是换传输层（那需要引入 Node 运行时，语言边界代价不成比例），
而是**把同一条流里 Kivio 尚未消费的消息变体补齐**。官方 `sdk.d.ts` 就是这条流的
权威类型定义 —— 当文档用，不当依赖用。

## Requirements

### R1 — result 的错误必须进错误通道（核心）

- `result` 分支要判 `subtype`（`success` 与四种 `error_*`）与 `is_error`。
- **`subtype: "success"` 且 `is_error: true` 也是错误** —— 实测未登录时正是这个组合，
  错误文案在 `result` 字段里。不能只看 subtype。
- 错误文案优先级：`errors[]`（error 子型独有）> `result`（success 子型带 is_error 时）。
- 必须走 `errors::classify`（spec 第 5 条），不得把裸串直接落气泡。
- `usage` 的解析保持现状不变（上一轮刚修好，含 iterations 末项与 cache）。

### R2 — system 子类型补齐

- `status`（`SDKStatusMessage`）：`status` + 可选 `permissionMode`。
- `task_notification`：`status`（completed/failed/stopped）+ `summary`。
- `hook_started` / `hook_response`：**判断是否值得暴露**。实测一次最简调用就有 4 条，
  噪音大；若无明确用途则显式忽略并注释「有意不接」，而不是留空落进 `_`。
- 未知 subtype 一律安全忽略（不得 panic、不得中断流）。

### R3 — 工具进度

- `tool_progress`：`tool_use_id` / `tool_name` / `elapsed_time_seconds`。
- `tool_use_summary`：`summary`。
- 需要评估能否映射到现有 `ToolCallRecord` 的状态机而**不新增事件变体**
  —— 新增 `UnifiedAgentEvent` 变体会牵动所有 CLI 的 match（上一轮加 `CliCompacted`
  已经动了 3 处）。若必须新增，说明理由。

### R4 — permission_denials

- result 里的 `permission_denials[]`（`tool_name` / `tool_use_id` / `tool_input`）
  应让用户可见 —— 被拒的工具调用目前完全无痕迹。
- 呈现方式待定：可并入错误文案，或作为一条 system 提示。选最轻的做法。

### R5 — 许可与协议边界

- paseo 为 AGPL-3.0，Kivio 为 GPL-3.0，**不得并入其代码/命名/结构**。
- `sdk.d.ts` 是 Anthropic 官方类型声明（MIT），作为**协议文档**参考、
  按字段名实现自有解析器，不构成代码借用。
- 所有字段结构以 `research/stream-json-protocol.md` 记录的实测 + 类型定义为准。

## Non-Goals

- **不引入 Node/SDK 依赖**。语言边界代价不成比例（打包 Node +40MB，或 Rust→Node→claude
  三层套壳），且直接解析是**超集** —— `iterations[]` 末项就是 SDK 抽象层拿不到的，
  上一轮正是靠它修对了用量。
- 不改命令行参数（已与 SDK 一致）。
- 不做 `total_cost_usd`（Kivio 无成本展示位）。
- 不动其它 CLI 的解析。

## Acceptance Criteria

- [ ] AC1 `{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}`
      → 产出 Error 事件且文案含该提示。用**实测原样本**做单测。
- [ ] AC2 四种 `error_*` subtype 各产出 Error；`errors[]` 非空时优先用它。
- [ ] AC3 正常 `success` 且 `is_error: false` → **不**产出 Error，usage 解析不受影响
      （现有 usage 单测全部保持绿）。
- [ ] AC4 `status` / `task_notification` 被解析；未知 subtype 安全忽略不 panic。
- [ ] AC5 `tool_progress` / `tool_use_summary` 被消费（或有明确注释说明为何不接）。
- [ ] AC6 `permission_denials` 非空时用户可见。
- [ ] AC7 `cargo test --lib` 对照基线 **1209 passed / 0 failed / 18 ignored**，不得新增失败。
- [ ] AC8 真机：未登录状态跑一次 claude，确认气泡显示可操作的错误提示而非空回复。
      （本机嵌套 claude 恰好未登录，可直接复现。）
- [ ] AC9 真机：正常登录状态跑一轮，确认回答与用量都正常（不因新增分支回归）。

## Notes

- 上一轮（07-26-local-cli-context-usage）已接 `compact_boundary`，本轮不重复。
- 实测事实与复现命令在 `research/stream-json-protocol.md`。
- spec `guides/external-cli-agents.md` 第 5 条（错误出口统一走 classify）、
  第 10 条（非 JSON 行 continue）、第 13 条（必须带单测）适用。
