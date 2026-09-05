# Kivio Plugin v1

状态：已实现的个人范围插件格式，供内置 Kivio Agent 使用。

## 包入口与目录

插件是一个目录，必需入口为 `.kivio-plugin/plugin.json`。组件位于包根，不能放在 `.kivio-plugin` 内。

```text
my-plugin/
  .kivio-plugin/plugin.json
  skills/review/SKILL.md
  commands/check.md
  agents/reviewer.md
  hooks/hooks.json
  scripts/check.cjs
  .mcp.json
```

最小清单：

```json
{"schemaVersion": 1, "name": "my-plugin", "version": "1.0.0"}
```

完整常用清单（声明的路径必须真实存在）：

```json
{
  "schemaVersion": 1,
  "name": "my-plugin",
  "version": "1.0.0",
  "description": "代码检查与评审",
  "skills": "./skills/",
  "commands": "./commands/",
  "agents": "./agents/",
  "hooks": "./hooks/hooks.json",
  "mcpServers": "./.mcp.json",
  "metadata": { "author": "Your team", "license": "MIT" }
}
```

## 字段约定

| 字段 | 类型 | 含义 |
| --- | --- | --- |
| schemaVersion | 必填整数，当前为 1 | 格式协议版本；未知版本拒绝导入 |
| name | 必填字符串 | 1–64 个小写字母、数字；单个连字符分隔非空片段；用于命名空间 |
| version | 必填非空字符串 | 插件发布版本，建议 `1.2.3`；当前不比较版本、不解析依赖范围 |
| description | 可选字符串 | 插件管理页介绍 |
| skills / commands / agents / hooks / mcpServers | 可选字符串或字符串数组 | 组件路径，见下面的发现规则 |
| metadata | 可选对象 | 作者、许可证、仓库等附加信息；不影响运行，目前不专门展示 |
| $schema | 可选字符串 | 编辑器 schema 引用；加载器不联网获取 |

未知顶层字段报错，以免拼写错误或尚未实现的能力静默失效。扩展性描述放进 `metadata`。机器可读定义在 [JSON Schema](../schemas/kivio-plugin.v1.schema.json)，运行时还校验目标存在性和路径边界。

`schemaVersion` 表示格式版本，不能用来表达最低 Kivio 应用版本。v1 尚无应用版本约束、依赖声明或安装脚本协议。

## 组件发现

| 组件 | 省略字段时的默认位置 | 文件格式 |
| --- | --- | --- |
| skills | skills/ | 每个技能目录中的 SKILL.md |
| commands | commands/ | Markdown + YAML frontmatter |
| agents | agents/ | Markdown + YAML frontmatter；当前不支持 TOML |
| hooks | hooks/hooks.json | 工作流事件 JSON |
| mcpServers | .mcp.json | MCP server map JSON |

默认路径不存在时跳过。显式路径替代该组件的默认位置；数组 `[]` 明确关闭该组件的默认发现。原生清单不接受内联 hooks/MCP 对象，配置放在独立文件，便于查看和复用。多个配置文件依次加载；不声明依赖执行顺序。

路径相对包根，以 `./` 开头，使用 `/`，禁止 `..`、绝对路径、反斜线与 `:`。安装导入拒绝符号链接和越界目录链接。脚本及 references 等资源与组件一起保留在包内。

技能与命令命名空间为 `my-plugin:review`、`/my-plugin:check`；子代理使用 `my-plugin:reviewer`。同一类组件应使用唯一名称。完整的 Skill/agent 元数据语义沿用 Kivio 现有解析器。

## 原生 Hook

沿用 [工作流 Hook 协议](plugin-hooks.md)：SessionStart、UserPromptSubmit、SubagentStart、PreToolUse、PostToolUse。处理器类型为 command，使用 stdin JSON / stdout JSON，支持超时、取消、上下文、工具参数替换和审批结果。

```json
{
  "hooks": {
    "PreToolUse": [{
      "matcher": "write|edit",
      "hooks": [{
        "type": "command",
        "command": "node",
        "args": ["${PLUGIN_ROOT}/scripts/check.cjs"],
        "timeout": 10
      }]
    }]
  }
}
```

原生包使用 Kivio 工具名（例如 `write`）和参数（例如 `tool_input.path`），保留带插件命名空间的 agent 名称，不做 Claude 工具名转换。`PLUGIN_ROOT` 是安装内容目录，`PLUGIN_DATA` 是该插件的可写数据目录。环境变量通过环境传入；直接启动模式的 args 会展开这些占位符。

## 原生 MCP

建议使用 `mcpServers` 包装对象：

```json
{
  "mcpServers": {
    "local-helper": {
      "command": "node",
      "args": ["${PLUGIN_ROOT}/scripts/server.cjs"]
    }
  }
}
```

支持 stdio 与 Streamable HTTP（`type: "http"`、`url`）。加载器也接受直接 server map 和 Codex 的 `mcp_servers` 包装。MCP 服务名应唯一，注册时带插件身份；停用/移除会撤销所属注册。环境变量缺失产生诊断，依赖程序不自动安装。

## 安装及兼容

扩展 → 插件 → 通用插件，导入本地包根或 HTTPS Git 仓库及子目录。导入后默认停用，启用后允许执行脚本，作用域为个人。卸载只删除托管副本（包含插件 data）及所属 MCP 注册。

清单优先级为 `.kivio-plugin` → `.codex-plugin` → `.claude-plugin`，只加载最高优先级的一份，解析失败不回退。不修改来源仓库的清单。原生包与外部格式共享内部运行设施，但外部格式有独立兼容规则。

当前没有原生 marketplace 索引、项目作用域、自动更新、安装器、平台托管 apps、LSP 或额外 Hook 事件；这些不属于 v1 格式承诺。

可直接导入的示例：`tests/fixtures/plugins/kivio-example`。示例依赖 Node.js，其 Hook 只演示拒绝写入 `protected` 目录的操作，不修改文件，也不是文件系统安全边界。
