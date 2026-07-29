# 适配 Apple 四种图标外观

## Goal

基于现有 Kivio 图标，按照 Apple App Icons 人机界面指南制作四类外观版本。

## Requirements

- 以 `src-tauri/icons/source-rounded.png` 为唯一视觉参考，保持 Kivio 双曲线标志的轮廓、比例和品牌识别度。
- 仅使用内置“图片生成”工具完成图像创作，不使用 Icon Composer、Xcode 资产工具或其他 Apple 官方制图工具。
- 输出默认、深色、透明、色调四种外观，每种均为 1024×1024 PNG。
- 各版本只调整材质、背景、明暗与色彩表现，不增删核心标志元素，不加入文字、水印或额外图形。
- 成品保存到项目内的新目录，不能覆盖现有图标。

## Acceptance Criteria

- [x] 四种外观均已生成并保存为独立 PNG。
- [x] 每张图片尺寸为 1024×1024。
- [x] 四种外观在缩略尺寸下仍能辨认出一致的 Kivio 标志。
- [x] 默认与深色版本保持品牌蓝色；透明与色调版本更克制，并适合相应系统外观。
- [x] 最终交付列出保存路径、生成方式和使用的提示词。

## Notes

- Keep `prd.md` focused on requirements, constraints, and acceptance criteria.
- Lightweight tasks can remain PRD-only.
- For complex tasks, add `design.md` for technical design and `implement.md` for execution planning before `task.py start`.
