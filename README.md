<p align="center">
  <img src="public/icon.png" width="120" height="120" alt="Kivio Desktop">
</p>

<h1 align="center">Kivio Desktop</h1>

<p align="center">
  <strong>macOS / Windows 屏幕级 AI 助手：Agent 客户端，加上翻译、截图 OCR、视觉问答。热键呼出，用你自己的 API Key。</strong>
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
  <a href="#english">English</a>
  &nbsp;·&nbsp;
  QQ 群：<strong>1104450740</strong>
</p>

<p align="center">
  <img src="docs/screenshots/qq-group.png" width="220" alt="Kivio QQ 群 1104450740">
</p>

---

常驻托盘。热键翻译输入、选中文字或屏幕内容，框选后直接问 AI。打开客户端则是完整 Agent：工具、子代理、Skills、MCP、知识库、多模型并排。

自带 Key，无账号、无中转；数据只留本机，无遥测。

<a name="功能"></a>

## 功能

<p align="center">
  <img src="docs/screenshots/chat-client.png" width="840" alt="Kivio Desktop AI 客户端">
</p>

- **聊天** —— 工具循环、子代理、Skills、MCP、知识库、附件；一问多模型对比。
- **外部 CLI** —— 对话可交给本机已装的 Claude Code、Codex、Cursor、OpenCode、Gemini、Kimi、Pi、Hermes、Grok、DeepSeek Harness。
- **Lens** —— 热键冻结屏幕，框选（macOS 可点窗口）后提问，可画箭头，可把对话交回客户端。
- **翻译** —— 快速翻译、选中翻译、截图翻译、替换翻译（译文画回原位置）。

<p align="center">
  <img src="docs/screenshots/lens-formula-extraction.gif" width="760" alt="Lens 公式提取">
</p>

<p align="center">
  <img src="docs/screenshots/screenshot-translation.png" width="760" alt="截图翻译">
</p>

协议：OpenAI Chat Completions / Responses、Anthropic、Gemini。翻译、截图、Lens、每条对话可各用不同模型。

<a name="热键"></a>

## 热键

| 功能 | macOS | Windows |
|---|---|---|
| 打开聊天 | `⌘⇧K` | `Ctrl+Shift+K` |
| 快速翻译 | `⌘⌥T` | `Ctrl+Alt+T` |
| 截图翻译 | `⌘⇧A` | `Ctrl+Shift+A` |
| 选中翻译 | `⌘⇧T` | `Ctrl+Shift+T` |
| 替换翻译 | `⌘⇧R` | `Ctrl+Shift+R` |
| Lens | `⌘⇧G` | `Ctrl+Shift+G` |

都是开关，可在设置里改。

## 安装

[下载最新版](https://github.com/ZMGID/kivio/releases/latest) — macOS `.dmg`（Apple Silicon）· Windows `-setup.exe` 或解压即用的 `-portable.zip`。

DMG 未签名，首次请右键打开，或：

```bash
xattr -cr "/Applications/Kivio Desktop.app"
```

macOS 需要辅助功能与屏幕录制权限。启动后按引导填服务商即可。

## 新版本 —— v2.9.3

- Windows 便携版（解压即用）
- DeepSeek 官方 / 内置搜索
- 附件发送后保持小卡片；最后一轮可换模型再答
- 外部 CLI 跟上 Claude Code 2.1.238、Codex 0.148、dsh rc.8

完整记录：[Releases](https://github.com/ZMGID/kivio/releases)

## 开发

```bash
npm install
npm run dev          # Rust + UI（macOS 会编 Swift sidecar）
npm run dev:ui       # 只开 Vite
npm run lint && npm run typecheck && npm test
cargo test --manifest-path src-tauri/Cargo.toml
```

GPL-3.0-or-later © ZM · [LINUX DO](https://linux.do) · QQ **1104450740**

---

<a name="english"></a>

<h1 align="center">Kivio Desktop · English</h1>

<p align="center">
  <strong>A screen-level AI assistant for macOS and Windows: an agentic client, plus translation, screenshot OCR, and visual Q&amp;A. Hotkey anywhere, your own API keys.</strong>
</p>

<p align="center">
  <a href="https://github.com/ZMGID/kivio/releases/latest"><strong>Download</strong></a>
  &nbsp;·&nbsp;
  <a href="#features">Features</a>
  &nbsp;·&nbsp;
  <a href="#hotkeys">Hotkeys</a>
  &nbsp;·&nbsp;
  <a href="#kivio-desktop">中文</a>
  &nbsp;·&nbsp;
  QQ: <strong>1104450740</strong>
</p>

---

Lives in the tray. Hotkeys translate typing, selection, or what's on screen; capture a region and ask. The client is a full agent: tools, sub-agents, Skills, MCP, knowledge base, multi-model replies.

Bring your own keys. No account, no proxy, no telemetry. Data stays on disk.

<a name="features"></a>

## Features

<p align="center">
  <img src="docs/screenshots/chat-client.png" width="840" alt="Kivio Desktop AI client">
</p>

- **Chat** — tool loop, sub-agents, Skills, MCP, knowledge base, attachments; one question, many models.
- **External CLIs** — hand a conversation to Claude Code, Codex, Cursor, OpenCode, Gemini, Kimi, Pi, Hermes, Grok, or DeepSeek Harness if they're installed.
- **Lens** — freeze the screen, select a region (or a window on macOS), ask, optionally annotate, send the thread back to chat.
- **Translate** — quick, selection, screenshot, and in-place replace.

<p align="center">
  <img src="docs/screenshots/lens-formula-extraction.gif" width="760" alt="Lens formula extraction">
</p>

<p align="center">
  <img src="docs/screenshots/screenshot-translation.png" width="760" alt="Screenshot translation">
</p>

Protocols: OpenAI Chat Completions / Responses, Anthropic, Gemini. Translator, screenshot, Lens, and each chat can use a different model.

<a name="hotkeys"></a>

## Hotkeys

| Action | macOS | Windows |
|---|---|---|
| Open chat | `⌘⇧K` | `Ctrl+Shift+K` |
| Quick translate | `⌘⌥T` | `Ctrl+Alt+T` |
| Screenshot translate | `⌘⇧A` | `Ctrl+Shift+A` |
| Selection translate | `⌘⇧T` | `Ctrl+Shift+T` |
| Replace translate | `⌘⇧R` | `Ctrl+Shift+R` |
| Lens | `⌘⇧G` | `Ctrl+Shift+G` |

Toggles, remappable in Settings.

## Install

[Latest release](https://github.com/ZMGID/kivio/releases/latest) — macOS `.dmg` (Apple Silicon) · Windows `-setup.exe` or unzip-and-run `-portable.zip`.

The DMG is unsigned. First launch: right-click → Open, or:

```bash
xattr -cr "/Applications/Kivio Desktop.app"
```

macOS needs Accessibility and Screen Recording. Then follow the onboarding wizard.

## What's New — v2.9.3

- Windows portable zip
- Official / hosted DeepSeek search
- Compact attachment cards; reply to the last turn with another model
- External CLI catch-up: Claude Code 2.1.238, Codex 0.148, dsh rc.8

[Releases](https://github.com/ZMGID/kivio/releases)

## Development

```bash
npm install
npm run dev          # Rust + UI (builds Swift sidecar on macOS)
npm run dev:ui       # Vite only
npm run lint && npm run typecheck && npm test
cargo test --manifest-path src-tauri/Cargo.toml
```

GPL-3.0-or-later © ZM · [LINUX DO](https://linux.do) · QQ **1104450740**
