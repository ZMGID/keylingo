# 包格式与兼容范围

清单优先级：`.kivio-plugin/plugin.json` → `.codex-plugin/plugin.json` → `.claude-plugin/plugin.json`。只选一份，选中的解析失败不会回退；不要把多个清单的组件合并执行。

原生最小清单：

```json
{"schemaVersion":1,"name":"example-helper","version":"1.0.0"}
```

组件默认位置位于包根：`skills/`、`commands/`、`agents/`、`hooks/hooks.json`、`.mcp.json`。显式字段可指字符串或数组路径，覆盖该组件默认位置，`[]` 关闭默认发现。路径必须以 `./` 开头、使用 `/`、保持包内；禁止绝对路径、`..`、反斜线和 `:`。原生 hooks/mcpServers 只接受文件路径，不接受内联对象。导入拒绝符号链接及越界目录链接。

原生 `schemaVersion` 当前只能为整数 1；`name` 是 1–64 位小写字母/数字与单连字符分隔的非空片段；`version` 非空，当前不做依赖或版本范围解析。未知顶层字段报错，描述性信息放 `metadata`。`schemaVersion` 不能表示最低应用版本。

外部 Claude/Codex 包有自己的路径与内联配置兼容规则，不要用原生字段限制机械重写它们。让加载器返回诊断。每个组件必须可实际使用；未知事件、不支持的组件会阻止启用，而不是只加载其中一半。

## 实际运行语义

- 安装根：`<app_data>/plugins/packages/<UUID>/content`；`record.json` 在 content 同级，持久数据在 `data/`。
- 技能/命令显示 `<plugin>:<name>`，斜杠推荐 `/plugin:command`；内部 ID 是 `pkg-<UUID>-<原ID>`。禁用表不能写一个猜测的短 ID。
- MCP ID 带 `plugin-package-<UUID>-` 前缀。启用调用注册，停用/移除调用撤销和断连。技能 source 为 `plugin` 时从所属插件控制，不在普通技能页改。
- 环境变量缺失、依赖程序缺失须单独处理，导入不会自动安装 Node/Python 等依赖。
- Hooks 支持 `SessionStart`、`UserPromptSubmit`、`SubagentStart`、`PreToolUse`、`PostToolUse` 的 command 处理器；原生包用 Kivio 工具名和参数，Claude/Codex 格式有工具名兼容转换。详见 Hook 技能。

目前不承诺：原生 marketplace、项目范围通用插件、自动更新、依赖自动安装、安装脚本协议、LSP、output styles、平台托管 apps、专有 UI、TOML 子代理、context: fork、内嵌组件 hooks 或额外事件。用户给的是多宿主项目初始化工具时，先核对其产物；它可能不是插件包。

源码：`src-tauri/src/plugins/{packages,catalog,install,lifecycle,state}.rs`；`src-tauri/src/skills/discover.rs`；仓库文档 `docs/agents/kivio-plugin-format.md` 与 `docs/agents/plugin-hooks.md`。早期 research 文档记录“尚未实现”不代表当前版本，冲突以当前代码和加载器诊断为准。
