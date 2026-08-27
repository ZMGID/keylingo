<div align="center">

<img src="public/icon.png" width="120" height="120" alt="Kivio Desktop">

# Kivio Desktop

### macOS / Windows 屏幕级 AI 助手：Agent 客户端，加上翻译、截图 OCR 与视觉问答

[![Release](https://img.shields.io/github/v/release/ZMGID/kivio?style=flat-square&color=4f46e5&label=release)](https://github.com/ZMGID/kivio/releases/latest)
[![Platform](https://img.shields.io/badge/platform-macOS%20%7C%20Windows-lightgrey?style=flat-square)](https://github.com/ZMGID/kivio/releases)
[![Tauri](https://img.shields.io/badge/built%20with-Tauri%202-orange?style=flat-square)](https://tauri.app/)
[![Downloads](https://img.shields.io/github/downloads/ZMGID/kivio/total?style=flat-square)](https://github.com/ZMGID/kivio/releases/latest)
[![License](https://img.shields.io/badge/license-GPL--3.0-blue?style=flat-square)](LICENSE)

**中文** · [English](README.en.md) · [更新日志](https://github.com/ZMGID/kivio/releases)

[下载](https://github.com/ZMGID/kivio/releases/latest) · [功能](#功能) · [文档](#文档) · QQ 群 **1104450740**

<img src="docs/screenshots/qq-group.png" width="220" alt="Kivio QQ 群 1104450740">

</div>

常驻托盘。热键翻译输入、选中文字或屏幕内容，框选后直接问 AI。打开客户端则是完整 Agent：工具、子代理、Skills、MCP、知识库、多模型并排。

自带 Key，无账号、无中转；数据只留本机，无遥测。

## ❤️ 赞助

> 想出现在这里？欢迎通过 [GitHub Issues](https://github.com/ZMGID/kivio/issues) 或 QQ 群 **1104450740** 联系。

<details open>
<summary>点击折叠</summary>

<table>
<tr>
<td width="180" align="center" valign="middle">
<a href="https://hezubus.cc"><img src="docs/sponsors/hezubus.png" alt="合租巴士 Hezubus" width="150"></a>
</td>
<td>
感谢 <a href="https://hezubus.cc">合租巴士</a> 赞助本项目。<a href="https://hezubus.cc">合租巴士</a> 提供 GPT / Claude 等多款模型的官方稳定极速 API 中转服务，支持企业级定制、报销开票、7×16h 专属技术支持，更有独家适配的 WebSocket 连接方式，畅享极速首字速度。Codex 补贴倍率低至 0.08。<a href="https://hezubus.cc">点此注册</a>。
</td>
</tr>
</table>

</details>

## 为什么用 Kivio

屏幕上的字、框选的画面、本机已装的编码 CLI，不必拆成三套软件。Kivio 把它们收进同一个托盘应用：热键随时呼出，对话走你自己的供应商。

- **一个循环，多种宿主** — 内置 Agent、只读研究用的 Kivio Chat、以及本机外部 CLI（Claude Code、Codex、Cursor、OpenCode、Gemini、Kimi、Pi、Hermes、Grok、DeepSeek Harness），共用同一套聊天界面
- **屏幕级工具** — 快速翻译、选中翻译、截图翻译、替换翻译（译文画回原位置）；Lens 冻结屏幕后提问、标注，并可把对话交回客户端
- **自带 Key** — OpenAI Chat Completions / Responses、Anthropic、Gemini、Grok（xAI Responses）；翻译、截图、Lens、每条对话可各用不同模型
- **扩展** — Skills、MCP、知识库、子代理、插件、连接器；右侧 Dock 带文件树、Git 与终端
- **跨平台** — macOS（Apple Silicon）与 Windows 10/11，Tauri 2 原生应用

## 截图

| 聊天客户端 | 设置 |
| :--------: | :--: |
| ![聊天客户端](docs/screenshots/chat-client.png) | ![设置](docs/screenshots/settings.png) |

<p align="center">
  <img src="docs/screenshots/lens-formula-extraction.gif" width="760" alt="Lens 公式提取">
</p>

<p align="center">
  <img src="docs/screenshots/screenshot-translation.png" width="760" alt="截图翻译">
</p>

## 功能

完整记录见 [Releases](https://github.com/ZMGID/kivio/releases) · 当前版本说明：[v2.9.4](docs/releases/v2.9.4.md)

### 聊天与 Agent

- 工具循环、子代理、Skills、MCP、知识库、附件；一问多模型对比
- 三种运行时：Kivio Agent（完整工具）、Kivio Chat（检索 / 抓取 / 知识库等只读能力）、外部 CLI
- 对话可交给本机已装的 Claude Code、Codex、Cursor、OpenCode、Gemini、Kimi、Pi、Hermes、Grok 或 DeepSeek Harness
- 可从本机 CLI 导入原生会话（钉在原 CLI 与原工作目录上续聊）

### 翻译与 Lens

- 快速翻译、选中翻译、截图 OCR 翻译、替换翻译
- Lens：热键冻结屏幕，框选（macOS 可点窗口）后提问，可画箭头 / 矩形 / 马赛克，可把对话交回客户端
- OCR：系统引擎（Apple Vision / Windows OCR）或可选 RapidOCR 离线包

### 系统

- 全局热键（可改、带冲突检测）、托盘常驻、浅色 / 深色
- 用量统计、生命周期钩子、可选聊天窗口保活（关闭后隐藏而不是销毁）

## 热键

都是开关，可在设置里改。

| 功能 | macOS | Windows |
|---|---|---|
| 打开聊天 | `⌘⇧K` | `Ctrl+Shift+K` |
| 快速翻译 | `⌘⌥T` | `Ctrl+Alt+T` |
| 截图翻译 | `⌘⇧A` | `Ctrl+Shift+A` |
| 选中翻译 | `⌘⇧T` | `Ctrl+Shift+T` |
| 替换翻译 | `⌘⇧R` | `Ctrl+Shift+R` |
| Lens | `⌘⇧G` | `Ctrl+Shift+G` |

## 下载安装

### 系统要求

- **macOS**：Apple Silicon（`.dmg` 未签名）
- **Windows**：Windows 10 / 11（需要 Edge WebView2，系统通常已带）

从 [Releases](https://github.com/ZMGID/kivio/releases/latest) 下载：

- macOS：`Kivio.Desktop_*_aarch64.dmg`
- Windows 安装包：`Kivio.Desktop_*_x64-setup.exe`
- Windows 便携版：`Kivio.Desktop_*_x64-portable.zip`（解压即用）

macOS 首次请右键打开，或：

```bash
xattr -cr "/Applications/Kivio Desktop.app"
```

macOS 需要**辅助功能**与**屏幕录制**权限。启动后按引导填供应商即可。

## 文档

| 文档 | 说明 |
|---|---|
| [领域术语](CONTEXT.md) | 产品用词（外部 CLI 代理、内置运行时、原生会话、集 / 项目……） |
| [架构决策](docs/adr/) | 已采纳的 ADR（导入会话、侧栏顺序等） |
| [聊天架构](docs/CHAT_ARCHITECTURE.md) | 聊天子系统设计 |
| [Agent 运行时](docs/CHAT_AGENT_RUNTIME_PRD.md) | Agent 循环与工具 |
| [Chat Probe](docs/chat-probe.md) | debug 构建的无头测试通道 |
| [发版打包](docs/RELEASE_PACKAGING.md) | DMG / NSIS 发布清单 |
| [版本说明](docs/releases/) | 各版本亮点 |
| [渲染基线](docs/perf/chat-rendering-baseline.md) | 聊天渲染性能标定 |
| [开发说明](CLAUDE.md) | 仓库结构、命令与实现约束 |

模型适配器契约：[src-tauri/src/chat/model/README.md](src-tauri/src/chat/model/README.md)

## 快速开始

1. 安装并打开 Kivio，完成首次引导（供应商 + 热键）。
2. **添加供应商**：设置 → 供应商 → 添加驱动 → 选预设（或自定义），填入 API Key。
3. **聊天**：`⌘⇧K` / `Ctrl+Shift+K` 打开客户端，选模型后发送。需要本机 CLI 时，在运行时选择器里切换。
4. **屏幕**：用热键翻译或打开 Lens；需要的权限在系统设置里打开。

## 常见问题

<details>
<summary><strong>需要注册账号吗？数据存在哪？</strong></summary>

不需要账号。Key 写在本机 `settings.json` 里，对话、知识库、笔记都在应用数据目录：

- Windows：`%APPDATA%\com.zmair.kivio`
- macOS：`~/Library/Application Support/com.zmair.kivio`

无遥测，请求只发往你配置的供应商。

</details>

<details>
<summary><strong>支持哪些外部 CLI？</strong></summary>

本机已安装即可作为对话后端：Claude Code、Codex、Cursor、OpenCode、Gemini、Kimi、Pi、Hermes、Grok、DeepSeek Harness。可用性按本机探测；模型列表按所选 CLI 懒加载。

</details>

<details>
<summary><strong>macOS 提示已损坏 / 无法打开？</strong></summary>

安装包未签名。请右键 → 打开，或执行 `xattr -cr "/Applications/Kivio Desktop.app"`。截图与替换翻译还需要屏幕录制权限。

</details>

<details>
<summary><strong>导入的 CLI 对话为什么不能换模型续聊？</strong></summary>

导入只做显示快照，历史真身仍在原 CLI。续聊走该 CLI 的原生会话，并钉在原来的工作目录。详见 [ADR-0001](docs/adr/0001-imported-cli-conversations-stay-on-their-cli.md)、[ADR-0002](docs/adr/0002-imported-history-is-a-snapshot.md)。

</details>

## 开发

<details>
<summary><strong>环境与命令</strong></summary>

需要 Node.js、npm、Rust，以及 Tauri 2 的平台依赖。macOS 上 OCR sidecar 还需要 Swift。

```bash
npm install
npm run dev          # Rust + UI（macOS 会编 Swift sidecar）
npm run dev:ui       # 只开 Vite
npm run lint
npm run typecheck
npm test
```

Rust 测试：

```bash
# macOS / Linux
cargo test --manifest-path src-tauri/Cargo.toml

# Windows（请用脚本；直接 cargo test 的测试二进制可能起不来）
powershell -File scripts/win-cargo-test.ps1
```

协议类型由 Rust 生成，改协议后需要：

```bash
npm run protocol:generate
npm run protocol:check
```

</details>

<details>
<summary><strong>架构总览</strong></summary>

```
┌─────────────────────────────────────────────────────────────┐
│                 前端（React 18 + Vite + Tailwind）            │
│   同一套 bundle，四个窗口：main / chat / lens / translate     │
└────────────────────────┬────────────────────────────────────┘
                         │ Tauri IPC · chat-protocol
┌────────────────────────▼────────────────────────────────────┐
│                  后端（Tauri 2 + Rust）                       │
│   AgentHost 循环（prepare → planning → rounds → synthesis）  │
│     ├─ GUI 聊天                                              │
│     ├─ 外部 CLI 代理                                         │
│     └─ 子代理                                                │
└─────────────────────────────────────────────────────────────┘
```

- 实时聊天走版本化 `chat-protocol`（`npm run protocol:check` 校验生成物）
- 供应商适配器：OpenAI Chat、Anthropic Messages、Gemini、OpenAI Responses（含 xAI Grok）
- 设置存在 `settings.json`（含 API Key），对话在 `conversations/`，崩溃草稿走 JSONL journal

更细的模块地图见 [CLAUDE.md](CLAUDE.md)。

</details>

## 贡献

欢迎 [Issues](https://github.com/ZMGID/kivio/issues) 与 PR。提交前请确保：

- `npm run lint`
- `npm run typecheck`
- `npm test`

较大的功能请先开 issue 讨论。议题约定见 [docs/agents/issue-tracker.md](docs/agents/issue-tracker.md)。

## Star History

[![Star History Chart](https://api.star-history.com/svg?repos=ZMGID/kivio&type=Date)](https://www.star-history.com/#ZMGID/kivio&Date)

## License

[GPL-3.0-or-later](LICENSE) © ZM
