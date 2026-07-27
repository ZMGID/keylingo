# Windows Mica 激活后移除侧边栏透明色层

## Goal

增强 Windows 聊天窗口的 Mica 材质表现：仅在系统确认 Mica 已成功启用后，移除侧边栏
当前叠加的半透明主题色层，让系统合成器的材质直接可见。

## Requirements

- 仅改变 Windows 聊天窗口；macOS 的 Menu 材质及其侧边栏可读性叠层保持不变。
- Windows Mica 成功启用时，侧边栏背景必须完全透明，不再叠加亮色或暗色主题 RGB。
- Mica 未启用、被尺寸阈值禁用、用户关闭透明侧边栏设置或应用失败时，侧边栏继续使用
  现有不透明主题色回退，保证内容可读。
- 不引入 CSS blur 或 `backdrop-filter`，材质效果仍由 Windows 合成器负责。
- 现有主题色、边框、侧边栏折叠行为不变。

## Acceptance Criteria

- [x] Windows 上 `Effect.Mica` 成功应用后，宿主的原生效果状态使侧边栏背景变为
  `transparent`。
- [x] Windows Mica 未激活或应用失败时，侧边栏背景仍为
  `rgb(var(--chat-sidebar-surface))`。
- [x] macOS 原生 Menu 效果激活时，亮色与暗色侧边栏仍分别保留现有 `0.72` / `0.66`
  透明色层。
- [x] 自动化测试覆盖 Windows 与 macOS 原生效果下不同的侧边栏背景策略。
- [x] 相关 Vitest、TypeScript 类型检查和 ESLint 检查通过。

## Notes

- Keep `prd.md` focused on requirements, constraints, and acceptance criteria.
- Lightweight tasks can remain PRD-only.
- For complex tasks, add `design.md` for technical design and `implement.md` for execution planning before `task.py start`.
