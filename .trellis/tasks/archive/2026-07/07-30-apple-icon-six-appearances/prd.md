# 补齐 Apple 六种图标呈现

## Goal

将已有四类 Apple 图标外观补齐为规范所列的六种实际呈现。

## Requirements

- 保留现有 Default 与 Dark 成品。
- 将 Clear 与 Tinted 分别提供 Light 和 Dark 版本。
- 所有成品均为 1024×1024 PNG，并保持一致的 Kivio 双曲线标志。
- 图像内容仅通过内置“图片生成”工具制作，不使用 Apple 官方图标工具。

## Acceptance Criteria

- [x] 目录中具有 Default、Dark、Clear Light、Clear Dark、Tinted Light、Tinted Dark 六个明确命名的成品。
- [x] 六个成品均为 1024×1024 PNG。
- [x] 浅色与深色变体在对应背景中清晰可辨，且标志轮廓一致。

## Notes

- Keep `prd.md` focused on requirements, constraints, and acceptance criteria.
- Lightweight tasks can remain PRD-only.
- For complex tasks, add `design.md` for technical design and `implement.md` for execution planning before `task.py start`.
