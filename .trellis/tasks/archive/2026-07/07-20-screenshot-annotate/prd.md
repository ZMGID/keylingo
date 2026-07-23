# 独立截图标注模式：箭头 / 矩形 / 马赛克 + 复制 / 保存

## 背景（现状盘点）

复用度非常高，大部分基础设施已经存在：

- **截图选区**：`Lens.tsx` 的 `select` 态已有完整的窗口 hover 高亮 + 拖拽区域选择，走 `lens_capture_window` / `lens_capture_region`（macOS SCK / Windows xcap），冻结帧、多显示器坐标都已处理。
- **箭头标注**：`stage==='ready'` 已有 `drawMode` — `src/lens/ArrowSvg.tsx`（SVG 实时渲染）+ `src/lens/annotation.ts::composeAnnotatedImage`（OffscreenCanvas 合成到物理像素 PNG）+ Cmd+Z 撤销 + Esc 退出。
- **模式路由**：`#lens?mode=<m>` hash query → `readModeFromHash()`，Rust 侧 `lens_request_internal(app, mode)` + 每模式一条热键（`shortcuts.rs`），全链路成熟（现有 chat/translate/translateText/replace 四个模式）。
- **剪贴板**：`arboard` 已是直接依赖（`commands.rs` 用于文本）；图片需要新加一个 `set_image` 命令。
- **保存对话框**：`tauri-plugin-dialog` 已装。

缺的只有：新 `screenshot` 模式的路由/热键、矩形和马赛克两种新标注类型、标注工具栏 UI、复制到剪贴板 / 保存文件两个出口。

## 需求

新增一个**独立截图模式**（类 Snipaste 轻量版）：

1. **入口**：新热键（默认建议 `CommandOrControl+Shift+S`，可在设置改）触发 `#lens?mode=screenshot`。复用现有 select 态选窗口/拖区域。
2. **截图后**进入标注态：不显示 AI 输入框，显示一个标注工具栏，工具：
   - **箭头**（复用现有 Arrow）
   - **矩形**（空心描边，同箭头红色）
   - **马赛克**（拖出一个矩形区域，对该区域像素化处理；SVG 预览时可先用半透明占位/低清预览，合成时真正像素化）
3. **通用交互**：
   - Cmd+Z / Ctrl+Z 撤销最后一个标注（跨类型统一 undo 栈）
   - Esc 关闭整个模式（丢弃）
4. **出口**（工具栏按钮）：
   - **复制**：合成标注后的 PNG → 写入系统剪贴板 → 关闭 Lens
   - **保存**：合成 PNG → 系统保存对话框（tauri-plugin-dialog）→ 写文件 → 关闭 Lens
5. **设置页**：截图热键可配置（沿用现有 hotkey 设置模式，`normalize_hotkey` + 事务回滚）。

## 非目标（本期不做）

- 文字标注、荧光笔、序号气泡、椭圆等更多工具
- 标注对象的选中/移动/删除（只有 undo）
- 贴图（pin 到屏幕）
- 截图历史管理
- 颜色/线宽自定义

## 验收标准

- [ ] 新热键触发后可选窗口或拖区域截图，截图后出现标注工具栏（无 AI 输入框）
- [ ] 箭头、矩形、马赛克三种工具均可绘制，SVG 实时预览
- [ ] 马赛克在最终导出 PNG 中是真实像素化（物理像素分辨率下），不是预览占位
- [ ] Cmd+Z 撤销任意类型的最后一个标注；Esc 关闭
- [ ] 复制按钮：导出 PNG 进系统剪贴板，可粘贴到其他应用；随后 Lens 关闭
- [ ] 保存按钮：弹出系统保存对话框，PNG 落盘正确；随后 Lens 关闭
- [ ] 热键在设置页可改，冲突时报错回滚（沿用现有机制）
- [ ] 现有 chat/translate/translateText/replace 四个模式行为不变（`npm run lint` + `typecheck` + `npm test` + `cargo test` 通过）

## 技术要点（供 design 参考）

- `Mode` 增加 `'screenshot'`；`Arrow` 类型泛化为 `Annotation = { kind: 'arrow'|'rect'|'mosaic', x1,y1,x2,y2 }`（同一坐标系，全是两点拖拽，undo 栈天然统一）。
- `composeAnnotatedImage` 扩展：rect 描边；mosaic 用 canvas 缩小再放大（`imageSmoothingEnabled=false`）实现像素化。
- Rust 新命令：`copy_image_to_clipboard(base64)`（arboard `set_image`）；保存走前端 dialog 插件 + 新 `save_png(base64, path)` 或复用现有文件写入。
- `screenshot` 模式 `keepFullscreen` 恒 true（标注必须在原位画）。
