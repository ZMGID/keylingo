# Design — 从本地 CLI 导入对话

把用户在本机 CLI（claude / codex / grok / kimi / gemini / opencode）里已经聊过的会话，导入成 Kivio 的对话，并且**能接着聊**——由原来那个 CLI、在原来那个工作目录下续。

决策依据见 [ADR-0001](../../../docs/adr/0001-imported-cli-conversations-stay-on-their-cli.md)（钉死原 CLI + 原工作目录）、[ADR-0002](../../../docs/adr/0002-imported-history-is-a-snapshot.md)（快照，不同步）、[ADR-0003](../../../docs/adr/0003-acp-agents-import-via-protocol.md)（ACP 组走协议不解析存储）。术语见根目录 `CONTEXT.md`。

## 支持范围

| CLI | 枚举方式 | 历史来源 | 会话绑定落在哪 |
| --- | --- | --- | --- |
| claude | 读 `~/.claude/projects/<编码 cwd>/*.jsonl` | 解析 jsonl | `external-agent-sessions/<conv>.json` |
| grok | 读 `~/.grok/sessions/<百分号编码 cwd>/<uuid>/` | 解析该目录 | `live-<conv>.json` |
| codex | CodexAppServer 若有等价方法则走协议，否则读 `~/.codex/sessions/**/rollout-*.jsonl` | 同左 | `live-<conv>.json` |
| opencode | ACP `session/list`（带 cwd） | ACP `session/load` 重放 | `live-<conv>.json` |
| kimi | ACP `session/list`（带 cwd） | **无**——见下方说明 | `live-<conv>.json` |

**kimi 只绑定、不带历史快照。** 它的 `session/load` 能绑定但不重放（ADR-0003 能力矩阵），而本地 `agents/main/wire.jsonl` 里 `context.append_message` 只落用户消息，assistant 正文散在 129 条 `context.append_loop_event` 里、没有稳定可读的形态。所以导入 kimi 会话时消息区为空，附一条说明「kimi 不提供可读取的历史；续聊时 CLI 那边的上下文是完整的」。这不影响正确性——按 ADR-0002 快照本来就只用于显示，不参与模型输入。将来 kimi 的 wire 格式稳定了再补解析器。

**不支持**：

- `pi` —— 无本地历史，session id 由 Kivio 生成。
- `hermes` —— `sessions` 表不记工作目录，无法判断项目归属。
- `gemini` —— Kivio 走 ACP 驱动它，而本机 gemini CLI 的 ACP `loadSession=false` 且 `session/list` 返回 `Method not found`，**导入后无法续聊**，与 ADR-0001 冲突。将来 gemini 实现了 ACP 会话方法可原样加回。
- `cursor` —— `cursor-agent` 本机未安装，能力未验证。

绑定文件写哪一个取决于 `RuntimeAgentDef.resumes_session_via_cli`：`true` 的（claude）写 `external-agent-sessions/<conv>.json`（`{conversationId, agentId, sessionId, stablePromptHash, model}`）；`false` 的写 `live-<conv>.json`（`LiveSessionHandle { agent_id, protocol, native_id, cwd }`）。

## 入口

侧边栏**已创建的项目**的菜单里加一项「从 CLI 导入对话」。没绑项目的对话没有这个入口——工作目录必须对齐是硬约束，没有项目根就没有可比对的目标。

弹出列表按 CLI 分组，**只列出工作目录等于该项目根的会话**，显示标题 / 时间 / 消息数，可多选批量导入。

- ACP 组：`session/list` 的 `cwd` 参数直接在源头过滤（**必须传**，否则 agent 返回全局最近会话，分页 limit 会在够到本目录之前截断）。
- claude / gemini：目录名是有损编码或哈希，反解不了；拿项目根**正向算**去定位目录。
- grok：百分号编码无损，可反解。
- codex / kimi / opencode：各自有明文 cwd 字段（`session_meta.payload.cwd` / `session_index.jsonl` 的 `workDir` / `session.directory` 列）。

cwd 比对一律 **realpath-aware**（先解符号链接、junction，再比）。

已导入过的会话在列表里标「已导入」且不可勾选，点击跳转到已有的那条 Kivio 对话——会话绑定是 1:1 的，两条 Kivio 对话绑同一条原生会话会让两边的快照都残缺，而模型看到的是混在一起的完整版。

## 导入动作

1. 解析/重放出历史，写成 Kivio 的 `ChatMessage`：`text` → `content`，`thinking` → `reasoning`，`tool_use` + 对应 `tool_result` → `tool_calls: Vec<ToolCallRecord>`，图片 → `attachments`。子 agent 分支、hook 注入、CLI 内部账务条目丢弃。
2. 读文件的那几个：每个 `tool_result` 正文只留前 2KB 并标注已截断（快照不参与模型输入，截断零风险；实测一条 claude 会话 10MB / 3522 行，719 个 tool_result 占了绝大部分体积）。
3. 建 `Conversation`：`project_id` = 当前项目，`agent_runtime = { kind: External, external_agent_id, external_model }`（`external_model` 从原生会话读出），`provider_id` / `model` 照抄现有「新建外部对话」路径的填法。
4. 写会话绑定文件（上表最后一列）。
5. 记下原生存储的指纹（最后修改时间 + 消息条数），供后续过期检测。

## 过期提示

打开一条已导入的对话时，比对当前指纹与导入时记录的指纹。不一致就在顶部挂一条提示：「这条会话在 CLI 那边有新内容，此处显示的历史不完整」。**只提示，不同步。**

理由：用户在 Kivio 之外继续用该 CLI 聊过之后，Kivio 里的快照就残缺了；而在 Kivio 里续聊时 CLI 会 resume 它自己那份完整历史——模型知道的比用户在界面上看到的多。这个不一致不值得花代价消除，但不该是静默的。

## 已验证 / 未验证

**已实测（2026-08-01，本机）**

- claude：异目录 `--resume` 报 `No conversation found with session ID`；回到原目录同一 id 正常续上。
- kimi：异目录报 `was created under a different directory` 并打印 `cd` 命令。
- gemini：`--list-sessions` 在异目录为空，在原目录列出 2 条。
- codex / grok / opencode：跨目录能按 id 找到会话（grok 还会打印 `originally in <原目录>`）。
- kimi 与 opencode 的 ACP 会话和 TUI 会话**共用同一个存储**（kimi `session_index.jsonl` 52 条里 42 条由 Kivio 产生；opencode `session` 表里有 Kivio 目录下的记录）。
- **ACP 探针**（临时脚本，起 `opencode acp` / `kimi acp` / `gemini --experimental-acp` 走一遍 `initialize` → `session/list` → `session/load`）：
  - opencode `loadSession=true`，`session/list {cwd}` 返回 19 条 TUI 会话，`session/load` 成功并重放 15 条 `session/update`。
  - kimi `loadSession=true`，`session/list {cwd}` 返回 2 条，`session/load` 成功但**只推一条 `available_commands_update`，不重放历史**。
  - gemini `loadSession=false`，`session/list` 报 `Method not found (-32601)`。

**未验证**

`cursor` 的 `cursor-agent` 本机未安装，全程未验证。codex 是否有等价于 `session/list` 的 app-server 方法未查，暂按读文件设计。

## 参考

paseo（AGPLv3，`E:\ZM database\cankao\paseo`）实现过同类功能。本设计只取协议层事实——ACP `session/list` 的 cwd 参数与分页截断陷阱、`session/load` 的历史重放语义、claude 项目目录的编码规则与 200 字符截断、cwd 比对需 realpath-aware、按 `(provider, sessionId)` 过滤已导入。**未参考其代码结构、未复制任何实现。**
