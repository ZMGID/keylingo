<p align="center">
  <img src="public/icon.png" width="120" height="120" alt="Kivio Desktop">
</p>

<h1 align="center">Kivio Desktop</h1>

<p align="center">
  <strong>macOS / Windows 屏幕级 AI 助手：一个 Agentic AI 客户端，加上即时翻译、截图 OCR、视觉问答 —— 全部一键呼出，全部用你自己的 API Key。</strong>
</p>

<p align="center">
  <a href="https://github.com/ZMGID/kivio/releases/latest"><img src="https://img.shields.io/github/v/release/ZMGID/kivio?style=flat-square&color=4f46e5&label=release" alt="Latest Release"></a>
  <img src="https://img.shields.io/badge/macOS-Apple%20Silicon-success?style=flat-square" alt="macOS (Apple Silicon)">
  <img src="https://img.shields.io/badge/Windows-10%2F11-success?style=flat-square" alt="Windows 10/11">
  <img src="https://img.shields.io/badge/Tauri-v2-24273a?style=flat-square" alt="Tauri v2">
  <img src="https://img.shields.io/badge/license-GPL--3.0-blue?style=flat-square" alt="GPL-3.0">
</p>

<p align="center">
  <a href="https://github.com/ZMGID/kivio/releases/latest"><strong>下载</strong></a>
  &nbsp;·&nbsp;
  <a href="#功能">功能</a>
  &nbsp;·&nbsp;
  <a href="#热键">热键</a>
  &nbsp;·&nbsp;
  <a href="#快速开始">快速开始</a>
  &nbsp;·&nbsp;
  <a href="#english">English</a>
  &nbsp;·&nbsp;
  QQ 群：<strong>1104450740</strong>
</p>

<p align="center">
  <img src="docs/screenshots/qq-group.png" width="220" alt="Kivio QQ 群 1104450740">
</p>

---

## Kivio Desktop 是什么？

Kivio Desktop 常驻托盘 / 菜单栏，工作在整个**屏幕**层面，而不只是自己的窗口里。在任何地方按下热键：翻译你输入的、翻译你选中的、翻译你看到的，或者框选屏幕任意区域直接向 AI 提问。从托盘打开 AI 客户端，则是一个完整的 Agent 聊天应用：工具调用、子代理、Skills、MCP、知识库、多模型并排回答。

代码里落实的三条设计原则：

- **自带 Key。** 所有 AI 调用都走你自己配置的服务商 —— 原生支持 OpenAI 兼容、Anthropic、Google Gemini 三类协议。没有账号系统，没有中转服务器。
- **本地、安静。** 全无遥测和统计上报；唯一的后台网络请求是 GitHub 版本检查。设置与对话数据只存在本机磁盘。
- **空闲时轻。** 窗口按需创建、关闭即销毁（不是隐藏），空闲进程保持很小的占用。

<a name="功能"></a>

## AI 客户端

<p align="center">
  <img src="docs/screenshots/chat-client.png" width="840" alt="Kivio Desktop AI 客户端">
</p>

与服务商无关的 Agent 运行时，带真正的工具循环，不是聊天套壳。

**一次问多个模型。** 把一个问题同时发给多个模型，以标签页或并排分栏对比回答。每个回答独立流式生成，某个模型报错不影响其他列，最后由你点选哪个回答进入后续上下文。

**原生工具**（每个可单独开关，文件/终端类工具需按对话授权一次）：

| 分组 | 工具 |
|---|---|
| 网络 | `web_search`、`web_fetch` |
| 文件 | `read`（文件、目录、图片）、`grep`、`glob`、`write`、`edit` |
| 终端 | `bash`，支持可追踪的后台任务（`bash_output`、`kill_background`） |
| 知识库 | `knowledge_search`，回答带 `[n]` 引用 |
| 记忆 | `memory_read` / `memory_modify` / `memory_search` 长期记忆 |
| Agent | `agent`（子代理）、`todo_write`、`ask_user`、图片生成 |

**子代理。** 内置 `general-purpose`、`researcher`、`coder`、`reviewer` 四种人格，各有工具白名单；模型在一条消息里就能并行分派多个。也可以用 Markdown 文件添加自己的子代理。

**Skills。** Markdown 定义的技能，对话中即时激活。内置：`pdf`、`docx`、`xlsx`、`diagram`、`doc-coauthoring`、`frontend-design`、`mcp-builder`、`skill-creator`。支持从文件夹或 ZIP 导入自己的技能。

**MCP。** 接入外部 Model Context Protocol 服务器（stdio 或 streamable HTTP），持久会话、JSON 导入、实时连接状态。

**知识库（RAG）。** 多库文档检索：混合搜索（sqlite-vec 向量 + FTS5 BM25，RRF 融合）+ 可选重排。支持导入 txt / csv / markdown / html / docx / xlsx / pdf（文本层），图片走 OCR 入库，网页可按 URL 导入。回答中的 `[n]` 引用可点击跳转来源。

**连接器。** Obsidian（笔记库注入）、Notion、GitHub、Linear、Sentry、Atlassian、Composio —— Token 或 OAuth 2.1 + PKCE。

**外部 CLI Agent。** 把对话交给已安装的终端 Agent 接管 —— Claude Code、codex、cursor、opencode、gemini、kimi、pi、hermes —— 自动检测、流式输出、会话管理都已内置。

**长对话不失忆。** 上下文压缩内建在循环里：先用轻量 "microcompact" 降解旧工具结果，预算不够时才动用 LLM 摘要，界面上有可视化的压缩时间线。

**还有：** 项目与集两种对话组织方式、对话全文搜索、文件/图片附件、助手搭建器、带审批策略的计划/编排模式、Agent 待办列表、生成文件卡片（`~/Kivio/outputs/`）、按调用的 Token 用量统计。

## 屏幕工具

### Lens —— 截什么，问什么

<p align="center">
  <img src="docs/screenshots/lens-formula-extraction.gif" width="760" alt="Lens 公式提取">
</p>

一个热键冻结屏幕。拖拽框选区域（macOS 还可以直接点选窗口），可以画红色箭头指着要问的地方，然后提问。回答流式呈现：思考过程收在可折叠的推理块里，公式由 KaTeX 渲染，最多保留 20 条截图+问答历史。Lens 还会自己规划联网搜索（Tavily / Exa / Exa MCP / Ollama / Grok —— Exa MCP 无 Key 也有低额度可用）并展示来源。一键即可把截图 —— 或整段多轮对话 —— 交接到 AI 客户端继续。

<p align="center">
  <img src="docs/screenshots/lens-optimize-text.gif" width="760" alt="Lens 文本问答">
  <br>
  <sub>截取屏幕上的文字，原地继续处理。</sub>
</p>

### 翻译，四种姿势

<p align="center">
  <img src="docs/screenshots/screenshot-translation.png" width="760" alt="截图翻译">
</p>

- **快速翻译** —— 鼠标旁弹出小输入框，边输边译（600 ms 防抖）；回车把译文写入剪贴板，并可自动粘贴回原来的应用。
- **选中翻译** —— 通过无障碍 API 抓取当前选中文本（失败时回退剪贴板方案），弹出可拖动的浮动译文卡；没选中任何内容则静默不弹。
- **截图翻译** —— 框选区域或窗口，译文流式出现在选区旁的卡片里，下方附识别出的原文。
- **替换翻译** —— 框选后，译文按行直接"画"在原文位置上，背景色取自截图采样，融入原画面。行定位固定使用 RapidOCR。

每种模式的提示词都可编辑（支持 `{lang}` / `{text}` 占位符），卡片宽度可调，流式输出可开关。

### OCR 引擎

截图翻译的文字识别三选一，在设置中切换：

- **云端视觉模型**（默认）—— 一次多模态调用同时完成 OCR + 翻译。
- **系统 OCR** —— macOS 走 Apple Vision（随包 Swift sidecar），Windows 走 Windows.Media.Ocr。
- **RapidOCR** —— 完全离线的 PaddleOCR（PP-OCRv6 medium，50 种语言）ONNX 管线；由用户主动一键下载（约 139 MB 模型 + ONNX Runtime）。替换翻译固定使用此引擎。

## 模型与服务商

- **四种原生协议：** OpenAI Chat Completions、OpenAI Responses、Anthropic Messages、Google Gemini `generateContent` —— 各是一等适配器，不经有损的兼容层。
- **预设** DeepSeek、OpenRouter、SiliconFlow、GLM、Ollama Cloud，各带"获取 API Key"直达链接；任何 OpenAI 兼容端点都可以自定义添加。
- **按功能路由：** 翻译、截图翻译、Lens、每个聊天对话都可以分别指定服务商和模型；视觉、标题摘要、压缩、图片生成还有各自独立的默认模型槽位。
- **多 Key 故障转移：** 每个服务商可配置一组 Key。鉴权错误（401/402/403）立即换 Key；限流（429）先退避重试、超过阈值才切换；失败的 Key 冷却 60 秒。服务器错误只退避、不消耗备用 Key。
- **按模型覆盖**（上下文窗口、最大输出、能力、价格），以及按服务商的 gzip 请求体压缩开关（应付挑剔的 WAF 网关）。

## 设置

<p align="center">
  <img src="docs/screenshots/settings.png" width="560" alt="Kivio Desktop 设置">
</p>

设置内嵌在 AI 客户端窗口里：通用、翻译、截图、Lens、聊天、记忆、默认模型、Kivio Code、外部 Agent、MCP、Skills、联网搜索、连接器、知识库、用量、服务商、关于。亮点：首次启动分步引导（服务商 → 联网搜索 → 快捷键）、设置导出/导入备份、主题色预设与深色模式、中英双语界面、开机自启，以及一个只存内存的请求调试面板 —— 密钥自动掩码、可复制为 cURL。

## Kivio Code

仓库还内置 `kivio-code`：基于同一套运行时的终端编码 Agent（Rust CLI/TUI），也可用主程序的 `kivio code` 子命令启动，自带会话、MCP 配置与技能装载。

<a name="热键"></a>

## 热键

| 功能 | macOS | Windows |
|---|---|---|
| 快速翻译 | `⌘⌥T` | `Ctrl+Alt+T` |
| 截图翻译 | `⌘⇧A` | `Ctrl+Shift+A` |
| 选中翻译 | `⌘⇧T` | `Ctrl+Shift+T` |
| 替换翻译 | `⌘⇧R` | `Ctrl+Shift+R` |
| Lens 截图问答 | `⌘⇧G` | `Ctrl+Shift+G` |

所有热键都是开关式（再按一次关闭），可在设置中重新绑定（带冲突检测）。托盘菜单：打开 AI 客户端 · 显示翻译器 · 设置 · 退出。

<a name="快速开始"></a>

## 快速开始

1. **[下载最新版](https://github.com/ZMGID/kivio/releases/latest)** —— macOS：Apple Silicon `.dmg` · Windows：NSIS `-setup.exe`，或解压即用的 `-portable.zip`。
2. **安装并启动。** DMG 未签名，首次打开请右键 → 打开，或执行：
   ```bash
   xattr -cr "/Applications/Kivio Desktop.app"
   ```
   macOS 会请求**辅助功能**（热键、选中取词、粘回）与**屏幕录制**（截图）权限；屏幕捕获基于 ScreenCaptureKit。Windows 手动启动时默认打开 AI 客户端。
3. **跟随首次引导** —— 添加服务商，可选配置联网搜索，确认快捷键。
4. **开始用。** 托盘 → 打开 AI 客户端做聊天、工具与文档；或在任意界面按热键使用翻译和 Lens。

Kivio Desktop 启动后会检查 GitHub Releases 的新版本（可关闭），并支持应用内直接下载安装更新。

## 新版本 —— v2.9.3

- **便携版** —— Windows 增加解压即用的 zip。
- **DeepSeek 搜索** —— 官方搜索与内置托管搜索；Responses 解析修好。
- **MCP / 外部 CLI** —— 服务按来源分组；跟上 Claude Code 2.1.238、Codex 0.148、dsh rc.8。Codex 沙箱逃逸改为询问。
- **聊天** —— 附件保持小卡片；最后一轮可换模型再答；可切断上下文；侧栏可重生标题。
- **其它** —— Ox Alpha；编程计划预设弹窗。去掉轨迹 Dock 与捆绑 Pyodide。

完整历史见 [GitHub Releases](https://github.com/ZMGID/kivio/releases)。

## 开发

| 层 | 技术栈 |
|---|---|
| 后端 | Rust · Tauri v2 |
| 前端 | React 18 · TypeScript · Vite · TailwindCSS v4 |
| OCR | Apple Vision（Swift sidecar）· Windows.Media.Ocr · RapidOCR（ONNX） |

```bash
npm install
npm run dev          # 完整应用：Rust 后端 + Vite UI（macOS 上自动构建 Swift sidecar）
npm run dev:ui       # 仅 Vite UI，不编译 Rust

npm run lint         # ESLint，零警告
npm run typecheck    # tsc --noEmit
npm test             # Vitest 前端测试
cargo test --manifest-path src-tauri/Cargo.toml   # Rust 测试
```

架构说明：[CLAUDE.md](CLAUDE.md) 与 `docs/`。

## 许可证

GPL-3.0-or-later © ZM。见 [LICENSE](LICENSE)。

## 社区

- [LINUX DO](https://linux.do)
- QQ 群：**1104450740**

---

<a name="english"></a>

<h1 align="center">Kivio Desktop · English</h1>

<p align="center">
  <strong>A screen-level AI assistant for macOS and Windows: an agentic AI client, plus instant translation, screenshot OCR, and visual Q&A — all one hotkey away, all on your own API keys.</strong>
</p>

<p align="center">
  <a href="https://github.com/ZMGID/kivio/releases/latest"><strong>Download</strong></a>
  &nbsp;·&nbsp;
  <a href="#features">Features</a>
  &nbsp;·&nbsp;
  <a href="#hotkeys">Hotkeys</a>
  &nbsp;·&nbsp;
  <a href="#quick-start">Quick Start</a>
  &nbsp;·&nbsp;
  <a href="#kivio-desktop">中文</a>
  &nbsp;·&nbsp;
  QQ Group: <strong>1104450740</strong>
</p>

---

## What is Kivio Desktop?

Kivio Desktop lives in your tray / menu bar and works at the level of your *screen*, not just inside its own window. Press a hotkey anywhere to translate what you typed, translate what you selected, translate what you see, or capture any region and ask AI about it. Open the AI client from the tray and you get a full agentic chat app: tool calls, sub-agents, Skills, MCP servers, a knowledge base, and side-by-side multi-model answers.

Design principles, as implemented in code:

- **Bring your own keys.** Every AI call goes to providers *you* configure — OpenAI-compatible, Anthropic, and Google Gemini native protocols. No account, no middleman server.
- **Local and quiet.** No telemetry or analytics of any kind; the only background network call is the GitHub release check for updates. Settings and conversations stay on disk on your machine.
- **Light when idle.** Windows are created on demand and *destroyed* on close (not hidden), so the idle process keeps a small footprint.

<a name="features"></a>

## The AI Client

<p align="center">
  <img src="docs/screenshots/chat-client.png" width="840" alt="Kivio Desktop AI client">
</p>

A provider-agnostic agent runtime with a real tool loop, not a thin chat wrapper.

**Ask many models at once.** Fan one question out to multiple models and compare the answers in tabs or side-by-side columns. Each answer streams independently; one model failing never blocks the rest, and you choose which answer the conversation continues from.

**Native tools** (each individually toggleable, file/shell tools ask for per-conversation consent):

| Group | Tools |
|---|---|
| Web | `web_search`, `web_fetch` |
| Files | `read` (files, directories, images), `grep`, `glob`, `write`, `edit` |
| Shell | `bash` with tracked background jobs (`bash_output`, `kill_background`) |
| Knowledge | `knowledge_search` with `[n]` citations |
| Memory | `memory_read` / `memory_modify` / `memory_search` long-term memory |
| Agent | `agent` (sub-agents), `todo_write`, `ask_user`, image generation |

**Sub-agents.** Built-in personas — `general-purpose`, `researcher`, `coder`, `reviewer` — each with its own tool allow-list; the model can dispatch several in parallel from a single message. You can add your own as markdown files.

**Skills.** Markdown-defined skills, activated mid-conversation. Bundled: `pdf`, `docx`, `xlsx`, `diagram`, `doc-coauthoring`, `frontend-design`, `mcp-builder`, `skill-creator`. Import your own from folders or ZIPs.

**MCP.** Connect external Model Context Protocol servers over stdio or streamable HTTP, with persistent sessions, JSON import, and live connection status.

**Knowledge base (RAG).** Multi-library document retrieval: hybrid search (sqlite-vec vectors + FTS5 BM25, fused by Reciprocal Rank Fusion) with an optional reranker. Ingests txt / csv / markdown / html / docx / xlsx / pdf (text layer), plus images via OCR and web pages via URL import. Answers cite sources as clickable `[n]` markers.

**Connectors.** Obsidian (vault injection), Notion, GitHub, Linear, Sentry, Atlassian, Composio — token or OAuth 2.1 + PKCE.

**External CLI agents.** Hand a conversation over to an installed terminal agent — Claude Code, codex, cursor, opencode, gemini, kimi, pi, or hermes — with detection, streaming, and session management built in.

**Long conversations that keep working.** Context compaction runs inside the loop: a cheap "microcompact" pass degrades old tool results first, and an LLM summary kicks in only when needed, with a visible compaction timeline in the UI.

**And the rest:** projects and sets for organizing conversations, full-text conversation search, file/image attachments, an assistant builder, plan/orchestrate mode with approval policies, agent todo lists, generated-file cards (`~/Kivio/outputs/`), and per-call token usage statistics.

## Screen Tools

### Lens — capture and ask

<p align="center">
  <img src="docs/screenshots/lens-formula-extraction.gif" width="760" alt="Lens formula extraction">
</p>

One hotkey freezes the screen. Drag a region (or, on macOS, click a window), optionally draw red arrows to point at things, then ask. Answers stream in with reasoning shown in a collapsible thinking block, LaTeX rendered by KaTeX, and up to 20 capture+Q&A entries kept in history. Lens can also plan its own web searches (Tavily / Exa / Exa MCP / Ollama / Grok — Exa MCP works keyless at low quota) and show the sources it used. One click sends the screenshot — or the entire multi-turn exchange — into the AI client to continue.

<p align="center">
  <img src="docs/screenshots/lens-optimize-text.gif" width="760" alt="Lens text Q&A">
  <br>
  <sub>Capture text on screen and work with it in place.</sub>
</p>

### Translation, four ways

<p align="center">
  <img src="docs/screenshots/screenshot-translation.png" width="760" alt="Screenshot translation">
</p>

- **Quick translator** — a small input popup at your cursor; results appear as you type (600 ms debounce), Enter copies the translation and can auto-paste it back into the app you came from.
- **Selected-text translation** — grabs the current selection via Accessibility APIs (with a clipboard fallback) and shows a floating, draggable translation card. Nothing pops up if nothing is selected.
- **Screenshot translation** — capture a region or window; the translation streams into a card next to the selection, with the recognized original underneath.
- **Replace translation** — capture a region and the translation is painted *over* the original text on a canvas, line by line, with background color sampled from the screenshot so it blends in. Uses RapidOCR for line positions.

Prompts for each mode are editable (`{lang}` / `{text}` placeholders), card width is adjustable, and streaming can be toggled.

### OCR engines

Screenshot translation can recognize text three ways, selectable in Settings:

- **Cloud vision model** (default) — one multimodal call does OCR + translation together.
- **System OCR** — Apple Vision on macOS (via a bundled Swift sidecar) or Windows.Media.Ocr on Windows.
- **RapidOCR** — fully offline PaddleOCR (PP-OCRv6 medium, 50 languages) ONNX pipeline; a one-click, user-initiated download (~139 MB models + ONNX Runtime). Replace translation always uses this engine.

## Models & Providers

- **Four native wire protocols:** OpenAI Chat Completions, OpenAI Responses, Anthropic Messages, and Google Gemini `generateContent` — each a first-class adapter, so no feature is lost to a compatibility layer.
- **Presets** for DeepSeek, OpenRouter, SiliconFlow, GLM, and Ollama Cloud, each with a "get API key" link; any OpenAI-compatible endpoint works via custom provider.
- **Per-feature routing:** the translator, screenshot translation, Lens, and each chat conversation can each use a different provider and model; separate default slots exist for vision, title summarization, compaction, and image generation.
- **Multi-key failover:** each provider holds a pool of API keys. Auth errors (401/402/403) switch keys immediately; rate limits (429) retry with backoff and only switch after a threshold; failed keys cool down for 60 s. Server errors back off without burning backup keys.
- **Per-model overrides** (context window, max output, capabilities, pricing) and a per-provider gzip request-body toggle for WAF-fussy gateways.

## Settings

<p align="center">
  <img src="docs/screenshots/settings.png" width="560" alt="Kivio Desktop settings">
</p>

Settings live inside the AI client window: General, Translate, Screenshot, Lens, Chat, Memory, default-model routing, Kivio Code, external agents, MCP, Skills, Web Search, Connectors, Knowledge Base, Usage, Providers, and About. Highlights: a first-run wizard (provider → web search → hotkeys), settings export/import backup, theme color presets with dark mode, bilingual UI (中文/English), autostart, and a request debug panel that records recent provider calls in memory only — keys masked, copy-as-cURL.

## Kivio Code

The repo also ships `kivio-code`, a terminal coding agent (Rust CLI/TUI) built on the same runtime — also reachable as `kivio code` from the main binary, with its own sessions, MCP setup, and skill staging.

<a name="hotkeys"></a>

## Hotkeys

| Action | macOS | Windows |
|---|---|---|
| Quick translator | `⌘⌥T` | `Ctrl+Alt+T` |
| Screenshot translation | `⌘⇧A` | `Ctrl+Shift+A` |
| Selected-text translation | `⌘⇧T` | `Ctrl+Shift+T` |
| Replace translation | `⌘⇧R` | `Ctrl+Shift+R` |
| Lens capture & ask | `⌘⇧G` | `Ctrl+Shift+G` |

All hotkeys act as toggles and are remappable in Settings (with conflict detection). The tray menu has: Open AI Client · Show Translator · Settings · Quit.

<a name="quick-start"></a>

## Quick Start

1. **[Download the latest release](https://github.com/ZMGID/kivio/releases/latest)** — macOS: Apple Silicon `.dmg` · Windows: NSIS `-setup.exe` or unzip-and-run `-portable.zip`.
2. **Install and launch.** The DMG is unsigned; on first launch right-click → Open, or run:
   ```bash
   xattr -cr "/Applications/Kivio Desktop.app"
   ```
   macOS will ask for **Accessibility** (hotkeys, selected-text capture, paste-back) and **Screen Recording** (capture) permissions. Screen capture uses ScreenCaptureKit. On Windows, launching manually opens the AI client.
3. **Follow the first-run wizard** — add a provider, optionally set up web search, confirm hotkeys.
4. **Go.** Tray → Open AI Client for chat, tools, and documents; or press a hotkey anywhere for translation and Lens.

Kivio Desktop checks GitHub Releases for updates shortly after launch (can be disabled) and can download and install the update in-app.

## What's New — v2.9.3

- **Portable Windows build** — unzip-and-run zip, no installer.
- **DeepSeek search** — official and hosted builtin search; Responses parsing fixed.
- **MCP / external CLIs** — servers grouped by source; catch up Claude Code 2.1.238, Codex 0.148, dsh rc.8. Codex sandbox escapes ask first.
- **Chat** — compact attachment cards after send; reply with another model; context cutoff; regenerate titles from the sidebar.
- **More** — Ox Alpha; coding-plan presets in a modal. Trajectory dock and bundled Pyodide removed.

Full history: [GitHub Releases](https://github.com/ZMGID/kivio/releases).

## Development

| Layer | Stack |
|---|---|
| Backend | Rust · Tauri v2 |
| Frontend | React 18 · TypeScript · Vite · TailwindCSS v4 |
| OCR | Apple Vision (Swift sidecar) · Windows.Media.Ocr · RapidOCR (ONNX) |

```bash
npm install
npm run dev          # full app: Rust backend + Vite UI (builds Swift sidecar on macOS)
npm run dev:ui       # Vite UI only, no Rust compile

npm run lint         # ESLint, zero warnings allowed
npm run typecheck    # tsc --noEmit
npm test             # Vitest frontend suite
cargo test --manifest-path src-tauri/Cargo.toml   # Rust tests
```

Architecture notes: [CLAUDE.md](CLAUDE.md) and `docs/`.

## License

GPL-3.0-or-later © ZM. See [LICENSE](LICENSE).

## Community

- [LINUX DO](https://linux.do)
- QQ Group: **1104450740**
