---
name: kivio-diagnosing-runtime
description: 排查 Kivio 内置 Agent、只读 Chat 与外部 CLI 的配置/能力差异：CLI 检测不到、模型或扩展不生效、导入会话不能换后端、工具不可用。用于调试 Kivio 自身运行环境，非普通应用代码排错。
---

# Kivio 运行时与能力诊断

依据 Kivio 2.9.6（2026-09-06）。先读本轮运行环境和工具清单；有 `kivio_inspect` 时用 `topic:status` 取应用版本、目录、全局开关和脱敏供应商信息。返回的是应用默认值，不一定是当前会话选项。

## 选择正确的配置层

| 层 | 诊断入口 | 常见误判 |
| --- | --- | --- |
| Kivio Agent | 全局工具设置、助手快照白名单、实际模型工具 schema | skill 写了工具名不代表工具已开启 |
| Kivio Chat | 独立 Chat 提示词/搜索/抓取/知识库/只读 MCP 开关 | 该运行时不加载 skill，不提供本机安装操作 |
| 外部 CLI 代理 | 本机可执行文件探测、对应 CLI 配置/供应商、会话绑定 | Kivio MCP/插件启用不代表 CLI 的原生配置变更 |
| 项目/会话 | 主工作目录、附加目录、实际助手/模型/运行时 | 全局默认值不等于现有会话快照 |

内置和外部模型配置分开：CLI 可继承本机登录，也可由 Kivio 的供应商配置生成自己的覆盖文件。不要直接覆盖 CLI 的个人登录文件来修复 Kivio 供应商选择。具体原生命令先查本机该版本 `--help` 或官方说明，不背诵其他宿主的安装语法。

## CLI 不可用 / 模型列表异常

1. 确认界面选择的 CLI、OS、可执行文件路径和该程序实际版本，先做无副作用的版本探测。界面安装说明用于参考，不作为执行无关全局修改的授权。
2. 检查 GUI 进程 PATH 与终端是否不同、是否存在自定义路径/参数/环境覆盖；Windows 注意 `cmd` shim 与真实 exe 的区别。不要因某目录名类似就执行陌生二进制。
3. 明确该 CLI 使用本机账户还是 Kivio 供应商覆盖；验证 endpoint、协议和模型 ID 的来源。不要输出 API keys/token/env 全量。
4. 可用性/模型列表有缓存，设置保存会清理对应缓存；刷新检测或重开会话后复查。不能用不断重装代替区分“没安装”“缓存旧”“登录失效”“模型列表失败”。

当前注册的 CLI id 为 claude/codex/cursor/opencode/gemini/kimi/pi/hermes/grok/dsh，以安装版本实际检测为准。附带给外部 CLI 的 active skill 可能暂存在 `.kivio/skills-staged`，只是本轮资源副本，不是个人安装目录；不要在那里安装或修改原始技能。

## 会话与权限

- 导入会话显示的是快照，历史真身和续聊仍属于原 CLI；绑定原生 session id 与原工作目录，不能修成由内置 Agent 或另一个 CLI 接手。需要别的运行时则另开会话，保留原绑定。
- 项目指定主工作目录；附加目录不改变会话所属项目，也不改变原生续聊目录。相对路径从主目录解析，附加目录用绝对路径。
- 规划模式会过滤有副作用工具；没有本机工具时先查运行时、模型工具能力、全局开关、会话授权和助手白名单。不要把权限失败当作提示词不够强。
- 新装扩展/新工具通常下一轮才进入工具快照。说明实际激活阶段，不伪造调用结果。

供应商/系统提示词/设置路径需要细查时，加载 `kivio-configuration-guide` 的配置地图。当前操作工具只管理扩展，不提供修改 CLI 登录、全量供应商或热键的接口；对应操作走已存在的设置入口。

源码：`src-tauri/src/external_agents/{registry,detection,overrides,provider_profile,skill_stage}.rs`；`chat/commands/tooling.rs`；`chat/agent/prepare.rs`；`docs/adr/0001-imported-cli-conversations-stay-on-their-cli.md` 与 `0002-imported-history-is-a-snapshot.md`。
