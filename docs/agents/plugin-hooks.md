# Plugin 与工作流 Hook 协议（第一版）

## 使用入口

- 扩展 → 插件 → **通用插件**：输入本地目录或 HTTPS Git 仓库，必要时填写插件子目录。导入不执行安装器，默认停用；检查组件后点启用。
- 设置 → Hooks → **工作流 Hooks**：编辑并单独保存 JSON。`enabled: true` 表示允许执行脚本。旧生命周期 Hook 继续作为异步通知，保持原有配置和行为。
- 当前作用域为个人，供内置 Kivio Agent 使用。Kivio Chat 不执行工作流脚本；外部 CLI 继续使用自己的原生插件配置。

## 插件包

原生格式见 [Kivio Plugin v1](kivio-plugin-format.md)。入口优先级为 `.kivio-plugin/plugin.json` → `.codex-plugin/plugin.json` → `.claude-plugin/plugin.json`；只加载一份，解析失败不回退。包根保留原有目录结构，支持 Skills、Markdown commands、Markdown agents、stdio / Streamable HTTP MCP、command Hooks。

```text
example/
  .claude-plugin/plugin.json
  skills/review/SKILL.md
  commands/check.md
  agents/reviewer.md
  hooks/hooks.json
  scripts/context.cjs
  .mcp.json
```

组件路径必须位于包根内。支持 manifest 自定义路径；Claude/Codex 兼容清单的 hooks/MCP 支持文件、内联对象和数组，Kivio 原生清单仅接受路径或路径数组。Claude skills 自定义路径追加到默认目录；commands/agents 自定义路径替代默认目录。Claude 默认 hooks/MCP 与显式配置一起读取，相同 JSON 不重复加载；Codex 和 Kivio 原生格式使用显式配置替代默认文件。

插件内容复制到 `<app_data>/plugins/packages/<uuid>/content`，元数据保存在同级 `record.json`，插件可写数据放在 `data`。Git 来源保存实际 commit；不递归克隆 submodule，不复制 `.git`、`node_modules`，拒绝 symlink 和越界 junction，导入内容限制为 100 MiB。依赖由用户自行准备。

Skill/agent 内部 ID 带包身份，显示名称带插件名称，命令形式如 `/example:check`。同名插件只能启用一个来源或版本；更换版本可以先导入新包，再停用旧包、启用新包。当前没有自动更新、marketplace 浏览或项目级开关。

停用后新的 Hook、Skill、MCP 调用会检查插件状态；已经注入模型的历史上下文不会被追溯抹除，已在执行的工具遵循正常取消机制。移除仅删除 Kivio 托管副本及该包的 MCP 注册，不删除导入来源。更新或编辑配置在下一轮重新构造运行时；运行中的钩子按已取得的配置执行。

## 工作流 Hook 配置

```json
{
  "enabled": true,
  "hooks": {
    "PreToolUse": [
      {
        "matcher": "^write$",
        "hooks": [
          {
            "type": "command",
            "command": "node",
            "args": ["/absolute/path/check.cjs"],
            "timeout": 10
          }
        ]
      }
    ]
  }
}
```

个人配置和 Kivio 原生插件使用 Kivio 工具名及参数。Claude/Codex 格式的兼容插件会对常见 Claude 工具名进行转换：Bash、Read、Write、Edit、Glob、Grep、Agent/Task；文件工具的 `file_path` 与 Kivio `path` 双向转换。其余工具参数仍须满足 Kivio 实际 schema，不能假设两个宿主的所有工具完全一致。

`args` 存在时直接启动可执行文件，避免 Shell 解析；不存在时使用 Kivio 默认 Shell（Windows 优先 Git Bash、否则 PowerShell；其他平台 sh）。可指定 `shell: "bash"`。脚本命令中的环境变量保持为变量引用，不把路径直接拼成 Shell 源码；PowerShell 路径转换为对应的环境变量语法。

## 事件与输入

共有字段：`schema_version: 1`、`hook_event_name`、`session_id`、`cwd`、`agent_type`。

| 事件 | 边界 | 特有输入 |
| --- | --- | --- |
| SessionStart | 进程内首次进入该会话；应用重启后恢复会话；自动摘要压缩完成 | `source`: startup / resume / compact |
| UserPromptSubmit | 主用户消息，以及运行中的 steering / follow-up；模型请求前 | `prompt`：原始用户文本，插件命令展开不覆盖它 |
| SubagentStart | 子代理首次模型请求前 | `agent_type` |
| PreToolUse | 工具 schema 校验与审批之前 | `tool_name`、`tool_input`、`tool_use_id` |
| PostToolUse | 已执行工具返回后；审批拒绝等未执行路径不触发 | 同上，加 `tool_response`（当前是 Kivio 返回的工具文本） |

简单的 matcher 字符/`|`列表按完整名称匹配，其他字符串按正则处理；空字符串或 `*` 匹配全部。按插件身份排序，包内按声明顺序执行，同一事件中的处理器串行等待。多个并行工具仍可同时触发各自的 Hook，作者应使用 tool_use_id/session_id 管理并发。

会话启动上下文缓存于进程内，会在后续轮次重新加入模型的 system 前缀；UserPromptSubmit 的上下文也保留到后续轮次。缓存上限 256 个会话/配置组合、每组合 64 KiB，配置改变使用新缓存身份。主会话和子代理上下文独立。`data` 是插件级持久目录，作者需要自己使用 session_id 隔离会话状态。

## 输出

stdin 收到一份 UTF-8 JSON，写完关闭。脚本 stdout 可返回：

```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "ask",
    "permissionDecisionReason": "请确认修改此文件",
    "updatedInput": { "path": "safe.txt", "content": "text" },
    "additionalContext": "用于此次执行的补充信息"
  }
}
```

- `additionalContext`：启动/用户消息/子代理事件注入相应模型上下文；工具事件放进工具反馈。仅 SessionStart / UserPromptSubmit 接受普通 stdout 文本作为上下文。
- `PreToolUse.updatedInput`：完整替换参数，重新校验 schema，然后进行审批。多个 Hook 依次看到上一个修改后的输入。
- `PreToolUse.permissionDecision`：deny 阻止执行；ask 请求宿主审批；allow 仍受原有权限检查约束。一个 Hook 拒绝后停止后续处理。
- `UserPromptSubmit` 可用顶层 `decision: "block"` 和 `reason` 拒绝；启动事件/用户提示中的 `continue: false` 按拒绝处理。PostToolUse 的停止理由作为反馈，不会撤销已完成工具。
- 退出码 2 可拒绝 PreToolUse / UserPromptSubmit，以 stderr 为理由；其他非零退出是执行错误。
- JSON 声明的 hookEventName 必须匹配实际事件。以 `{` / `[` 开头的无效 JSON 不会伪装成普通上下文。

环境提供 `PLUGIN_ROOT`、`PLUGIN_DATA`、`CLAUDE_PLUGIN_ROOT`、`CLAUDE_PLUGIN_DATA`、`CLAUDE_PROJECT_DIR` 与 `KIVIO_HOOK_EVENT`。Skill/agent 正文中的插件根和数据目录占位符也会展开。不会读取或写入 Claude/Codex 的个人设置文件。

每个处理器超时为 1–600 秒，默认 30 秒，覆盖 stdin 写入、输出读取和等待退出；单管道输出上限 64 KiB，输入上限 1 MiB。超时、输出过大、取消或 future 被丢弃都会清理进程树。PreToolUse 错误不会继续执行工具；PostToolUse 错误反馈给模型；启动/用户提示错误通过原有回复错误通道显示。

## 当前明确不支持

Stop/SubagentStop 等其余事件，prompt/agent/http 型工作流处理器，async workflow hooks，组件内嵌 hooks、context: fork，TOML 子代理、LSP、output styles、插件依赖自动安装、平台托管 apps 与专有 UI。未知事件或已知不支持的组件会生成诊断并阻止启用，避免把只加载一部分的包误报为可用。

当前没有仿制 Claude transcript 文件、CLAUDE_ENV_FILE 持久环境或宿主状态栏。插件可以使用明确传入的 JSON 和插件数据目录；依赖这些宿主专属设施的脚本需要适配。包支持不等于对所有 Claude/Codex 工具 schema、权限模型和会话行为完全等价。

## 开发验证

Windows 使用 `scripts/win-cargo-test.ps1`；普通 `cargo test` 测试程序缺少 Common-Controls v6 清单时可能以 0xC0000139 退出。

相关测试过滤：`workflow_hook`、`plugins::packages`、`chat::agent::execute::tests`、`chat::hooks::tests`。前端测试为 PluginPackages、WorkflowHooksPanel、HooksTab；变更还应通过 TypeScript 与 UI 构建。
