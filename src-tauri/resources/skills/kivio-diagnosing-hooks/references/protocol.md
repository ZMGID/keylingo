# 工作流 Hook 协议

个人配置示例（脚本路径要换成已创建的真实路径）：

```json
{
  "enabled": true,
  "hooks": {
    "PreToolUse": [{
      "matcher": "write|edit",
      "hooks": [{"type":"command","command":"node","args":["C:/actual/path/check.cjs"],"timeout":10}]
    }]
  }
}
```

插件 hooks 文件只需 `{"hooks":{...}}`，开关由包控制。个人工作流无需伪造一个插件包。有 args 时直接启动 executable，不经 Shell；无 args 才经默认 Shell，可以显式指定 `shell:"bash"`。Windows 的 Shell 依运行时探测，不能假定 PowerShell。

## 输入与事件

stdin 一份 UTF-8 JSON，输入写完关闭。共有键 `schema_version:1`、`hook_event_name`、`session_id`、`cwd`、`agent_type`。

- SessionStart：进程内首次进入会话、应用重启后恢复、自动压缩后；`source` 为 startup/resume/compact。
- UserPromptSubmit：原始用户文本 `prompt`，包括运行中 steering/follow-up；插件命令展开不覆盖原文。
- SubagentStart：子代理首次模型请求前。
- PreToolUse：schema 校验与审批之前；`tool_name`、`tool_input`、`tool_use_id`。
- PostToolUse：实际执行工具之后，附 `tool_response` 文本；审批拒绝等未执行情况不触发。

原生/个人 Hooks 用 Kivio 工具名，如 `bash`、`read`、`write`、`edit`、`agent`；文件参数为 `path`。Claude/Codex 兼容包会转换部分名称和 file_path/path；其他 schema 不承诺相同。不要改 matcher 为模型显示 ID 的猜测形式。

简单 matcher / `|` 列表按全名匹配，复杂表达式用正则，空或 `*` 全匹配。插件按身份排序，包内按声明顺序串行；并行工具仍可能同时触发，持久状态用 session_id/tool_use_id 隔离。

## 输出

```json
{"hookSpecificOutput":{"hookEventName":"PreToolUse","permissionDecision":"deny","permissionDecisionReason":"此规则禁止修改该路径"}}
```

- PreToolUse：`updatedInput` **完整替换**工具参数，重新 schema 校验再审批；多个处理器依次看到前一份改写。deny 阻止，ask 请求审批，allow 仍受宿主权限限制。
- `additionalContext` 注入相应事件上下文。SessionStart/UserPromptSubmit 可接受普通 stdout 文本；工具事件结果反馈给模型。
- UserPromptSubmit 可返回顶层 `decision:"block"` + `reason`；启动/用户事件中的 `continue:false` 按拒绝处理。
- 退出码 2 可拒绝 PreToolUse/UserPromptSubmit，以 stderr 为理由；其他非零为执行错误。PostToolUse 的停止反馈不会撤销已执行工具。
- 若输出 JSON 含 hookEventName，必须与事件一致；以 `{`/`[` 起始的坏 JSON 不是普通上下文。

提供 `PLUGIN_ROOT`、`PLUGIN_DATA`、`CLAUDE_PLUGIN_ROOT`、`CLAUDE_PLUGIN_DATA`、`CLAUDE_PROJECT_DIR`、`KIVIO_HOOK_EVENT`。插件 root 为 content，data 为同级 data；个人 Hook root 为 plugins，data 为 hook-data。args 的占位符由宿主展开，不能把任意路径拼成 shell 源码。

不提供仿制 Claude transcript、CLAUDE_ENV_FILE 持久环境、状态栏。脚本进程设置 env 不会修改后续工具进程的环境。

默认 timeout 30 秒，单位**秒**，合法 1–600；单管道输出 64 KiB、输入 1 MiB。通知 Hook 的 timeoutMs 用毫秒，不能混填。超时/取消会清理进程树。PreToolUse 失败不继续执行；PostToolUse 失败反馈给模型。

源码：`src-tauri/src/chat/workflow_hooks.rs`、`src-tauri/src/chat/hooks.rs`、`src-tauri/src/plugins/packages.rs::{workflow_hooks_save,hook_runtime}`；完整产品协议见仓库 `docs/agents/plugin-hooks.md`。
