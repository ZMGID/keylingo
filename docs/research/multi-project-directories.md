# 一个对话跨多个项目目录

**Date:** 2026-08-28  
**Scope:** 产品调研。场景是「base 库升级、多个业务工程一起改」——用户要在**一条对话**里读写多个互不嵌套的仓库。  
**Not in scope:** 代码改动。

Kivio 词汇（`CONTEXT.md`）：**项目** = 带一根目录的对话分组；**工作目录** = 原生会话创建时所在目录（也决定这条会话属于哪个项目、部分 CLI 能不能续上）；**集** 与项目互斥；**外部 CLI 代理** / **内置运行时** 是一条对话上二选一。本文把「主工作目录之外再授权的那些文件夹」暂称为**附加目录**——词汇表里还没有这个词。

---

## 结论

行业已经收敛成同一模型，不是「一条对话属于 N 个项目」：

> **一条会话有且只有一个主工作目录（相对路径从这里解）。另外再挂一份附加目录列表，用来扩文件系统授权，不改主目录。**

Kivio 现在缺的是这份附加目录列表及其 UI / 下发，不是从零发明工作区。项目仍应是「对话分组 + 一根主目录」；跨仓改动挂在**对话**上（这次升级任务），而不是把一条对话同时塞进多个项目。

内置运行时其实已经能写附加目录：相对路径绑项目根，绝对路径 / `~/` **不受项目根约束**。缺口是模型不知道还有哪些仓、Dock/Git 只盯一根目录、外部 CLI 的沙箱没把那些仓加进授权。

---

## 行业怎么做

### 反复出现的四种形状

| 形状 | 谁在用 | 一句话 |
| --- | --- | --- |
| **主目录 + 附加目录** | Claude Code、Codex CLI、Gemini CLI、OpenCode、ACP | 相对路径仍走主目录；额外路径进 allowlist |
| **编辑器多根工作区** | VS Code / Cursor Agents Window / Cline | `.code-workspace` 里多根 folders；第一根往往是 primary |
| **只读旁路** | Aider | 一次只编一个 git 仓；别的仓用 `/read` 当参考 |
| **父目录当伞** | 所有产品的无功能退路 | 把多个仓放进同一个父文件夹再绑定——仓不在同一棵树下就废 |

用户要的「base 升完、业务仓一起改」是第一种（或第二种）的读写版，不是 Aider 那种只读旁路。

还有两个**不要跟附加目录搞混**的功能：

- **Git worktree**：同一仓库的并行 checkout，不是跨仓。
- **子代理各绑一仓**：编排层；Claude Code / Cursor 有，但都建立在「先能看见多根目录」之上。

### 产品对照

| 产品 | 官方名字 | 用户怎么挂 | 主目录还在吗 | 写附加目录？ | 配置挂在哪 | 已知坑 |
| --- | --- | --- | --- | --- | --- | --- |
| Claude Code | `--add-dir` / `/add-dir` / `permissions.additionalDirectories` | 启动 flag、会话中 slash、settings.json | 是。改主目录用 `/cd`（v2.1.169+），跟 `/add-dir` 不是一回事 | 是，权限跟主目录同一套 | 会话级 flag；项目级 settings 持久 | settings 那条**只授权文件**，不加载对方的 skills/commands/subagents；`--add-dir` 会加载 `.claude/skills|commands|agents`。`CLAUDE.md` 默认不从附加目录读，要 `CLAUDE_CODE_ADDITIONAL_DIRECTORIES_CLAUDE_MD=1` |
| Codex CLI | `--add-dir`；config `sandbox_workspace_write.writable_roots` | 可重复 flag；`config.toml` | 是（cwd / `-C`/`--cd`） | `workspace-write` 下把附加路径加成 writable root。官方建议用 `--add-dir`，别开 `danger-full-access` | 会话 ephemeral vs config 持久 | Kivio 已经用 app-server 的 `runtimeWorkspaceRoots`，不是 CLI `--add-dir`。社区 issue：`apply_patch` 有时不吃 `--add-dir` 白名单 |
| Gemini CLI | `--include-directories`；`/directory add`；config `includeDirectories` | flag（可逗号/可重复）、slash、settings | 是 | 进 workspace | 启动 + 会话中 + 配置 | 官方写最多 **5** 个附加目录；restrictive sandbox 下 `/directory add` 禁用，只能启动时带 |
| OpenCode | `/add-directory`；permission `external_directory` | slash；`opencode.json` | 是（`--dir` 才换主目录） | 授权后可读写 | 会话 permission set；全局/项目 config | 官方 CLI 没有 Claude 那种 `--add-dir`；靠 permission 规则。实验开关 `OPENCODE_EXPERIMENTAL_WORKSPACES` |
| ACP（协议） | `additionalDirectories` | Client 在 `session/new` / `load` / `resume` 里带 | **必须**。相对路径仍以 `cwd` 为基。有效根集合 = `[cwd, ...additionalDirectories]` | 协议把根集合当成工具边界（SHOULD） | 每次生命周期请求都要**整表重发**；省略或 `[]` = 本会话没有附加根，不会从上次隐式恢复 | Client **只有** Agent 声明了 `sessionCapabilities.additionalDirectories` 才能发。Kivio 今天 **没发这个字段** |
| Cursor | Multi-root workspaces in Agents Window | 可复用的多 folder workspace | 第一根经常被当成 primary | 设计上跨仓改 | workspace 级，不是单条 Agent 会话私有 | 2026-04-24 changelog。论坛：Shell `working_directory` 会静默落到第一根；Grep 会扫全部根；非第一根的文件链接点不开。Worktree / 异步子代理是另一条线 |
| Cline | VS Code multi-root | `File > Add Folder to Workspace` / `.code-workspace` | 第一 folder = primary | 跨 folder 读写、按仓独立认 git | 编辑器 workspace | `.clinerules` / skills / `@git` 只看 primary；checkpoints 在多根下直接关掉。`@workspace-name:path` 消歧 |
| Aider | （无原生多仓） | `/read` / `--read` 只读外仓；或从公共父目录启动 | 一次一个 git 仓 | 外仓默认只读 | 无 | 官方 FAQ：「Currently aider can only work with one repo at a time」 |
| Pi | 无授权目录 flag | — | 只有启动目录 | 靠 Pi 自己的文件权限，Kivio 塞 `--append-system-prompt` 是错的（已修） | — | `defs/pi.rs` 写明 `--help` 没有 `--add-dir` 等价项 |

### Claude Code：两套「附加」不要当成一个

来源：[CLI reference](https://code.claude.com/docs/en/cli-reference)、[permissions](https://code.claude.com/docs/en/permissions)、[slash-commands](https://code.claude.com/docs/en/slash-commands)。

1. **`--add-dir` / `/add-dir`（含 Agent SDK `additionalDirectories` → 转成 `--add-dir`）**  
   授权读/写，并且会加载该目录下的 `.claude/skills/`、`.claude/commands/`、`.claude/agents/`。其它 `.claude/` 配置（output styles 等）不加载。
2. **`permissions.additionalDirectories`（settings.json）**  
   **只授权文件**。不加载 skills / commands / subagents，环境变量也救不回来。适合「这两个仓长期一起改、但不要把对方的人设/技能灌进来」。
3. **`/cd`**  
   换**主**工作目录，会加载新目录的 `CLAUDE.md`，`--resume` 也从新位置找会话。不是附加。

Kivio 已经把 `--add-dir` 接到 `RuntimeContext.extra_allowed_dirs`（`defs/claude.rs`），但运行时只用来挂 **skill 扫描路径**和**附件目录**，没有用户项目。

### Codex：sandbox 根，不是「第二个项目」

来源：[Codex CLI reference](https://developers.openai.com/codex/cli/reference)、[config reference](https://developers.openai.com/codex/config-reference)、[approvals & security](https://developers.openai.com/codex/agent-approvals-security)、[PR #5335](https://github.com/openai/codex/pull/5335)。

- `--add-dir`：额外 **writable** 根，主工作根仍是 cwd / `--cd`。
- `sandbox_workspace_write.writable_roots`：写进 `config.toml` 的持久版。
- 工作区 = 当前目录 + `/tmp` 一类临时目录；`/status` 能看到。
- `.git` / `.codex` / `.agents` 在 writable root 里仍可能是只读保护，所以 `git commit` 还是会要审批。

Kivio 走 `codex app-server`：`thread/start` / `turn/start` 的 `runtimeWorkspaceRoots`（`session/codex_app_server.rs`）。`extra_allowed_dirs_for_agent` 对 **codex 故意返回空**——附件根走另一条 `extra_writable_roots`。用户项目附加目录应并进这条 roots 列表，不要幻想 CLI `--add-dir` 会出现在 argv 里。

### ACP：协议已经为 GUI host 留好了口

来源：[ACP v1 session-setup](https://agentclientprotocol.com/protocol/v1/session-setup)、[v2 session-setup](https://agentclientprotocol.com/protocol/v2/session-setup)。

Kivio 的 cursor / opencode / gemini / kimi / hermes / grok 都走 ACP。`session/new` 今天只发 `cwd` + `mcpServers`（`session/acp.rs::build_session_new_params`），**没有** `additionalDirectories`。`AcpTerminalHost::set_extra_roots` 是空实现，注释写明宿主不做路径围栏。

正确接法：initialize 时看 Agent 有没有 `sessionCapabilities.additionalDirectories`；有才发；每次 new/load/resume **整表重发**。没能力的 CLI 不要发这个字段。

### Cursor / Cline：多根是编辑器工作区，不是聊天属性

来源：[Cursor changelog 2026-04-24](https://cursor.com/changelog/04-24-26)、[Cline multi-root docs](https://docs.cline.bot/features/multiroot-workspace)、[VS Code multi-root](https://code.visualstudio.com/docs/editor/multi-root-workspaces)。

这两家把「有哪些根」放在 **IDE workspace**。聊天继承工作区，不能在一条 Agent 会话里挂、另一条不挂。Cline 把跨仓重构 / 依赖升级写成主用例，同时老实承认：规则、skills、checkpoint 仍偏向 **primary（第一根）**。Cursor 论坛里的静默 cwd 错误，和 Kivio ADR-0001 否决「跨目录 resume」的理由是同一类：模型在错误的根里写文件，比报错更危险。

Kivio 不是多根编辑器。把 `.code-workspace` 做成项目真身，会把「聊天分组」和「编辑器工作区」焊死，也和「集 / 项目互斥」拧巴。可以**借鉴** primary + 具名根 + `@名字:相对路径`，不要把项目改成 VS Code workspace。

### Gemini CLI / OpenCode

- Gemini：[configuration](https://github.com/google-gemini/gemini-cli/blob/ecf8fba1/docs/get-started/configuration.md) `context.includeDirectories` + `--include-directories`，上限 5；会话中 `/directory add|show`。Kivio 驱的是 `gemini --experimental-acp`，附加目录应走 ACP 字段，不是再拼一套 Gemini TUI flag。
- OpenCode：[permissions](https://opencode.ai/docs/permissions/) 的 `external_directory`；TUI `/add-directory`（[PR #14244](https://github.com/anomalyco/opencode/pull/14244)）把规则存进**该会话**。Kivio 同样应走 ACP `additionalDirectories`（若该版本声明了能力），而不是去改用户的 `opencode.json`。

---

## Kivio 现在绑在哪

### 数据：一根目录，一条归属

- `ChatProject.root_path: Option<String>` —— 项目最多一根目录（`chat/types.rs`）。
- `Conversation.project_id` 与 `set_id` 互斥。没有「附加项目 id 列表」。
- 工作目录解析只有一条链：有项目 → 项目根；否则 → 每条对话自己的工作台目录  
  （内置：`nativeTools.workingDirectory/<conv_id>`；外部：`chat-workspaces/<conv_id>`）。  
  见 `resolve_conversation_working_directory`、`resolve_effective_cwd`。
- 设置里的 `native_tools.workspace_roots` 是**遗留字段**。sanitize 只把 `[0]` 迁到 `working_directory`，然后清空。注释写明 runtime 不得再拿它当路径边界。不要复活成多项目。

### 内置运行时：相对走根，绝对不围栏

`native_tools/mod.rs`：相对路径拼项目根；`..` 和绝对路径允许。「这不是沙箱」。系统提示把绝对路径写进 **workbench 段**（跟在用户消息后面），故意不放进静态系统前缀，以免每换一个项目就打爆 prompt cache（`prepare.rs`）。

所以内置侧「跨仓写」技术上已经通：模型只要用绝对路径。缺的是目录清单、Dock、以及不要让 glob 默认扫全世界。

子代理复用**父对话**的项目工作区（`sub_agent.rs` 注释），没有 per-call cwd。Cursor 那种「每个子代理一个仓」是后续增强，不是 v1。

### 外部 CLI：管道在，业务源不在

`run.rs` 已经组 `extra_allowed_dirs`：

- `extra_allowed_dirs_for_agent(def, skill_scan_paths)` —— **codex 除外为空**
- 非斜杠且有降级附件时，再加本会话附件目录

下发：

| 代理 | 今天怎么用 extra dirs | 要接用户项目附加目录时 |
| --- | --- | --- |
| claude | `--add-dir` | 把附加目录 append 进去（已有测试） |
| codex | argv 不用 extra dirs；附件走 `runtimeWorkspaceRoots` | 并进 `extra_writable_roots` |
| ACP 族 + grok | `RuntimeContext` 有字段，`build_session_new_params` **不发送** | 声明了能力再发 `additionalDirectories` |
| pi | 无 flag | 只在提示词里写绝对路径；UI 标明「Pi 不能扩授权」 |
| dsh | 未见授权目录通道 | 先探 SDK；没有就降级成提示词 |

`cwd_hint`（`external_agents/prompt.rs`）只告诉模型一个工作目录。

### Dock / Git：一个 workdir

`dock_resolve_cwd` 的合同是「必须和 agent 真正写文件的目录一致」，因此只有一根。文件树、Git、终端都吃这一个路径。Git 命令按单仓 `git` 子进程来，没有多仓 commit。

### 导入：ADR-0001 锁死主工作目录

导入的 CLI 对话钉在**原来那个 CLI + 原来那个工作目录**。否决过「对能跨目录 resume 的 CLI 放宽」——cwd 错了会静默写到错误的仓。

附加目录**不得**改导入会话的主工作目录，也不得让一条原生会话同时出现在两个项目的导入列表里。列出条件仍然是「原生会话工作目录 == 当前项目根」。

---

## 设计选项

### A. 对话级附加目录（建议做）

对话挂 `additional_directories: [{ path, name? }]`。归属仍是一个项目（或一个集）。主工作目录不变。

- 贴合「这一次升级要动哪几个仓」
- 不拆 集/项目互斥，不拆侧栏
- 不碰 ADR-0001
- 和 Claude `/add-dir`、OpenCode 会话 permission、ACP `additionalDirectories` 同构
- 外部 CLI 本来就要按会话组 argv / session/new

UI：对话里「附加目录」chip；从已有项目里勾，或再选一个文件夹。勾项目只是复用名字和路径，**不**把对话搬进那个项目。

### B. 项目级附加目录

`ChatProject` 增加 `additional_directories`。该项目下每条对话都带上。

- 像 Claude 的 `permissions.additionalDirectories`
- 适合「这五个仓永远一起改」
- 要改项目的定义（CONTEXT.md 现在是单数「该目录」）
- 项目里偶尔只想动一个仓的对话会被噪音干扰

适合作为 **A 的默认值**：项目上记一份常用附加仓，新建对话拷贝到对话字段，之后对话可以改、可以清空。

### C. 一条对话属于多个项目

`project_ids: Vec`。侧栏、导入、集互斥、手动顺序全部要重做。竞品没有把「聊天分组」做成多归属。**否决。**

### D. 子代理每仓一个

父对话编排，每个 `agent` 调用带不同 cwd。Kivio 子代理今天继承父工作区。即便做了，模型仍要先看见所有仓。**v2+，不能替代 A。**

### E. 只绑公共父目录

零开发。仓分散在不同盘符（`E:\base` vs `D:\biz-a`）就失败，还会把无关文件夹暴露进去。可当用户手册里的权宜之计，不当产品。

---

## 建议怎么做

### 词汇

在 `CONTEXT.md` 加 **附加目录**：一条对话在项目主目录之外额外授权给 agent 的文件夹。不改变对话属于哪个项目，也不改变导入用的工作目录。  
_Avoid_: workspace、多根工作区、cwd、第二个项目。

项目定义保持「一根主目录」。若做选项 B，再写成「主目录 + 可选的附加目录默认列表」。

### 数据

- 对话字段：`additional_directories: Vec<{ path: String, name?: String }>`（绝对路径，canonicalize，去重，不能等于主目录）。
- `ConversationMetadataMutation` 增加显式变体（repository 合同：`Option` 只表示 set/clear）。
- 不要用遗留 `workspace_roots`。
- 协议：这是会话元数据，走现有 conversation 事件，不要新开一条 realtime 流。

### 提示词

附加目录清单放进 **workbench 段**（已从静态前缀剥离），列出：

- 默认工作台 = 主目录（相对路径落这里）
- 每个附加目录的 **名字 + 绝对路径**
- 动附加仓时必须用绝对路径；不要用 `../其他仓`

不要把这份清单放进静态系统前缀。

内置 glob/grep：默认仍只扫主目录。Cursor 的多根 grep 会扫全部根，大仓直接超时。要搜附加仓就显式给绝对 `path`。

### 内置工具

v1 不必给 `read`/`edit` 加 `root` 参数——绝对路径已经能用。把清单写进提示词即可。  
v2 可加可选 `root`（项目名 / 附加名），减少模型拼 Windows 路径。

`run_command` 的 `cwd` 已经能指向附加目录（绝对路径）。提示词写清：跨仓命令用 `cwd`，不要 `cd path &&`。

### 外部 CLI 下发

在 `run.rs` 把对话附加目录 merge 进现有 extra-dirs 管道，按代理分流（上表）。ACP 必须 capability-gate。能力缺失时 UI 警告，不要假装沙箱已经放开。

Claude：Kivio 用的是 `--add-dir`，会带上对方 `.claude/skills`。若不想跨仓技能泄漏，需要产品开关或改走「只授权」通道——Claude 的只授权通道是 settings `additionalDirectories`，Kivio 现在没写那文件。v1 先接受「`--add-dir` 会加载对方 skills」，文档写清楚。

### Dock / Git

v1：**根切换器**（下拉：主目录 + 每个附加目录）。树 / Git / 终端跟当前选中根走，合同仍然是「看到的目录 = 选中根」，不是「看到的目录 = agent 可能写到的所有地方」。  
v2：VS Code 式多根树；Git 按根分 tab。永远不要做一个跨仓原子 commit。

Cline 的教训：checkpoint / 规则默认只认 primary。Kivio 的 `.kivio/agents`、项目 skills 继续只从**主项目根**加载，除非以后单独做「从附加目录加载 .kivio」（对标 Claude 的 CLAUDE.md 开关，默认关）。

### 导入与续聊

主工作目录不变。附加目录是 Kivio 侧授权，不是原生会话身份。导入列表仍按项目根过滤。

### UI 范围（v1）

- 对话标题栏或输入条：附加目录 chips + 添加（已有项目 / 选文件夹）
- 从项目添加：多选其它已绑定文件夹的项目
- 外部 CLI 不支持时：chip 旁警告，不阻止内置运行时使用
- 不改侧栏信息架构

---

## 明确不要做

1. 一条对话多个 `project_id`。
2. 复活 `native_tools.workspace_roots` 当运行时边界。
3. 为了附加目录去改导入会话的工作目录（ADR-0001）。
4. 把 glob/grep 默认范围扩到全部根。
5. 假定所有外部 CLI 都能扩授权（Pi 不能；ACP 要看 capability）。
6. v1 做跨仓 Git 一键提交或多仓 checkpoint。
7. 把附加目录写进静态系统前缀（prompt cache）。

---

## 建议分期

1. **对话字段 + 内置提示词 + 输入条 chips。** 内置运行时立刻能靠绝对路径跨仓。人工验收：base + 两个业务仓一次升级。
2. **下发外部 CLI：** Claude `--add-dir`、Codex `runtimeWorkspaceRoots`、ACP `additionalDirectories`（有能力才发）。Pi/不支持：警告。
3. **Dock 根切换器**（树 + Git + 终端跟选中根）。
4. **项目级默认附加目录**（新建对话拷贝；不强制旧对话）。
5. **可选：** 子代理 `cwd`/`root` 覆盖；文件工具 `root` 参数；`.kivio` 是否从附加目录加载（默认否）。

---

## 词汇表缺口

**附加目录** 应写入 `CONTEXT.md`。若把默认列表放到项目上，项目那条「该目录决定读写位置」要改成「主目录决定默认读写位置；附加目录另计」。

这与 ADR-0001 **不冲突**，前提是附加目录绝不替换导入用的工作目录。

---

## Sources

- Claude Code CLI: https://code.claude.com/docs/en/cli-reference
- Claude Code permissions（additional directories / `/cd`）: https://code.claude.com/docs/en/permissions
- Claude Code slash commands / skills 与 `--add-dir` 的关系: https://code.claude.com/docs/en/slash-commands · https://code.claude.com/docs/en/skills
- Codex CLI reference: https://developers.openai.com/codex/cli/reference
- Codex config: https://developers.openai.com/codex/config-reference
- Codex sandbox: https://developers.openai.com/codex/agent-approvals-security
- Codex `--add-dir` PR: https://github.com/openai/codex/pull/5335
- Gemini CLI configuration: https://github.com/google-gemini/gemini-cli/blob/ecf8fba1/docs/get-started/configuration.md
- Gemini CLI `/directory`: https://github.com/google-gemini/gemini-cli/blob/HEAD/docs/reference/commands.md
- OpenCode permissions: https://opencode.ai/docs/permissions/
- OpenCode `/add-directory` PR: https://github.com/anomalyco/opencode/pull/14244
- ACP v1 session setup: https://agentclientprotocol.com/protocol/v1/session-setup
- ACP v2 session setup: https://agentclientprotocol.com/protocol/v2/session-setup
- Cursor changelog (multi-root in Agents Window): https://cursor.com/changelog/04-24-26
- Cline multi-root: https://docs.cline.bot/features/multiroot-workspace
- VS Code multi-root: https://code.visualstudio.com/docs/editor/multi-root-workspaces
- Aider FAQ（一次一个 git 仓）: https://github.com/Aider-AI/aider/blob/bdb4d9ff/aider/website/docs/faq.md
- Kivio ADR-0001: `docs/adr/0001-imported-cli-conversations-stay-on-their-cli.md`
- Kivio 现状：`chat/types.rs` `ChatProject` / `Conversation`；`chat/storage.rs::resolve_conversation_working_directory`；`native_tools/mod.rs`；`external_agents/workspace.rs`；`external_agents/run.rs`；`external_agents/defs/claude.rs`；`external_agents/session/codex_app_server.rs`；`external_agents/session/acp.rs`；`dock/mod.rs`；`chat/agent/prepare.rs`；`settings.rs` 里 `workspace_roots` 遗留迁移
