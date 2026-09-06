# 发现、优先级与调用

普通扫描同 ID **第一份优先**，按以下顺序：

1. 应用资源目录中的内置 `skills/`。
2. 项目：从工作目录向上，每层先 `.kivio/skills` 再 `.agents/skills`，遇 `.git` 或家目录停止，最多 24 层。
3. 已启用预设插件的技能根。
4. `~/.kivio/skills`。
5. 旧 `<app_data>/skills`。
6. `~/.agents/skills`。
7. `settings.chatTools.skillScanPaths`，按设置顺序。

这里 `settings.` 指 `<app_data>/settings.json` 中的外层 Store 键。内置优先于项目，不能靠放同名个人技能覆盖内置。跨来源覆盖通常不报警，同来源重复 ID 才有 warning。通用插件的组件另外追加，ID 带包 UUID，显示名带 `<plugin>:<skill>`。

每个根递归最多 6 层；到达包含 SKILL.md 的目录即记录该技能并停止向下扫描。忽略 `.git`、`node_modules`、`.svn`、`.hg`。额外扫描路径直接作为路径处理，不要假定会展开 `~` 或环境变量，填写已解析的绝对路径。

## Frontmatter 的实际语义

```yaml
---
name: review-changes
description: 根据用户要求检查当前项目改动并给出可执行的评审意见。
triggers: [/review-changes]
argument-hint: <检查重点>
arguments: [focus]
---
检查重点：$ARGUMENTS
```

- 必需 `name`、`description`；可选 `id`，省略时由 name slug 生成。不要用改 name 的方式修复已存储的开关 ID，除非确实要改身份。
- `disable-model-invocation: true` 从自动目录隐藏，显式选择/斜杠仍可用；不能绕过全局禁用或助手白名单。
- `recommended-tools`、`mcp-tools`、`allowed-tools` 被读成推荐工具，**不是**工具权限配置。
- `triggers`、`argument-hint`、`arguments` 供斜杠机制使用。描述只用单行，列表只用简单标量。

全局 `chatTools.disabledSkillIds` 缺省表示启用；同时要满足插件状态/Obsidian 连接器要求。具备助手快照时按 `skillIds` 白名单过滤，空列表可导致全部不可用。`chatTools.nativeTools.skillRuntime` 控制 skill 工具；模型不支持工具时只能依实际 fallback 读取正文，不能假装调用了脚本。

## 操作入口与验证

技能页提供导入目录/ZIP、打开个人目录、启停、额外扫描目录等现有功能。开发接口为 `chat_skills_list`（可传 projectCwd）、`chat_skills_read`、`chat_skills_import`、`chat_skills_install_from_url`、`chat_skills_uninstall`；这些是 Tauri IPC，不是默认对模型暴露的工具。删除仅处理 Kivio 拥有的个人内容，不能拿共享来源或内置目录当个人副本删除。

运行时 `skill` 工具形状为 `{"name":"目录中实际显示的名称"}`。它返回正文、实际 Skill directory 和资源路径；后续 `read`/`bash` 必须以该目录解析相对路径。`skill` 只加载，不负责安装或运行所有脚本。

源码：`src-tauri/src/skills/{discover,parse,types,runtime,catalog,mod}.rs`；`src-tauri/src/settings.rs::skill_global_unavailable_error`；`src-tauri/src/chat/agent/prepare.rs::skill_allowed_for_conversation`。
