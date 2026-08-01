# Kivio

Kivio 是一个桌面 AI 助手：屏幕级工具（划词翻译、截图 OCR、Lens 视觉问答）+ 一个完整的 agent 聊天应用。聊天既可以跑 Kivio 自己的 agent 循环，也可以由用户本机安装的外部编码 CLI 来承担。

## Language

### 运行时与会话

**外部 CLI 代理**：
用户本机已安装、被 Kivio 驱动来充当某条对话后端的编码 CLI（claude、codex、grok、kimi、gemini、opencode、hermes、pi、cursor）。
_Avoid_: 第三方 agent、外部模型

**内置运行时**：
Kivio 自己的 agent 循环。与外部 CLI 代理是同一条对话上二选一的关系。
_Avoid_: 本地 agent、原生循环

**原生会话**：
外部 CLI 自己维护的那份对话历史，存在该 CLI 自己的目录或数据库里，由该 CLI 自己的会话 id 标识。Kivio 不持有它的真身。
_Avoid_: CLI 会话、上游会话

**会话绑定**：
一条 Kivio 对话与一条原生会话之间的对应关系。绑定存在时，这条对话的续聊由对应的 CLI 用原生会话 id 承担。
_Avoid_: 会话映射、session 关联

**工作目录**：
一条原生会话被创建时所在的目录。它同时决定两件事：这条会话属于哪个项目，以及某些 CLI 能不能续上它。
_Avoid_: cwd、工作区、workspace

### 导入

**导入**：
把一条已存在的原生会话变成一条 Kivio 对话：历史被解析出来用于显示，同时建立会话绑定。导入不复制历史的所有权——真身仍在 CLI 那边。
_Avoid_: 迁移、同步、导入历史

**续聊**：
在一条已导入的对话上继续对话。始终由原来那个 CLI 承担，不能改由内置运行时或另一个 CLI 接手。
_Avoid_: 恢复、resume、接管
