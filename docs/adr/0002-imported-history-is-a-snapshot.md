# 导入的历史是一次性快照，不与 CLI 存储保持同步

导入时把原生会话解析一次、写进 Kivio 的 conversation，之后就是一条普通的外部 CLI 对话；不在每次打开时重读 CLI 的存储。因为重读会把在 Kivio 里聊出来的内容**降级**——Kivio 跑外部 CLI 时记的是完整的工具卡片和 reasoning 段，而解析 CLI 存储只能还原出解析器支持的那部分。

## Considered Options

**投影：每次打开都从 CLI 存储重读。** 否决：除了上面的降级问题，加载路径还得分叉（有会话绑定的走解析器、没有的走 conversation JSON），而续聊产生的新消息又得拼回投影里，比快照复杂。

## Consequences

用户在 Kivio 之外继续用该 CLI 聊同一条会话时，Kivio 里的历史会残缺。而在 Kivio 里续聊时 CLI 会 resume 它自己那份**完整**历史——**模型知道的比用户在界面上看到的多**。

这是静默的不一致，必须可见但不必消除：导入时记下原生存储的指纹（最后修改时间 + 消息条数），打开对话时比对，不一致就在顶部挂一条"这条会话在 CLI 那边有新内容，此处显示的历史不完整"。**只提示，不同步**——同步的成本远高于收益。

## 快照不参与模型输入，所以可以截断

续聊时模型读的是 CLI 那边那份完整历史（由 `--resume` / ACP 自己加载），Kivio 这份快照**只用于显示**。因此解析时每个 `tool_result` 正文只留前 2KB 并标注已截断，不会影响任何模型行为。

这条不是可有可无的优化：实测本机一条 claude 会话是 **10MB / 3522 行**，其中 719 个 `tool_result` 装着文件全文和命令输出，占了绝大部分体积。原样内嵌会让 Kivio 的 conversation JSON 一起膨胀、加载变慢——与 `docs/image-generation.md` 里"大图内嵌撑爆对话文件"是同一个毛病。

解析范围同理收窄到四类块：`text` / `tool_use` / `tool_result` / `thinking`，外加图片转附件。子 agent 分支（claude 的 `isSidechain`）、hook 注入、`file-history-snapshot` 等 CLI 内部账务一律丢弃——Kivio 的子 agent 卡片是运行时事件驱动的，没有"从历史还原嵌套卡片"的结构，硬塞要新增渲染路径。

**适用范围**：截断与解析范围只针对需要读文件的那几个（claude / grok / codex，见 [ADR-0003](./0003-acp-agents-import-via-protocol.md)）。ACP 代理的历史由 `session/load` 重放、走实时渲染路径落盘，体积行为与平时聊天一致，不另做截断。两者不一致是有意的：与实时路径保持一致比导入结果整齐更重要。
