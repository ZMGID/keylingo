# Kivio 自配置技能与操作工具

用户可以直接说“把这个 skill 给你装上”“这个插件为什么不生效”“帮我接这个 MCP”。模型先按资源类型加载对应指南，再用原生工具检查和操作，最后区分安装、发现、启用、实际调用的结果。

本次调查基于 2026-09-06 的 Kivio 2.9.6 工作树，初始 HEAD `226368e23b048af648a93adaba198530dfb34e50`。ZCode 截图仅作为能力分类参考，未复制其规则。调查以本地实现为依据，没有把第三方安装说明视为用户指令，也没有安装第三方插件来做调研。

## 调查结果与设计依据

| 查明的事实 | 实现依据（仓库相对路径） | 对技能设计的影响 |
| --- | --- | --- |
| 内置 skills 由 Tauri resources 分发并自动扫描 | `src-tauri/tauri.conf.json`；`src-tauri/src/skills/discover.rs` | 直接增加七个内置目录，无额外安装器或个人目录硬编码 |
| 普通 skill 内置优先、项目其次，同 ID 第一份生效 | `skills/discover.rs::{scan_root_entries,dedup_records}` | 不能建议通过同名个人副本覆盖内置技能 |
| frontmatter 是轻量解析，不是完整 YAML 语义 | `skills/parse.rs` | 单行 description；不把 allowed-tools 当权限配置 |
| 模型原来只拿到 skill 加载器，没有通用配置操作工具 | `mcp/{types,native_registry}.rs` | 增加 kivio_inspect / kivio_configure，避免教模型调用不存在的 CLI 或把 Tauri IPC 当模型工具 |
| settings.json 有 Store 外壳，设置同时存在内存中 | `settings.rs::{load_settings,persist_settings}`；`state.rs` | 修改在最新设置锁内合并、保存，再更新内存；不写运行中的裸 JSON |
| 通用插件已有校验/托管副本/启停/MCP 清理 | `plugins/packages.rs` | 复用现有包服务，导入不执行安装脚本，诊断非空不绕过启用限制 |
| Hook 有旁路通知与同步工作流两套机制 | `chat/hooks.rs`；`chat/workflow_hooks.rs`；`plugins/packages.rs` | 分开说明事件、格式、单位及可影响的执行范围 |
| 内置 Chat 不加载 skill；外部 CLI 有独立配置与原生会话 | `chat/agent/prepare.rs`；`chat/commands/tooling.rs`；`external_agents/` | 自配置目标必须先分清宿主，导入会话仍绑定原 CLI |
| 工具和技能注册信息每轮冻结；前端设置有缓存 | `skills/runtime.rs`；`chat/agent/`；`src/api/settingsCache.ts` | 返回下一轮刷新提示；操作成功通过无敏感载荷的事件刷新设置缓存 |

## 七个内置技能

位于 `src-tauri/resources/skills/`，每个入口含独立触发描述；只在相关任务中加载正文。可直接从技能页查看，或自然语言自动匹配：

- [kivio-configuration-guide](../../src-tauri/resources/skills/kivio-configuration-guide/SKILL.md)：身份、配置地图、操作路由和工具契约。
- [kivio-diagnosing-skills](../../src-tauri/resources/skills/kivio-diagnosing-skills/SKILL.md)：技能安装、备份更新、优先级、发现和激活。
- [kivio-diagnosing-plugins](../../src-tauri/resources/skills/kivio-diagnosing-plugins/SKILL.md)：通用包、预设 CLI、连接器分类及兼容诊断。
- [kivio-diagnosing-mcp](../../src-tauri/resources/skills/kivio-diagnosing-mcp/SKILL.md)：独立 MCP 配置、连接测试、所有权和过滤排查。
- [kivio-diagnosing-hooks](../../src-tauri/resources/skills/kivio-diagnosing-hooks/SKILL.md)：两套 Hook 与同步输入输出协议。
- [kivio-diagnosing-commands](../../src-tauri/resources/skills/kivio-diagnosing-commands/SKILL.md)：skill triggers、参数替换和插件命名空间。
- [kivio-diagnosing-runtime](../../src-tauri/resources/skills/kivio-diagnosing-runtime/SKILL.md)：内置/外部运行时、CLI 检测、模型和会话绑定。

Kivio 不读取 `agents/openai.yaml` 的调用策略，因此这套资源采用 Kivio 已支持的简单 frontmatter，没有额外生成不会生效的元数据。

## 操作实现

`src-tauri/src/self_config/` 提供两个原生工具，在 `mcp/native_registry.rs` 统一注册，遵守原有审批/会话授权。inspect 跟随读取开关；configure 跟随命令执行开关，并限制主会话使用；规划模式和 Kivio Chat 的原有筛选保持有效。没有新增监听端口、任意代码执行 API、通用 settings 覆盖或免审批通道。

工具支持个人/项目 skill 安装（目录或单技能 ZIP）、备份更新、普通 skill 启停和扫描设置、通用插件导入/启停/移除、独立 MCP 增量配置/移除/测试、个人工作流 Hooks 保存。新技能采用分阶段复制、验证后替换；拒绝路径越界、链接、多技能 ZIP 和过大输入，旧版备份位于扫描根之外。

插件操作复用 packages 服务；MCP 更新保留未指定字段，并拒绝直接改插件/连接器所属 server。配置文件参数不是整份 settings Store。详情与例子见 [工具契约](../../src-tauri/resources/skills/kivio-configuration-guide/references/tools.md)。

当前没有通过新工具操作供应商密钥、默认模型、热键、通知 Hooks、OAuth、预设 CLI 安装或外部 CLI 原生设置；技能会给出它们已有的设置入口。这套能力并不声明自动处理所有客户端所有配置。

## 验证与维护

已有 Rust 测试覆盖：新技能按 Kivio 解析器加载、自动可发现属性、安装资源完整性、覆盖保护/备份、无效更新保留旧版、ZIP 越界/多技能拒绝、MCP 增量更新保留凭据及其他 server、所有权保护、秘密脱敏、窄配置字段限制、工具注册顺序与权限元数据。前端测试覆盖后台配置变化通知、订阅者更新、旧异步读取不覆盖新设置。

Windows Rust 测试使用 `scripts/win-cargo-test.ps1 --lib self_config`、`--lib mcp::native_registry`、`--lib skills::`；前端使用 `npm test` / `npx tsc --noEmit`。这不等于已对真实服务、实际模型选择及所有平台进行端到端验收。

本次执行结果：七个技能通过 skill-creator 校验及 Kivio 解析器测试；13 份指南文件的相对链接检查通过；上述 Rust 分组共 64 项通过；前端 177 个测试文件、1,376 项通过；TypeScript、协议生成物检查和生产 UI 构建通过。本次变更的前端文件单独 lint 通过；全仓 lint 因已有 `output/playwright/plugin-tabs.tsx:33` 的 `react-refresh/only-export-components` warning 未通过，未修改该无关临时文件。未启动新构建的桌面应用或连接真实第三方服务进行验收。

手动验收建议在测试账户/测试项目完成：

1. 重编译并启动带有本次改动的 Kivio，选择内置 Agent，开启技能、本机读取和命令工具。
2. 发送“看看你现在的配置”，检查实际选择指南并调用 inspect，输出不含凭据。
3. 发送“把这个本地技能装给你”，下一轮通过 skills inspect 和 skill 调用验收；重装同名时应保留备份。
4. 使用已有 `tests/fixtures/plugins/kivio-example` 导入和启用，再验证它的命令/Hook；检查托管副本与来源目录分离。
5. 添加测试 MCP、验证工具发现、停用/移除；确认插件所属 MCP 不能被独立操作覆盖。
6. 在设置页保留草稿后从会话执行配置操作，确认刷新/重开后看到新状态，无关字段仍保留。

如果以后修改扫描优先级、frontmatter、配置字段、包格式或 Hook 协议，同时更新相应技能参考与行为测试。技能里的源码定位用于维护，普通用户不需要持有源码仓库。
