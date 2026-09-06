---
name: kivio-diagnosing-commands
description: 创建或排查 Kivio 自定义斜杠命令、skill triggers 和插件 commands，解决 /命令 不触发、参数未替换、同名冲突；不用于调试普通 Shell 命令。
---

# Kivio 斜杠命令

依据 Kivio 2.9.6（2026-09-06）。先确认这是内置 Agent 的 skill 触发器、插件命令还是外部 CLI 原生命令。外部 CLI 自己的 `/命令` 不由 Kivio skill 目录负责。

## 新建一个个人命令

Kivio 普通自定义命令可做成技能，不需要发明 `~/.kivio/commands` 自动加载机制。创建 `~/.kivio/skills/review-changes/SKILL.md`（仅项目则 `<项目>/.kivio/skills/...`）：

```yaml
---
name: review-changes
description: 用户要求检查当前改动或调用 /review-changes 时提供改动评审。
triggers: [/review-changes]
argument-hint: <关注点>
arguments: [focus]
---
根据用户给的关注点评审改动：$ARGUMENTS
```

推荐先写完整目录，再通过 `kivio_configure` 的 `skill_install` 安装并用 `kivio_inspect` 的 skills 查看实际身份；所需工具受原开关/权限约束，正文不是可执行 Shell，不能把参数插入 shell 字符串直接执行。

## 当前解析规则

- 只识别用户文本**开头**（允许前导空白）的第一个 `/word`，后续是参数。精确匹配，不是前缀匹配；触发器忽略大小写。
- 自定义 `triggers` 之外还会有 `/<id>` 和 `/<slug(name)>` 默认触发器；name 和 id 改动会影响存储的禁用/引用关系。
- `$ARGUMENTS` 是去除首尾空白后的完整尾部文本；`arguments` 声明的名称按空白分词做位置映射，`$ARG_FOCUS`/声明名称对应的 `$FOCUS` 取相应词，缺失取空串。**不按 Shell 引号解析多词参数**。
- 未声明的 `$NAME` 保持原样；`$ARG_...` 特殊约定的未知参数为空。替换只扫描一次，不递归替换用户值里的 `$`。
- `disable-model-invocation:true` 只限制自动选取，用户显式斜杠可用；全局禁用、插件/连接器门控和助手白名单仍生效。
- 触发成功会固定该技能并展开正文，不会解锁它声明的工具。

## 插件命令

通用插件 `commands/*.md` 由插件加载器转换成 Skill；使用 `/plugin-name:command-name`，不要依赖无命名空间短命令。原始 Markdown 可以经加载器补足元数据，包解析时诊断决定是否可用。组件目录和插件清单要一起保留，不能只把 commands 文件复制到任意目录。

## 排障

用 `kivio_inspect {"topic":"skills"}` 看 command/skill 的真实 ID、source 和 path，再检查正文 frontmatter、triggers、开关与当前助手白名单。已安装却不触发时测试下一轮；同一轮 registry 已缓存。检查是否被前端内置命令拦截，优先换独特名字而不是占用内置命令。

没有可调用工具或当前是 Kivio Chat 时，不声称命令已执行。创建好命令后分别验证“目录发现”和“输入 `/命令 参数` 后正文按预期展开”，不要只检查文件存在。

源码：`src-tauri/src/skills/{types,parse,runtime,discover}.rs`；`src-tauri/src/chat/commands/tooling.rs::try_apply_skill_slash_trigger`；`src/chat` 的命令选择器。
