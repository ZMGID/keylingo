# 对话生命周期 Hooks

## 背景

Kivio 的 agent loop（`chat/agent/`）目前对外部世界完全封闭：用户无法在「一轮对话开始 / 工具执行前后 / 回答结束」这些时刻挂自己的 Shell 脚本或 HTTP 回调。参考实现是 LiveAgent 的 `automation` 子系统（`crates/agent-gui/src-tauri/src/commands/automation/hook.rs` + `src/lib/automation/hookRunner.ts` + `src/pages/settings/HooksSection.tsx`）：8 个生命周期事件、command / http 两种 Hook、全局串行执行链、按会话 scope 取消。

业界调研（Claude Code hooks / Codex hooks / OpenCode plugins）：

| 产品 | 事件模型 | 载荷 | 阻断能力 |
|------|----------|------|----------|
| Claude Code | 12+ 事件（PreToolUse/PostToolUse/UserPromptSubmit/SessionStart/Stop…） | stdin JSON | 有（exit 2 / JSON decision） |
| Codex CLI | 10 事件（PreToolUse/PermissionRequest/PostToolUse/PreCompact/SessionStart/Stop…），`hooks.json` 或 `config.toml` | stdin JSON，无专用 env | 有 |
| OpenCode | 25+ 事件，TS 插件而非脚本 | JS 对象（可改 args/output） | 有 |
| LiveAgent | 8 事件，settings 存储 | env 变量 | 无（fire-and-forget） |

共识：**stdin JSON 是事实标准载荷**（Claude/Codex 一致），env 变量是 LiveAgent 的方言。Kivio 取交集：stdin JSON 为主，同时补 `KIVIO_*` env 便于一行脚本使用。

阻断语义（PreToolUse deny / Stop block）是 Claude/Codex 的核心卖点，但依赖一整套 decision schema + 权限系统对接。**本任务不做**，见「非目标」。

## 目标

1. 用户可在设置里为 8 个对话生命周期事件配置 Hook（Shell 脚本 / HTTP 请求），启用、禁用、编辑、删除。
2. Hook 在内置 agent loop 运行时按事件触发，收到结构化上下文；失败不影响对话本身，只上报警告。
3. 会话中止 / run 结束时，该 run 排队中的 Hook 被丢弃、在跑的脚本被杀掉。
4. UI 复刻参考实现：左侧生命周期导轨（按 AGENT / TURN / MESSAGE / TOOL 四个阶段分组）+ 右侧该事件的 Hook 列表，遵循 Kivio 现有设置页规格（`kv-panel` / `Button` / `IconButton`）。

## 事件集

沿用 LiveAgent 的 8 个事件（与 Kivio loop 阶段一一对得上，无需发明新词）：

| 事件 | 触发点（Kivio loop） | 阶段 |
|------|---------------------|------|
| `agent_start` | `run_agent_loop` 进入，prepare 之前 | AGENT |
| `turn_start` | 每次 planning step 开始 | TURN |
| `message_start` | 该 step 的助手消息开始产出 | MESSAGE |
| `message_end` | 该 step 助手消息产出完毕（含工具调用决策） | MESSAGE |
| `tool_execution_start` | 每个工具调用执行前 | TOOL |
| `tool_execution_end` | 每个工具调用返回后 | TOOL |
| `turn_end` | 该轮工具全部结束 / 该轮无工具即结束 | TURN |
| `agent_end` | loop 返回（成功、失败、取消都触发） | AGENT |

## 载荷

Hook 脚本 stdin 收到一行 JSON：

```json
{
  "event": "tool_execution_start",
  "hookName": "lint-guard",
  "conversationId": "…",
  "runId": "…",
  "messageId": "…",
  "cwd": "/path/to/workdir",
  "round": 2,
  "toolName": "write_file",
  "model": "provider:model"
}
```

同时注入 env：`KIVIO_HOOK_EVENT` / `KIVIO_HOOK_NAME` / `KIVIO_CONVERSATION_ID` / `KIVIO_RUN_ID` / `KIVIO_WORKDIR` / `KIVIO_TOOL_NAME`（无值则为空串）。
HTTP Hook 把同一 JSON 作为 POST body；用户自定义 body 时以用户 body 为准，事件名另走 `X-Kivio-Hook-Event` 头。

## 非目标（本期不做）

- **阻断 / 改写语义**：exit 2 拒绝工具调用、`decision: block` 强制模型继续、`updatedInput` 改写工具参数。本期 Hook 一律 fire-and-forget，失败只报警告。
- **matcher 通配**（Claude 的 `Edit|Write` 正则匹配工具名）：本期一个 Hook 绑一个事件，工具过滤由脚本自己读 `toolName` 判断。
- **外部 CLI agent runtime 的 Hook**：external_agents 走别人的进程，生命周期事件不可靠，本期仅内置 loop。
- **Cron 定时任务**：参考实现里 Hook 与 Cron 同属 automation，本期只做 Hook。

## 验收标准

1. 设置里新增「Hooks」页，能创建 command / http 两类 Hook，配置在 `settings.json` 持久化，重启后仍在。
2. 配置一个 `agent_end` 的 command Hook（例如 `echo done >> /tmp/kivio-hook.log`），在 chat 里发一条消息，对话结束后日志文件被写入。
3. 配置一个 `tool_execution_start` Hook，触发一次带工具调用的对话，脚本 stdin 收到含 `toolName` 的 JSON。
4. 脚本 exit != 0 或超时：对话正常完成，前端出现一条 Hook 失败警告，不打断流式。
5. 对话进行中点「停止」：排队 Hook 不再执行，在跑的脚本被杀。
6. 未配置任何 Hook 时，agent loop 无额外开销（不构造载荷、不进调度器）。
7. `npm run lint` / `npm run typecheck` / `npm test` / `cargo test` 全绿。
