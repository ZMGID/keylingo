# 导入的 CLI 对话钉死在原 CLI 和原工作目录上

从外部 CLI 导入的对话只能由**原来那个 CLI**、在**原来那个工作目录**下续聊，不能改用 Kivio 内置运行时或别的 CLI，也不能在别的目录下续。因为 Kivio 只把导入的历史当显示用、不重建 `model_messages`，续聊靠的是该 CLI 自己的 `--resume <原生会话 id>`——历史的真身一直在 CLI 那边，一个字节都不用翻译。

## Considered Options

**把历史翻译成 Kivio 的 `model_messages`，之后可用任意模型续聊。** 否决：要给每个 CLI 写工具名反向映射（claude 的 `Read`/`Edit`/`Bash` → `read_file`/`edit_file`/`run_command`），给每条 tool_result 编能过校验的 id，而 hook 注入、sidechain 子 agent、CLI 私有工具映射不上只能丢。丢完之后模型看到的历史和用户在界面上看到的不是同一份。

**对能跨目录 resume 的 CLI 放宽工作目录约束。** 否决：本机实测 codex / grok / opencode 确实能跨目录续上，但 Kivio 续聊时的 cwd 取自 `resolve_effective_cwd`（项目根）。会话续上了而工作目录不对，模型就会在错误的目录里读写文件——静默、无报错，比"找不到会话"危险得多。

## Consequences

- 导入是**项目内的动作**：只列出工作目录等于当前项目根的那些原生会话。
- 支持 6 个 CLI：claude、codex、grok、kimi、gemini、opencode。**pi** 没有本地历史（会话 id 是 Kivio 自己生成的），**hermes** 的 `sessions` 表不记工作目录、无法判断归属，**cursor** 的 `cursor-agent` 未验证——三个都不支持。
- 原工作目录不存在时（换机器、目录已删）不能导入，需要在导入前就明说，而不是让用户导进来之后每轮踩一次上下文重置。

## 本机实测依据（2026-08-01）

锁工作目录，必须在原目录才能续：**claude**（异目录报 `No conversation found with session ID`，回原目录同一 id 正常续上）、**kimi**（报 `was created under a different directory` 并打印 `cd` 命令）、**gemini**（`--list-sessions` 换目录即为空）。

不锁，跨目录也能续：**codex**、**grok**（会打印 `originally in <原目录>`）、**opencode**。
