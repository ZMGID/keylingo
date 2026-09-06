# 配置地图

这里的 `<app_data>` 指应用数据目录，不是 skill 目录，也不是当前项目。

| 平台 | `<app_data>` |
| --- | --- |
| Windows | `%APPDATA%\com.zmair.kivio`（Roaming，下方没有 `data` 子目录） |
| macOS | `~/Library/Application Support/com.zmair.kivio` |
| Linux 路径实现 | `$XDG_DATA_HOME/com.zmair.kivio`，未设时为 `~/.local/share/com.zmair.kivio`；不因此承诺 Linux 桌面发行支持 |

不要在未知目录找不到文件后立即创建“默认配置”。先确定当前账户和实际安装路径。旧标识 `com.zmair.keylingo` 是迁移来源，非当前写入目标。

## 设置文件与生效

`<app_data>/settings.json` 是 Tauri Store，形状是 `{"settings": { ... }}`；设置字段为 camelCase。不应将 `chatTools` 写在文件顶层。主要字段：

| JSON 路径（从文件根开始） | 用途 |
| --- | --- |
| `settings.providers` | 供应商列表，含密钥；不要完整打印 |
| `settings.defaultModels` | chat / vision / titleSummary / compression / imageGeneration / promptOptimize / advisor 各路由 |
| `settings.chat` | 内置 Agent 的聊天参数和系统提示词（具体字段读现有设置） |
| `settings.chat.chatMode` | Kivio Chat 自己的提示词及能力开关 |
| `settings.chatTools` | MCP、skills、工具开关与通知 Hooks |
| `settings.chat.externalCliAgents` | 外部 CLI 配置；修改前核对实际对象结构 |

读取时只投影需要的字段，密钥只说明是否已配置；不要输出 API keys、MCP env/header/auth 值或带认证信息的 URL。工具返回的原始错误、进程参数也可能包含秘密。

**优先应用设置入口。** 保存设置除了写盘，还更新内存、外部 CLI 配置派生文件和探测缓存；热键等另有运行时重应用。当前没有文件监控保证手工写盘热加载。

只有确需离线修复、且可以在 Kivio 完全退出后操作时才编辑此文件：先解析并备份原文件，保留外层 Store 其他键和无关设置，只改已核对字段，写 UTF-8 JSON 后重新解析，再启动应用验证。正在执行任务的 Kivio Agent 无法在退出宿主后继续运行；此时交付具体修改方案供设置界面执行，不安排后台脚本抢写或强杀宿主。解析失败要检查备份，不能拿默认设置覆盖来“修复”。

## 模型与提示词

供应商协议值为 `openai_chat` / `openai_responses` / `anthropic_messages` / `gemini` / `xai_responses`。模型名称包含 GPT、Claude、Gemini 并不足以判断网关协议，按服务商实际接口配置。Kivio 自带 Key，没有内置统一账户能代替用户的供应商认证。

先分清用户改的是当前会话模型、**新会话默认模型**还是图片/总结/压缩等副任务模型。空路由有各自回退逻辑；`advisor` 未配置意味着关闭，不是自动继承。API Key、endpoint 和模型 ID 分别核验，最小测试用目标功能的一次请求。

提示词有全局聊天、Kivio Chat、集的提示词和助手提示词等来源。助手配置在会话中可能是快照，修改定义不代表已有会话立即换了快照。项目表示工作目录，并不证明内置 Agent 会自动读取 `AGENTS.md`、`CLAUDE.md` 或一个自行发明的 `KIVIO.md`。用户要求写说明文件可以写；要说明实际由谁读取，不能承诺不存在的自动加载。

## 其他存储

- 个人 skills：`~/.kivio/skills`；共享 `~/.agents/skills` 和旧 `<app_data>/skills` 是扫描来源。
- 通用插件：`<app_data>/plugins/packages/<uuid>/{record.json,content/,data/}`。
- 预设 CLI 插件：`<app_data>/plugins/<catalog-id>/meta.json`，与通用插件分开。
- 个人工作流 Hooks：`<app_data>/plugins/workflow-hooks.json`；可写数据位于同级 `hook-data/`。
- 个人子代理：`<app_data>/agents/*.md`；项目子代理：`<项目>/.kivio/agents/*.md`；不要套用 skill 的优先级，它们使用另一套合并规则。
- 对话：`<app_data>/conversations/`；不要为排扩展故障删对话。

## 源码定位（维护时使用）

仓库相对位置：`src-tauri/src/app_data.rs`；`settings.rs::{ChatToolsConfig,ChatMcpServer,ProviderApiFormat,persist_settings,load_settings}`；`commands.rs::{save_settings,apply_settings}`；`chat/agent/prepare.rs::build_chat_system_prompt_with_segments`；`agents/mod.rs`。这些文件不会随普通 skill 单独分发，不能要求终端用户必须拥有源码。
