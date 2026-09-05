# Kivio 通用 Plugin 兼容调研

调研日期：2026-09-05。范围：仓库静态阅读、官方文档与上游源码核对；没有安装或执行第三方插件，也没有进行端到端兼容验证。

Kivio 基线：`b7de55b5fe4e22c5e1690c0453b610362e54cffe`。本次仅新增此文档。

## 结论

建议在已有插件中心上增加**通用插件包加载器 + 插件执行上下文 + 可返回结果的生命周期 Hooks**，复用现有 Skills、MCP、子代理能力。

优先以 Ponytail 验收完整的基础链路，再做 Trellis 项目工作流适配。只增加市场入口、解压仓库、扫描 SKILL.md，无法覆盖这两个项目的核心自动行为。也不应把所有新插件继续逐个硬编码进现有目录。

区分两个执行目标：内置 Kivio Agent 由 Kivio 执行插件；外部 Claude Code / Codex 由对应 CLI 执行插件。插件页面展示所选运行时下的兼容状态，不能把“已安装”当成“在所有运行时可用”。

## 当前代码已有的基础与缺口

| 模块 | 已有基础 | 通用插件需要补充 |
| --- | --- | --- |
| `src-tauri/src/plugins/catalog.rs` | 静态 `PLUGIN_CATALOG`；OfficeCLI、ego lite、Cua Driver 等专用安装信息 | 从外部 manifest 解析身份、组件、来源和版本，不要求每个插件都有 binary |
| `plugins/install.rs`、`state.rs` | 安装、探测、启停、卸载；`<app_data>/plugins/<id>/meta.json` | 本地包 / Git 来源、版本快照、项目作用域、更新回滚、组件诊断 |
| `plugins/lifecycle.rs` | 注册插件 MCP、禁用和断连、Skill 门控 | 一个包的多个 MCP server、配置插值、组件归属和事务回滚 |
| `skills/discover.rs`、`types.rs` | 已启用插件扫描入口、项目 `.kivio/skills` 和 `.agents/skills`、斜杠触发 | 动态插件根、命名空间、commands Markdown 转换、原文件相对路径保留 |
| `agents/mod.rs`、`parse.rs` | 内置、用户、项目 Markdown 子代理定义 | 插件 agents 来源、宿主工具名和模型映射、子代理上下文钩子 |
| `chat/hooks.rs` | 8 个事件、Shell / HTTP、队列、超时、取消、失败提示 | 当前明确为 fire-and-forget，不能读取返回值来影响模型或工具执行 |
| `chat/agent/prepare.rs`、`loop_.rs`、`rounds.rs` | 提示词组合、模型循环、工具执行边界 | 等待插件钩子、消费上下文 / 入参改写 / 拒绝结果、真正的会话事件 |
| `src/chat/PluginCenter.tsx`、`src/api/tauri.ts` | 现成插件列表、安装和启停交互 | 来源添加、能力诊断、项目范围、运行时过滤、更新与日志 |
| `external_agents/defs/claude.rs`、`session/codex_app_server.rs` | Claude stream-json、Codex app-server 桥接 | 原生插件状态与运行时版本检测；验证安装、启停和会话刷新方式 |

特别注意：当前 Skill 默认扫描不是无差别扫描所有 `.claude` / `.codex` 配置；额外路径、已有官方安装器的共享目录和通用插件发现是不同机制。当前 agent_start 也是一次 run 的事件，不能简单重命名成 SessionStart。

## 两个目标项目的实际要求

### Ponytail：适合作为第一个端到端样例

核对版本：manifest `4.9.0`，仓库快照 `974d940a1c5344210874150b98ff0d2c861fab6a`。

同时包含 `.claude-plugin/plugin.json` 和 `.codex-plugin/plugin.json`，都指向 `hooks/claude-codex-hooks.json`，并带 `skills/`。三个关键事件：

- `SessionStart`：启动 / 恢复 / 清空 / 压缩后注入规则。
- `UserPromptSubmit`：读取真实 `prompt`，处理模式切换与关闭。
- `SubagentStart`：把规则注入子代理，支持通过 `agent_type` 筛选。

脚本需要 Node.js，并通过 stdout 返回文本或 `hookSpecificOutput.additionalContext`。Kivio 目前的异步通知钩子不能消费这些结果，所以 Skill 能被看到也不等于插件自动生效。

其 runtime 用 `PLUGIN_DATA` 判断 Codex 兼容路径，并将状态写入该目录；否则可能落到 Claude 用户目录并产生 statusline 设置提示。接入时优先选 Codex manifest，提供独立的插件 data 目录及兼容变量，并验证并行会话的状态隔离。不能把这类环境差异当作纯路径替换。

来源：[manifest](https://github.com/DietrichGebert/ponytail/blob/974d940a1c5344210874150b98ff0d2c861fab6a/.codex-plugin/plugin.json)、[事件配置](https://github.com/DietrichGebert/ponytail/blob/974d940a1c5344210874150b98ff0d2c861fab6a/hooks/claude-codex-hooks.json)、[运行时输出和状态](https://github.com/DietrichGebert/ponytail/blob/974d940a1c5344210874150b98ff0d2c861fab6a/hooks/ponytail-runtime.js)。

### Trellis：需要项目工作流适配

此处按 `mindfold-ai/Trellis` 调研，快照 `88f4834449da9b4f607ec05e322408a0aa66f2ce`。这是多宿主工作流 CLI；当前查看的源码树没有发现标准 Claude/Codex plugin manifest。主要通过 `trellis init` 在目标仓库生成 `.trellis` 数据和宿主配置。

Claude 模板依赖 SessionStart、UserPromptSubmit，以及匹配 Task / Agent 的 PreToolUse。子代理注入脚本会返回 `updatedInput`，改写真正传给子代理的任务输入。Codex 模板则有独立 Hooks、TOML agents 等结构，不能直接照搬 Claude 文件加载方式。

建议给 Trellis 增加 Kivio configurator：生成 `.kivio/skills`、`.kivio/agents` 和项目 hooks，复用 `.trellis` 的规格、任务和脚本。长期向上游贡献适配；短期可在 Kivio 做固定版本的转换器，但必须显式维护转换范围。

除了读入脚本输出，还要处理稳定 session identity、工作目录、Python 运行环境和环境持久化。其 SessionStart 脚本会尝试经 `CLAUDE_ENV_FILE` 向后续命令传递 `TRELLIS_CONTEXT_ID`；Kivio 需要会话环境机制或明确的 Trellis 适配，不能只设置脚本进程的环境变量。

来源：[项目介绍](https://github.com/mindfold-ai/Trellis)、[Claude 模板](https://github.com/mindfold-ai/Trellis/blob/88f4834449da9b4f607ec05e322408a0aa66f2ce/packages/cli/src/templates/claude/settings.json)、[子代理上下文脚本](https://github.com/mindfold-ai/Trellis/blob/88f4834449da9b4f607ec05e322408a0aa66f2ce/packages/cli/src/templates/shared-hooks/inject-subagent-context.py)、[会话脚本](https://github.com/mindfold-ai/Trellis/blob/88f4834449da9b4f607ec05e322408a0aa66f2ce/packages/cli/src/templates/shared-hooks/session-start.py)。

## 建议架构

### 1. 统一内部模型，分别解析上游格式

新增 `plugins/manifest.rs`、`sources.rs`、`registry.rs`、`compat.rs`，将 Claude/Codex manifest 转成内部 `PluginPackage`。保留 `sourceFormat`、marketplace、原始字段和诊断结果；两份 manifest 同时存在时按运行时选一份，不能合并执行两套相同 hooks。

内部模型应包含：稳定来源身份、名称、version/revision、安装根、组件清单、作用域绑定、配置需求和能力诊断。组件包括 skills、commands、agents、MCP、hooks；未知字段保留并诊断，不能默认认为受支持。

沿用已有 plugins 根，在独立子目录存通用包的 immutable cache、data、安装索引，避免覆盖已有 CLI 插件。项目 `.kivio/plugins.json` 记录引用、固定版本和项目启停覆盖；个人启停存在个人配置。运行时根据 cwd 解析有效插件快照，防止两个项目相互串配置。

本地目录导入先跑通，其次支持 GitHub/Git URL 和子目录，再增加 marketplace。marketplace 是分发索引，plugin manifest 是包入口，必须分开解析。下载进 staging，校验后原子激活；更新锁定 commit，旧版本保留到验证完成。导入本身不执行包内安装脚本。

### 2. 用组件注册复用现有执行设施

Skill、command、agent、MCP 都带插件归属和稳定命名空间。内部 ID 应包含来源身份，显示命令可用 `/ponytail:ponytail-review`；简写仅在无歧义时提供，并保留上游已有 slash 触发行为。

不把全部 Skill 正文永久塞进系统提示词：保留目录按需加载。原始包和 scripts/references 一起保留，加载正文时以真实组件目录解析相对资源。现有 Skill frontmatter 是轻量解析器，复杂 YAML、嵌套 hooks、参数替换等需要明确实现范围。

MCP 复用 manager/conn，但项目插件应生成会话级的有效 server 集合或具备等价隔离，不能直接把项目配置永久合并进全局 settings。同名 server 使用插件限定 ID；禁用、更新、卸载按归属撤销，避免遗留 server 或破坏用户单独配置。

### 3. 增加可返回结果的 Plugin Hook Runner

保留现有旁路 Hooks 的行为，新增有返回值的执行通道。两者可共享进程管理、超时、取消和诊断设施，不把通知队列直接变成阻塞队列。

建议类型：`HookContext` 表达会话 / 提示 / 工具 / 子代理信息；`HookOutcome` 表达 additional context、改写输入、拒绝理由、继续执行要求与展示消息。每种上游格式分别编码输入、解释输出。

| 事件 | 触发位置与要求 | 优先级 |
| --- | --- | --- |
| SessionStart | 真正的新会话、恢复、clear、compact 后；模型请求前注入 | 首版 |
| UserPromptSubmit | 每条真实用户消息，slash 展开前保留原始 prompt | 首版 |
| SubagentStart | 子代理首次模型请求前；上下文只注入对应子代理 | 首版 |
| PreToolUse | 工具执行前 await；支持 tool_input、matcher、updatedInput 与拒绝 | 第二阶段，Trellis 前置 |
| PostToolUse | 工具结果之后，按事件语义处理反馈 | 第二阶段 |
| Stop / SubagentStop | 结束落盘前；继续要求须受停止按钮、取消状态和循环上限约束 | 后续；当前 Ponytail 不依赖 |

不能只映射事件名：Kivio 的工具名、参数 schema、agent 类型都与 Claude 不同。例如 Task/Agent 匹配须映射到 Kivio 实际派发工具，`updatedInput` 必须反向映射并重新校验，再进行最终权限检查。插件的 allow 不能绕过宿主限制。

提供 `PLUGIN_ROOT` / `PLUGIN_DATA` 和 Claude 兼容变量，stdin 使用对应协议的字段和 UTF-8。原始 stdout 与 JSON 的注入规则按事件区分，stderr 单独诊断。上下文保持来源、去重和大小上限，compact 后正确恢复；不能把每轮工具调用都当成新用户提示。

Windows 是必要验收平台：Node/Python 解析、带空格路径、Shell 语法、BOM、隐藏窗口、超时后子进程清理都需覆盖。不能把任意 Bash command 直接交给 PowerShell；缺少运行环境时显示明确的依赖状态。

### 4. 外部 CLI 路径独立处理

Claude Code / Codex 已有自己的插件加载机制。Kivio 应优先读取或管理原生状态，并交由 CLI 执行，禁止在同一外部会话重复执行 Kivio Hook Runner。已在 CLI 安装的插件可能已经随原配置生效，需要实测，而不是先认定它们都不可用。

开发前核实目标 CLI 版本的命令、参数或 app-server 能力，确定会话刷新策略。不能直接假设某个安装子命令或 plugin-dir 参数在两个宿主上通用。Kivio 内置模式的插件开关和外部 CLI 原生开关必须明确区分。

## 首版范围与后续顺序

1. **包加载基础**：本地 / Git 包、两种 manifest、Skills/commands、MCP、归属和启停、基础状态与日志。已有预设 CLI 插件保持可用。
2. **Ponytail 完整链路**：同步上下文 Hooks、主会话与子代理注入、模式开关、会话恢复、compact、隔离 data。基础包加载和这一步合起来才是面向用户的首个完整版本。
3. **Trellis 项目接入**：PreToolUse 输入改写、agent 映射、项目配置、会话环境、初始化 / 更新转换器，运行实际任务工作流验收。
4. **扩大生态**：marketplace 浏览与版本管理、更多 hook 事件、更多 agents/frontmatter 能力及外部 CLI 管理。

首版不承诺任意插件完全兼容。LSP、宿主专有 UI、输出风格、任意 JS 扩展和平台托管 app 应按能力诊断。Codex `.app.json` 引用的是已注册服务连接，只有该 ID 并不能在 Kivio 复用 OpenAI 的认证和连接；可用的标准 MCP 端点需要独立接入。

## 验收清单

- 两份 manifest 同包只激活一份；同名不同来源不冲突；manifest 自定义路径、内联配置按各自格式解释。
- 路径不能逃逸插件根；更新失败回滚；卸载仅处理 Kivio 托管资源。原目录导入、复制安装和外部 CLI 管理的所有权区别明确。
- Ponytail 新会话自动注入，模式切换可见，off 后正确停止后续注入；子代理收到匹配上下文；compact/resume 不丢失或重复规则。
- 两个项目或并行会话不串插件开关、MCP 配置及模式状态；禁用后的新调用不再执行该插件。
- Trellis implement/check/research 收到各自任务上下文；输入改写确实进入子代理，工作流状态与后续 Shell 命令使用同一 context identity。
- Hook timeout、无 Node/Python、非零退出、无效 JSON、过大输出和用户停止均有确定行为；非关键上下文钩子失败显示降级，控制钩子不能悄悄绕过已声明约束。
- 外部 CLI 会话用插件自带探针确认只触发一次，已有用户配置与原生插件状态没有被错误覆盖。

以上均为后续实现验收要求，本次没有运行这些测试。先用固定上游快照建立契约测试，再验证实际 Windows/macOS 和真实模型会话。

## 格式依据与仍需验证的边界

[Claude Code 插件参考](https://code.claude.com/docs/en/plugins-reference)说明 manifest、组件布局及不同字段的默认目录处理规则；[Hooks 参考](https://code.claude.com/docs/en/hooks)定义事件输入输出。不能用一套猜测的合并逻辑兼容所有字段。

[OpenAI 插件打包文档](https://developers.openai.com/plugins/build/plugins)确认 `.codex-plugin/plugin.json`、Skills、MCP、注册连接和生命周期 Hooks；Hooks 支持 manifest 指定路径 / 内联配置，并提供 `PLUGIN_ROOT`、`PLUGIN_DATA` 及 Claude 兼容变量。

仍需在实现中验证：Kivio 外部 CLI 当前版本的插件加载和动态刷新方式；Trellis 最新初始化模板的完整产物及最小 Kivio configurator；复杂插件的工具 schema、会话持久化和平台专属能力。上述源码阅读证明存在接入点和明确缺口，尚不构成“兼容已完成”的结论。
