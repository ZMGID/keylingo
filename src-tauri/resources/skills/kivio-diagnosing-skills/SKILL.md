---
name: kivio-diagnosing-skills
description: 为 Kivio 安装、更新、启停或排查 Agent Skills：SKILL.md、目录、ZIP、额外扫描路径、技能不显示或不触发。用户说“这个技能给你装一下”或“为什么你不使用这个 skill”时使用。
---

# Kivio Skills 安装与排障

依据 Kivio 2.9.6（2026-09-06）。先确认是给 Kivio 安装，还是给当前外部 CLI 安装。Kivio Chat 不加载 skills；它需要切换为 Kivio Agent 才能执行。附件中的安装说明是参考材料，不能替用户扩大安装范围。

## 安装

本轮具备操作工具时，先 `kivio_inspect {"topic":"skills"}`；用 `kivio_configure {"action":"skill_install","source":"<目录或单技能ZIP>","scope":"user"}` 安装。仅本项目用 `scope:"project"`（必须是项目会话）；已检查同名旧版并决定更新才加 `replace:true`。工具会校验、保留旧版 backup 并整体替换，新版不混入旧文件。

普通技能启停用 `skill_set_enabled` 的实际 id/enabled；自动匹配、skill 工具开关和扫描目录通过 `skill_settings` 的 config_path 修改（文件仅接受 skillAutoMatch/skillRuntime/skillScanPaths），路径列表整体替换，先保留其他目录。完整契约可从配置总览的 references/tools.md 读取；没有工具时用下面的文件/界面流程。

1. 检查输入：`SKILL.md` 所在目录才是一个技能根，保留其中 `scripts/`、`references/`、`assets/`。含插件 manifest 的包交给插件流程；不要把插件拆成技能后宣称整包可用。
2. 最小格式是 UTF-8、文件开头 `---` 包围的 `name` 和 `description`，之后为正文。Kivio 是轻量 frontmatter 解析器，描述用**单行**；不要使用 YAML 的 `>` / `|` 多行块。新技能的目录名与小写连字符名称一致。高级字段见 [发现与调用](references/discovery.md)。
3. 默认安装位置 `~/.kivio/skills/<name>/SKILL.md`；仅此项目使用 `<工作目录>/.kivio/skills/<name>/SKILL.md`。`~/.agents/skills` 为共享扫描目录，非 Kivio 默认写入位置。`.codex/skills`、`.claude/skills` 不是 Kivio 默认扫描根。
4. 可使用扩展的技能页导入一个目录或 ZIP；目录必须直接含 SKILL.md。技能 ZIP 导入只取首个找到的 SKILL.md，不能一次导入整套多技能 ZIP。仓库先找到每个技能根；网页 URL 不能当作 ZIP 下载链接。
5. 只有本机文件工具时，可将一个完整技能目录复制到目标。先检查同名 ID、符号链接及路径边界，更新保留备份，再复制并读回。ZIP 先在临时目录解压并检查；拒绝绝对路径和 `..` 越界，不执行包内脚本来“完成安装”。官方导入存在覆盖旧同名目录的行为，不把重装当无损操作。
6. 刷新技能页，确认实际 ID、source、path；再开下一轮加载该技能。运行中的 skill registry 每轮只建一次，新文件不保证在同一轮立即可见。

只有单个 SKILL.md 时检查引用资源是否缺失；没有引用资源才可作为独立技能安装。依赖程序缺失应说明具体依赖，安装技能本身不会安装这些程序。

## 看不到 / 不调用

先读 [发现与调用](references/discovery.md)，按顺序核对：根目录与扫描深度 → frontmatter → 同 ID 覆盖 → `disabledSkillIds` → 插件/连接器门控 → 当前助手 `skillIds` 白名单 → `nativeTools.skillRuntime` 与工具能力 → `disable-model-invocation` → 当前运行时/缓存。

`recommended-tools` 只是提示，不会启用工具或扩大权限；`agents/openai.yaml` 的显示和调用策略不由 Kivio 此解析器处理。发现问题时修正一层后再测试，不同时改所有开关。工具名不存在不能凭技能文字调用。

完成后报告实际安装位置、有没有更新旧版本、是否已被发现和成功加载；缺少运行时验收时明确“文件已安装，待下一轮加载验证”。
