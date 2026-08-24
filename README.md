<p align="center">
  <a href="README.md">English</a>
  &nbsp;·&nbsp;
  <a href="README.zh-CN.md">中文</a>
</p>

<p align="center">
  <img src="public/icon.png" width="120" height="120" alt="Kivio Desktop">
</p>

<h1 align="center">Kivio Desktop</h1>

<p align="center">
  <strong>A screen-level AI assistant for macOS and Windows: an agentic client, plus translation, screenshot OCR, and visual Q&amp;A. Hotkey anywhere, your own API keys.</strong>
</p>

<p align="center">
  <a href="https://github.com/ZMGID/kivio/releases/latest"><img src="https://img.shields.io/github/v/release/ZMGID/kivio?style=flat-square&color=4f46e5&label=release" alt="Latest Release"></a>
  <img src="https://img.shields.io/badge/macOS-Apple%20Silicon-success?style=flat-square" alt="macOS (Apple Silicon)">
  <img src="https://img.shields.io/badge/Windows-10%2F11-success?style=flat-square" alt="Windows 10/11">
  <img src="https://img.shields.io/badge/Tauri-v2-24273a?style=flat-square" alt="Tauri v2">
  <img src="https://img.shields.io/badge/license-GPL--3.0-blue?style=flat-square" alt="GPL-3.0">
</p>

<p align="center">
  <a href="https://github.com/ZMGID/kivio/releases/latest"><strong>Download</strong></a>
  &nbsp;·&nbsp;
  <a href="#features">Features</a>
  &nbsp;·&nbsp;
  <a href="#hotkeys">Hotkeys</a>
  &nbsp;·&nbsp;
  QQ: <strong>1104450740</strong>
</p>

<p align="center">
  <img src="docs/screenshots/qq-group.png" width="220" alt="Kivio QQ group 1104450740">
</p>

---

Lives in the tray. Hotkeys translate typing, selection, or what's on screen; capture a region and ask. The client is a full agent: tools, sub-agents, Skills, MCP, knowledge base, multi-model replies.

Bring your own keys. No account, no proxy, no telemetry. Data stays on disk.

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
