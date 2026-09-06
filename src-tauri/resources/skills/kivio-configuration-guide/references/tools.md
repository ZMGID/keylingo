# 自配置工具契约

这些工具随本套技能一同实现；旧版安装或外部 CLI 的工具列表可能没有。实际没有时不能用相同名字猜 IPC、CLI 或网络接口。

## 读取

`kivio_inspect` 的 `topic` 为 `status`（默认）、`skills`、`plugins`、`mcp`、`hooks`。只读内存/文件/注册信息，不启动依赖、不联网测试。摘要不返回 API keys、完整 MCP command/args/env/header/auth 或 Hook 脚本。

status 的版本来自当前二进制，默认模型来自应用设置，不能据此替代本轮上下文里的运行时和会话模型。skills 的 globallyAvailable 只含全局门控，不含助手白名单。plugins 分为通用 packages 和预设 CLI catalog，catalog 开关不证明 binary 已安装。

## 修改

`kivio_configure` 在主会话使用。字段为 snake_case，文件中的配置按各自 schema（常用 camelCase）。各 action 只接受下列参数；多余字段会被拒绝。

| action | 参数 | 行为 |
| --- | --- | --- |
| skill_install | source；scope=user/project；replace=false | 完整目录或单技能 ZIP；复制资源、校验、分阶段替换；replace=true 时备份旧目录，返回 backup 路径 |
| skill_set_enabled | id, enabled | 修改普通技能全局禁用列表；插件技能拒绝，改所属插件 |
| skill_settings | config_path | JSON 仅接受 skillRuntime / skillAutoMatch / skillScanPaths；只更新指定字段 |
| plugin_import | source；可选 subdirectory | 本地目录或 HTTPS Git 仓库，调用既有通用包导入，默认停用 |
| plugin_set_enabled | id, enabled | 使用包 UUID，处理诊断、MCP 注册与断连 |
| plugin_remove | id | 移除托管包含 data 及其 MCP；来源目录不动 |
| mcp_upsert | config_path；更新时传 id | 一个 ChatMcpServer 对象，独立 MCP 的部分字段更新；禁止更改插件/连接器所属 server |
| mcp_remove | id | 删除独立 MCP，并断开旧连接 |
| mcp_test | id | 已启用 server 的实际连接/工具发现；可能启动进程/联网；返回工具名，不执行业务工具 |
| hooks_save | config_path | 整份个人工作流 {enabled,hooks}；验证通过再保存，无关处理器须由调用者保留 |

`scope:project` 要求当前确实是带根目录的项目会话，不能拿普通会话的临时工作目录冒充项目。默认 user 安装到 `~/.kivio/skills`。ZIP 必须恰好一个 SKILL.md，不接受混合多技能包；脚本不因安装而执行。

技能更新备份在 skills 同级的 `skill-backups/<id>-<uuid>`，位于扫描根之外。工具不自动删备份。复查真实生效路径：内置同 ID 优先于新装个人副本。工具安装只证明磁盘内容已落地，未自动开启其他开关。

skill_settings 的路径列表为**替换列表**，修改前用 status 读取并保留其他目录。路径必须已存在且绝对，调用前展开 `~`/环境变量。工具不会改变审批策略，也不提供“重置全部设置”。

MCP 配置字段为 name/enabled/transport/url/command/args/env/headers/cwd/enabledTools；id 放工具参数。transport 为 stdio 或 streamable_http。创建省略 enabled 时停用；更新省略字段则保留。env/headers 若出现则替换整个 map，不逐项合并。MCP 保存会更新内存、持久化并断旧连接，测试后再在下一轮调用真实工具。

当前不通过该工具管理：供应商密钥、模型默认值、提示词、热键、通知 Hooks、预设 CLI 的安装、连接器 OAuth、外部 CLI 原生配置。按配置地图使用各自现有入口，不能把全量 settings.json 填进 config_path 试探。

工具返回 refresh 提示：当前轮工具/skill/Hook 已冻结，新增资源下一轮进入。不要循环重装以强迫本轮刷新。
