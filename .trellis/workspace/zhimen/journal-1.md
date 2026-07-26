# Journal - zhimen (Part 1)

> AI development session journal
> Started: 2026-07-12

---



## Session 1: Export conversations as Markdown

**Date**: 2026-07-13
**Task**: Export conversations as Markdown
**Branch**: `feat/external-agent-model-refresh`

### Summary

Added localized per-conversation Markdown export from the sidebar context menu, native save-dialog flow, privacy-safe backend rendering, filename sanitization, tests, and an executable export contract spec.

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `69609c6` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 2: Chat message navigator

**Date**: 2026-07-13
**Task**: Chat message navigator
**Branch**: `main`

### Summary

Implemented and refined a semantic chat message rail with turn-based navigation, visible-turn highlighting, centered compact layout, content-safe spacing, hover previews, linked wheel navigation, compaction nodes, and pointer-proximity fisheye feedback; validated with focused tests and user runtime screenshots.

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `80b3dcb` | (see git log) |
| `ac2ea0b` | (see git log) |
| `b3d1c2f` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 3: 替换翻译照片级擦除实施与质量复核

**Date**: 2026-07-13
**Task**: 替换翻译照片级擦除实施与质量复核
**Branch**: `main`

### Summary

完成离线包下载器、MI-GAN 擦除、区域布局、稳定 ID 翻译、自然尺寸 Canvas 排版和设置页进度；专项测试、类型检查、修改文件 ESLint、UI build 及 512×512 MI-GAN 热路径 194ms E2E 通过。任务保持 in_progress，待线程/内存基准、Windows 实机与原始截图视觉验收。

### Main Changes

- 集中离线模型 manifest、校验下载、续传重试和设置页进度。
- 接入 MI-GAN 擦除、OCR 多边形 mask、表格/段落区域布局与稳定 ID 翻译契约。
- 重写自然尺寸 Canvas 排版，并移除 `fillText` 的 `maxWidth` 横向压缩路径。

### Git Commits

(No commits - implementation remains in progress)

### Testing

- [OK] Rust 格式与替换翻译专项测试
- [OK] TypeScript、修改文件 ESLint、前端专项测试与 UI build
- [OK] 512×512 MI-GAN E2E：冷路径 513.8 ms，热路径 194.0 ms，mask 外逐像素不变

### Status

[WIP] **In Progress**

### Next Steps

- 补 1/2/8 线程与 ORT arena、同进程峰值内存基准。
- 完成 Windows 实机验证。
- 使用未经过 QQ 擦除处理的原始截图做最终视觉验收。


## Session 4: MCP 远程服务器 OAuth 授权入口 + lens 文本历史修复

**Date**: 2026-07-16
**Task**: MCP 远程服务器 OAuth 授权入口 + lens 文本历史修复
**Branch**: `main`

### Summary

修复 lens 纯文本会话(无截图)不进历史(拆分 history key 与后端 image 句柄)。为 MCP 服务器页的 streamable_http 服务器加 OAuth 授权按钮,复用 connector_oauth_connect(PKCE+DCR),把 auth+Authorization 拼回现有条目;client.rs 401+Bearer WWW-Authenticate 加 OAUTH_REQUIRED 前缀驱动设置页提示;connectors catalog id 未命中时回退 url(修 Linear/Sentry/Atlassian)。测试中发现并修复协议版本白名单过旧,接受 2025-11-25。TinyFish MCP 已实测可连。

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `0bcdb0e` | (see git log) |
| `49c0962` | (see git log) |
| `bc78956` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 5: 重做内置专家套件（去 AI 味 + 常用模型）+ 若干 UI 修复

**Date**: 2026-07-16
**Task**: 重做内置专家套件（去 AI 味 + 常用模型）+ 若干 UI 修复
**Branch**: `main`

### Summary

把 4 个占位内置专家重做为 7 个专业人设(写作/编程/前端/研究/数据/翻译/文档),每个 prompt 口语化并统一拼接去 AI 味文风块;非破坏 v2 迁移(按 id upsert 保留用户自建);内置默认不加入常用,专家中心 tab 改为 常用(首位)/广场/我的,对话栏只列常用;去掉启用/停用概念;修复重复内置徽章。另修请求调试左列吸顶去空缺。

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `13473a5` | (see git log) |
| `8c4913d` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 6: macOS overlay teardown and focus fixes

**Date**: 2026-07-16
**Task**: macOS overlay teardown and focus fixes
**Branch**: `main`

### Summary

Fixed input translator Esc/toggle teardown by restoring the TaoWindow class before destroying the WebView/NSPanel; prevented shortcut overlays from raising the Chat window; avoided redundant frontmost-app activation that caused Lens window flashing.

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `f297773` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 7: Harden overlay teardown and legacy assistants

**Date**: 2026-07-16
**Task**: Harden overlay teardown and legacy assistants
**Branch**: `main`

### Summary

Made macOS overlay class restoration and destruction execute atomically on the main thread with a safe hide fallback, and treated the legacy assistant enabled flag as compatibility-only so previously disabled assistants remain usable until archived. Added regression coverage and verified build, lint, typecheck, and targeted storage tests.

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `5adeae9` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 8: Fix chat scroll stutter

**Date**: 2026-07-17
**Task**: Fix chat scroll stutter
**Branch**: `main`

### Summary

Profiled long-conversation scrolling, identified virtualized historical message entrance animations as the dominant refresh/sticky effect, limited motion to the live streaming preview, added regression tests and frontend guidance, removed all temporary probes, and verified browser behavior plus the full Node 24 quality gate.

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `b3929ab` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 9: Optional model temperature and metadata refresh

**Date**: 2026-07-17
**Task**: Optional model temperature and metadata refresh
**Branch**: `main`

### Summary

Made temperature model-scoped and omitted by default across all request paths, added editable model overrides and tests, and refreshed the model database with Kimi K3, Kimi K2.7 Code variants, and Claude Mythos 5.

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `54d17e1` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 10: Fix external agent integration issues

**Date**: 2026-07-17
**Task**: Fix external agent integration issues
**Branch**: `codex/fix-external-agent-issues`

### Summary

Fixed Pi RPC shutdown EPIPE, added project-scoped OpenCode native model discovery and caches, and documented the cross-layer contracts.

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `13055fa` | (see git log) |
| `6edb78c` | (see git log) |
| `dfdfd96` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 11: 发布 Kivio Desktop v2.8.0

**Date**: 2026-07-17
**Task**: 发布 Kivio Desktop v2.8.0
**Branch**: `main`

### Summary

完成 v2.8.0 版本同步、双语发布说明、全量质量门、本地 arm64 DMG 构建与挂载验证；推送 main 和 Tag，Windows workflow 成功，GitHub Release 已包含 Windows NSIS 与 macOS DMG 并更新正式正文。

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `c02be5e` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 12: Resource lifecycle and performance hardening

**Date**: 2026-07-19
**Task**: Resource lifecycle and performance hardening
**Branch**: `main`

### Summary

Fixed child, Worker, OCR helper, Preview server, cache, knowledge-base lock, MessageList streaming, and MCP warmup lifecycle issues; added regression coverage and executable Trellis specs; all frontend, Rust, and Swift gates passed.

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `48e6f3a` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 13: 删除 Kivio Code 终端 agent 功能

**Date**: 2026-07-20
**Task**: 删除 Kivio Code 终端 agent 功能
**Branch**: `main`

### Summary

删除 kivio_code/ 模块、kivio-code binary、kivio code 子命令、cli_install 及相关 Tauri 命令与前端设置页。app_data_dir 搬迁到独立 app_data.rs。清理遗留死代码(force_compact/safe_context_window_for_model/native_enter_plan_mode_tool)与 5 个死依赖。净删约 25200 行,build/typecheck/lint/测试通过。

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `aa6995a` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 14: 截图标注收尾 + LaTeX \[...\] 分隔符修复

**Date**: 2026-07-23
**Task**: 截图标注收尾 + LaTeX \[...\] 分隔符修复
**Branch**: `main`

### Summary

修复 issue #19：chat/Lens 的 markdown 渲染不支持 \[...\] / \(...\) LaTeX 分隔符——在 normalizeMarkdownForRender 里转成 $$/$（跳过代码块，非贪婪配对不误伤流式未闭合），补测试，实机验证四类公式渲染正常。收尾 07-20-screenshot-annotate：质量门全绿（lint/typecheck/test 310/cargo 1066 passed 0 failed，优于 baseline，0 回归），PRD 验收全满足；补 CLAUDE.md 的 lens screenshot 模式说明；归档任务。

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `233c5b8` | (see git log) |
| `615e4c1` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 15: UI 开关统一 + 输入框草稿保留 + HTTP 连接池诊断修复

**Date**: 2026-07-24
**Task**: UI 开关统一 + 输入框草稿保留 + HTTP 连接池诊断修复
**Branch**: `main`

### Summary

统一技能/插件页开关到共享 Toggle 并修 MCP off 态可见性；新建/切换对话时输入框草稿(文字+附件)内存保留、恢复后光标落末尾；针对长时间运行后陈旧连接导致 error sending request(statusCode=null)，给共享 reqwest client 加 tcp/http2 keepalive+缩短 pool_idle_timeout，并新增 format_reqwest_error 展开 .source() 链让下次故障可定位层级(A+B；C 放宽重试分类待真实错误链后再做)。

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `3a686aa` | (see git log) |
| `1ff8567` | (see git log) |
| `eefa6f3` | (see git log) |
| `ac6ca55` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 16: 出图接口路由规范化 + Gemini/grok 生图打通

**Date**: 2026-07-25
**Task**: 出图接口路由规范化 + Gemini/grok 生图打通
**Branch**: `main`

### Summary

修复图片模型无图返回纯文字时的硬错误(透出文字),grok-imagine-image 改走 /images/generations;把出图端点选择从 6+ 函数 3 张模型名子串表收敛为单一 resolve_image_route + 模型名归一化(修 override 精确匹配静默失效)+ 猜错自动换端点重试并写会话缓存;真机测试通道验证 gemini→Chat/grok→ImagesApi 出图并逮到修复自愈错误串两向措辞缺口;沉淀 spec/guides/image-generation-routing.md。cargo test --lib 1132 passed。

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `dccb37a` | (see git log) |
| `d16356e` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 17: 修复审查发现的正确性 Bug + 会话重复加载

**Date**: 2026-07-25
**Task**: 修复审查发现的正确性 Bug + 会话重复加载
**Branch**: `main`

### Summary

全项目审查后收拢的 6 个正确性 Bug 全部修复：B1 多答流式列 memo 冻结（version 纳入 deps）、B2 Retry-After 巨值封顶、B3 后台作业按 conversation_id 隔离、B4a rerank 分被 fused 分覆盖、B4b 阈值把结果静默截到 rerank_top_k、B5 子任务重试谓词收窄到空响应。审查另报的多库候选池上限经核实为刻意设计，已在 prd 撤下。另修 Chat.tsx 会话重复加载：点击/新建/分支走「先 apply 再 sync 路由」，随后 hashchange 会对同一对话再 force reload 一遍；loadFromRoute 加 ref 相等短路，onChatOpenConversation 按 hash 是否变化二选一。验证 cargo test --lib 1135 passed、vitest 315/315、tsc + eslint 干净。

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `520bfe6` | (see git log) |
| `a336dde` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete


## Session 18: 拆分 SettingsShell.tsx / Chat.tsx 巨型组件 + 修聊天设置切换动画

**Date**: 2026-07-26
**Task**: 拆分 SettingsShell.tsx / Chat.tsx 巨型组件 + 修聊天设置切换动画
**Branch**: `main`

### Summary

SettingsShell.tsx 3824→2371（抽 12 个 tab 组件），Chat.tsx 4298→3924（抽 useChatRouting / useExternalSendQueue / useStreamRenderFrame / useTauriEvent，收敛 10 处订阅样板与 5 处按会话清理块）。测试 315→464，全部配变异验证。关键教训：ref 数量不等于内聚度——按此规划的 useToolConfirm / useChatStream 整体抽取被迫放弃，改按共现分析定边界后，同一批 ref 的写入侧（6 处重复删除块）可收敛、读取侧（30 处语义各异）不可搬。另修好聊天↔设置切换动画：CSS animation 走墙钟时间，大组件树挂载帧吃掉上百毫秒导致进场动画在首帧前已跑完，改 pre-enter + 双 rAF；顺带修 fill:both 残留 transform 使 fixed 遮罩失效。实测结论已填入 spec/frontend 的 hook-guidelines 与 quality-guidelines。

### Main Changes

(Add details)

### Git Commits

| Hash | Message |
|------|---------|
| `6050733` | (see git log) |
| `65c008f` | (see git log) |
| `f39887a` | (see git log) |
| `6bec239` | (see git log) |
| `d855523` | (see git log) |
| `624e2b5` | (see git log) |
| `789bf7a` | (see git log) |
| `3930d57` | (see git log) |
| `dbedcdd` | (see git log) |
| `e171c21` | (see git log) |
| `2ee81b1` | (see git log) |
| `2707e3b` | (see git log) |
| `ceb1d1c` | (see git log) |
| `dd87bed` | (see git log) |
| `c1e80a5` | (see git log) |
| `ceb37c2` | (see git log) |
| `8965c63` | (see git log) |
| `a886f6a` | (see git log) |

### Testing

- [OK] (Add test results)

### Status

[OK] **Completed**

### Next Steps

- None - task complete
