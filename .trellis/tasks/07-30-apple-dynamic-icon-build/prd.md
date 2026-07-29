# 接入 Apple 动态图标构建

## Goal

让 macOS 构建产物使用 Apple 动态 App Icon 资源，并能随系统外观呈现 Default、Dark、Clear 和 Tinted 效果。

## Requirements

- 基于现有 Kivio 标志制作可由 Apple 图标渲染器识别的分层 `.icon` 资源。
- 通过 Xcode 26.6 自带的命令行工具编译，不依赖 Icon Composer 图形界面。
- 接入现有 Tauri 2.10 构建流程，正常 `npm run build` 时自动生成并打包动态资源。
- 保留现有 `icon.icns` 作为旧系统和非动态图标路径的兼容回退。
- Windows 构建和开发流程不得依赖 macOS/Xcode 工具。
- 构建产物必须包含 Apple 编译后的图标资源及正确的 `Info.plist` 声明。

## Acceptance Criteria

- [ ] 仓库包含有效的 Kivio `.icon` 包，至少定义 Default、Dark、Mono 三种渲染语义。
- [ ] macOS 构建前命令能使用本机 Xcode 命令行工具生成 `Assets.car`。
- [ ] Tauri macOS `.app` 包含 `Assets.car`，同时保留 `icon.icns` 回退。
- [ ] `.app/Contents/Info.plist` 指向动态 App Icon 资源。
- [ ] 可用 `ictool` 导出并检查 Default、Dark、Clear Light/Dark、Tinted Light/Dark 六种 rendition。
- [ ] 非 macOS 环境跳过动态图标编译，不影响 Windows 发布。

## Notes

- Keep `prd.md` focused on requirements, constraints, and acceptance criteria.
- Lightweight tasks can remain PRD-only.
- For complex tasks, add `design.md` for technical design and `implement.md` for execution planning before `task.py start`.
