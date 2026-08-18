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

Kivio Desktop 常驻托盘 / 菜单栏，工作在整个**屏幕**层面，而不只是自己的窗口里。在任何地方按下热键：翻译你输入的、翻译你选中的、翻译你看到的，或者框选屏幕任意区域直接向 AI 提问。从托盘打开 AI 客户端，则是一个完整的 Agent 聊天应用：工具调用、子代理、Skills、MCP、知识库、Python 沙箱、多模型并排回答。

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
| Python | `run_python` —— 离线 Pyodide 沙箱，随包内置 numpy、pandas、matplotlib、pillow、micropip |
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

1. **[下载最新版](https://github.com/ZMGID/kivio/releases/latest)** —— macOS：Apple Silicon `.dmg` · Windows：NSIS `-setup.exe`。
2. **安装并启动。** DMG 未签名，首次打开请右键 → 打开，或执行：
   ```bash
   xattr -cr "/Applications/Kivio Desktop.app"
   ```
   macOS 会请求**辅助功能**（热键、选中取词、粘回）与**屏幕录制**（截图）权限；屏幕捕获基于 ScreenCaptureKit。Windows 手动启动时默认打开 AI 客户端。
3. **跟随首次引导** —— 添加服务商，可选配置联网搜索，确认快捷键。
4. **开始用。** 托盘 → 打开 AI 客户端做聊天、工具与文档；或在任意界面按热键使用翻译和 Lens。

Kivio Desktop 启动后会检查 GitHub Releases 的新版本（可关闭），并支持应用内直接下载安装更新。

## 新版本 —— v2.9.1

- **DeepSeek Harness** —— 补齐图片、附件路径、停任务（含子代理）和斜杠发现；模型图片与推理档写回已有路由；第三方供应商握手不再空转。空白机器可一键安装并填官方密钥。凭据按官方分层识别环境变量、凭据文件和 `.env`，第三方路由用各自的密钥环境变量，不再误拦。
- **外部 CLI 系统提示** —— 对话所属集的系统提示会注入外部 CLI，切集不再丢掉那一层人设。
- **供应商模型列表** —— 拉模型按协议鉴权，并解析 Gemini 原生 ListModels。
- **启动体验** —— 启动后可最小化到托盘；开机自启不再强行弹出聊天窗。
- **聊天** —— 工作台路径挪到 tools 之后，避免跨会话打穿前缀缓存。

## v2.9.0

- **DeepSeek Harness** —— 新增 dsh 作为外部 CLI 代理：官方供应商、四档 Agent 模式、插件页、斜杠命令（已知命令高亮）、工具卡、压缩，以及 `/compact` `/goal` `/feedback`。重启或改启动配置后仍续上原生会话；顶栏显示实际模型与思考档。后台子代理显示实时进度，完成后把汇报写回父对话，和官方 web 一致。用量条不再把已排除的缓存输入减第二次。
- **外部 CLI 更稳** —— 每个 CLI 记住上次的模型和思考档，切走再切回不再重置成 Auto。轮内重连保留原生会话，不再开空白对话。Grok 上游 503/429 的静默重试会显示状态。发送失败回填草稿。找不到原生会话时按 ACP 口径降级，不再硬失败。生成中禁止删消息，避免误回收附件。
- **Todo、问用户与工具卡** —— dsh / Claude Code 的 Todo 接到对话列表；Claude Task 卡立刻显示任务。外部 CLI 问用户收成统一卡片，下一条 CLI 只需加 codec。dsh 官方工具名、压缩和 job 事件走现成卡片与分隔线，不再当成一句 prompt。
- **Pi** —— 内置 `/compact` 走原生 compact，不再当 prompt 发给模型。导入尊重 `PI_CODING_AGENT_DIR` 和共享 MCP / 技能层；原生会话 id 更短。
- **Kivio Chat** —— 不再继承 Agent 人设和 shell 长文；发送路径挂上知识库；自定义 / 助手文本不能改写 Chat 契约；dock、技能和 `/plan` 在 Chat 里保持隐藏。
- **笔记** —— 笔记页可打开库文件夹，拖入外部 markdown 自动出现；目录变化自动刷新，编辑时不会把自己的保存冲掉列表。
- **新模型** —— 补上 Grok 4.6（500k 上下文），并把 grok CLI 的默认与回退模型从 4.5 抬到 4.6。
- **会话存储** —— 会话 JSON 不再存图片 base64，改为内容寻址外置，同一张图看多次只留一个文件；打开即迁移存量大文件。启动回收空工作区和已删会话的孤儿附件，非空工作区与用户上传不自动删。含图会话不再把磁盘和打开时的内存打满。
- **聊天体验** —— 重复点击已打开的会话不再重载；带图会话打开时等 Mermaid 落地再对齐；长文本不再全量哈希。生成中也能钉住。贴底打字不再整屏抖；暗色切会话不闪白。标题打字机失效已修。设置里添加 CLI 模型不再一闪即消。工具轮次默认不限（旧默认 20 会迁一次）。

## v2.8.9

- **长对话性能大修** —— 消息列表重做虚拟化与滚动跟随：屏外消息卸载、流式行独立渲染、重内容延迟挂载。长对话打开更快，流式生成与回翻历史更流畅，修复多种滚动跳动与生成结束时的闪动。
- **Kivio Chat 独立运行时** —— Kivio Chat 成为独立运行时，与 Agent 模式分开配置提示词。
- **新模型** —— 新增 Claude Opus 5 与最新 Gemini Flash；外部 CLI 新增 Kimi CLI 供应商配置。
- **发送与审批体验** —— 发送等待期间保留输入草稿与附件，消息真正进入发送流程后才清空；审批卡提交中禁用按钮并显示失败原因，可直接重试。
- **对话库与搜索** —— 全局搜索显示匹配片段并高亮跳转；修复从对话库打开对话的顺序问题；上下文用量面板收纳为三组。
- **界面修复** —— 恢复细胶囊滚动条并保持聊天滚动条可见，侧栏列表滚动时自动隐藏；统一设置面板头部与背景；CLI 供应商弹窗改为卡片分区。
- **安全与稳定** —— HTML 预览 iframe 沙箱化；Lens 覆盖层失活时后端强制关闭兜底；自动保存的设置在关闭窗口时同步到聊天视图。

## v2.8.8

- **对话库** —— 新增对话管理中心：搜索、书架分类、排序分组、多选批量操作、归档。插件管理改到设置页。
- **提示词缓存时长** —— 供应商可选择关闭、短时或长时缓存。
- **扩展中心中英文** —— 助手、技能、MCP、知识库、笔记、插件页面支持界面语言切换。
- **会话用量** —— 输入栏显示本会话的输入、缓存命中与输出 token 用量。
- **标题动画** —— 模型生成对话标题时，以打字效果替换临时标题。
- **Cursor Composer** —— 支持 Composer 1 / 1.5 / 2 / 2.5 等模型的上下文窗口与定价显示。
- **侧栏操作** —— 对话可钉选、一键归档；完整菜单改到右键；生成中显示波浪状态指示。
- **设置与用量统计** —— 调整设置导航顺序与记忆页布局；用量统计新增请求成功率。
- **问题修复** —— 修复流式中断后内容丢失、重复消息、滚动异常、批量删除残留，以及对话库切换闪白。

## v2.8.7

- **外部 CLI Agent 能力补齐** —— Claude Code 接入问用户、计划批准、后台任务和子代理实时进度，子代理调用独立显示为卡片；内置 Agent 改进读文件输出上限、上下文超窗压缩、空响应判定与用量统计。
- **Pi 与 pi-btw 适配** —— Pi 原生供应商配置接入统一外部 CLI 体系，pi-btw 事件映射进共享运行协议；流式中途断开时保留重试能力，并将上游断流与普通进程退出分开呈现。
- **长文本粘贴附件** —— 超长粘贴内容转换为可查看、编辑的内存文本附件，并完整覆盖持久化、重新生成、外部 CLI、导出和 steering，不再静默丢失正文。
- **聊天交互与渲染修复** —— 修复 `<details>` 标签原样显示、中文相邻粗体不渲染、问用户答案折叠、图片右键菜单被 WebView 吞掉，以及窗口 resize 竞态解除流式滚动跟随。
- **状态动效和暗色模式** —— 消息流末尾新增常驻运行状态行，后台任务计数与侧栏运行指示更准确；修复部分设备尤其暗色模式下的状态动画异常，并降低 WKWebView 悬停显隐引发的重绘成本。
- **记忆指令过滤** —— 收紧并修正记忆系统对命令字眼的安全匹配，避免正常内容被误过滤，同时清理输入区相关边界状态。
- **OpenCode、模型与用量** —— 完善 OpenCode 原生供应商配置；外部 CLI 上下文用量按轮更新，新增首 token 延迟和思考档位记录，修复模型映射与统计口径。
- **其它体验改进** —— 支持生成中消息排队与立即引导，检查更新可就地执行，发送后为新消息留出阅读空间；设置页供应商与 CLI 分栏更紧凑，并修复 Edge/WebView2 密码可见按钮重复。

## v2.8.6

- **实时协议版本化** —— 聊天的实时通信收进单条 `chat-protocol` 通道：运行事件带版本与序号，会话事件带 revision，断线可按快照重放，TypeScript 类型与 JSON Schema 由 Rust 侧生成后提交（`npm run protocol:check` 进 CI 门禁）。配套修掉会话持久化的并发竞争、侧栏刷新的全量扫盘与独占锁、一个订阅者抛错毒死整条实时流、空 delta 的段占位事件被吞。
- **从本地 CLI 导入对话** —— 绑定了文件夹的项目新增「从 CLI 导入对话」：列出工作目录等于项目根的原生会话，导入成 Kivio 对话，续聊仍由原 CLI 承担，支持 claude / codex / grok / kimi / opencode。导入的对话钉死在原 CLI 与原工作目录，历史是一次性快照、不与 CLI 同步（附过期提示）；claude 的项目目录编码有损，改为读 jsonl 里的明文 cwd。
- **每个供应商单独的请求配置** —— 供应商详情「测试连接」下方新增「请求配置 ›」：自定义请求头（行内编辑 + 校验 + 从 JSON / cURL 粘贴导入，中转站要的 X-Title / HTTP-Referer 之类终于有地方填）、跟随系统代理开关（关掉走直连）、prompt caching、按 Claude Code / Codex / Grok 注入整套客户端身份头。prompt 缓存不再只有 Anthropic，OpenAI 协议一并覆盖；测试连接读的是**编辑中**的配置，不再出现「测试通过、聊天 403」。
- **侧栏可手排，对话可钉位** —— 集与项目改为拖拽排序，索引里的数组顺序就是显示顺序（此前项目每次按 `updated_at` 倒序重排且不落盘，而那个字段只在显式改名/改色时写，作为「最近使用」是假信号）。展开后的对话也能拖：时间序仍是底座，拖过的钉在放下的那一行，其余按更新时间填空位。拖拽是插入线式、按每行真实位置命中，因此不要求行高相等；「最近」平铺列表保持纯时间线。
- **Grok (xAI) 独立为一种接口协议** —— xAI 的 Responses 端点严格拒收一批 OpenAI 专有字段（instructions / store / prompt_cache_key …），思考档位也是自己一套。新增 `xai_responses` 协议后不再每个会话白扔一次 400；**按用户选的协议分叉，不按 base_url 猜**（中转站可以把 grok 挂在任意域名上）。系统提示词写入 `input[0]` 的 `role:system` 项，`store:false` 显式下发 —— xAI 默认把响应存 30 天，而 Kivio 从不用 `previous_response_id`。
- **修一批流式、删除与渲染缺陷** —— 断流回落非流式的总超时 60s → 600s（high reasoning + 十万 token 输入根本不可能在 60 秒内答完，三次重试白烧 195 秒必然失败）；同一张图被 read 两次不再在历史里留两份 base64（实测占请求体 74%，而 token 估算故意不计图片、压缩层永远看不见）；新增 Timeout 失败分类并带上剥壳后的供应商原始报错；流式表格不再被贪婪正则四行只切一刀退化成段落；删除对话改为先摘文件与索引条目，Windows 上 dev server 钉住工作区不再让整个删除中止、刷新后对话又冒回来。
- **重会话切换从约 1 秒降到接近瞬时** —— 渲染策略的判据从「消息条数」换成成本估算：实测一个 14 条消息的对话因 7 条回答里塞了 231 个代码块，渲出 5433 个 DOM 节点，条数少不代表便宜。重会话走向上渐进加载而不是虚拟化 —— 本应用行高差三个数量级（提问 6~21px，回答 6885~11992px），virtua 只接受一个标量 itemSize，按消息粒度虚拟化结构性地做不到不跳。另外代码块外壳瘦身（语言标签与复制图标改走伪元素）、高亮结果加 LRU 缓存。
- **底部跟随不再抽搐或卡死** —— 滚动跟随改为按事件来源判定：拖原生滚动条、页内查找、focus 滚动、iframe 滚到头后的链式滚动都产生不了 wheel，此前这些情况下 following 恒为 true，又在 gap>32 时钉回底部，和外部反复互写 scrollTop —— 表现是贴底时整个列表抽搐、滚动条拖不动。
- **聊天界面接上语言开关** —— 聊天窗从来没做过 i18n（687 个键里只有 dock 的 78 个在用），设置切到 EN 而聊天仍是中文。常驻外壳搬进字符串表：侧栏、标题栏与窗口按钮、输入栏、顶栏各选择器、四个右键菜单、后台命令指示器、轮次导航条，共 145 个新键、中英双侧，取文案走新增的 `LangContext` 而不是给十九个组件各加一个 prop。
- **退出不再漏孤儿子进程** —— 退出清理里 `tokio::time::timeout(..)` 当参数传给 `block_on`，参数在进入运行时**之前**求值，而 `Sleep` 构造就要求时间驱动在场 —— 每次退出 panic「there is no reactor running」、退出码 101，且 panic 让它之后的整段清理全部不执行：外部 CLI 会话、后台命令进程组、OCR sidecar、插件预览每次关窗都在漏，讽刺的是这些清理的注释都写着「不同步等就会留下孤儿」。
- **外部 CLI 模型选单与其它** —— Claude Code 不再堆别名与具体版本，只保留四档家族 catalog 并应用 settings/env 映射，Codex 以精选四模型为主；修 Claude 多轮模型映射与 Codex effort 边界。HTML 预览在生成中只渲染源码、静默 600ms 才挂 iframe，指针在预览里滚也能带动聊天列表。Off 思考档显式下发 `none`/`disabled` 而不是默认 high；设置页齐边铺满；Mica 拖窗与缩放时不再闪；最大化图标在非整数缩放下不再缺边。
- **移除项（不兼容）** —— AI 客户端设置「响应」下的「流式输出」与「思考模式」两个开关删除，行为固定为开启；此前手动关掉过的用户下次启动会被 `sanitize_settings` 改回开启，界面上不再有关掉它们的入口（结构体字段保留，serde 向下兼容）。

完整历史:[GitHub Releases](https://github.com/ZMGID/kivio/releases)。

## 开发

| 层 | 技术栈 |
|---|---|
| 后端 | Rust · Tauri v2 |
| 前端 | React 18 · TypeScript · Vite · TailwindCSS v4 |
| OCR | Apple Vision（Swift sidecar）· Windows.Media.Ocr · RapidOCR（ONNX） |
| Python 沙箱 | Pyodide，随包离线 |

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

Kivio Desktop lives in your tray / menu bar and works at the level of your *screen*, not just inside its own window. Press a hotkey anywhere to translate what you typed, translate what you selected, translate what you see, or capture any region and ask AI about it. Open the AI client from the tray and you get a full agentic chat app: tool calls, sub-agents, Skills, MCP servers, a knowledge base, a Python sandbox, and side-by-side multi-model answers.

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
| Python | `run_python` — offline Pyodide sandbox, bundled with numpy, pandas, matplotlib, pillow, micropip |
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

1. **[Download the latest release](https://github.com/ZMGID/kivio/releases/latest)** — macOS: Apple Silicon `.dmg` · Windows: NSIS `-setup.exe`.
2. **Install and launch.** The DMG is unsigned; on first launch right-click → Open, or run:
   ```bash
   xattr -cr "/Applications/Kivio Desktop.app"
   ```
   macOS will ask for **Accessibility** (hotkeys, selected-text capture, paste-back) and **Screen Recording** (capture) permissions. Screen capture uses ScreenCaptureKit. On Windows, launching manually opens the AI client.
3. **Follow the first-run wizard** — add a provider, optionally set up web search, confirm hotkeys.
4. **Go.** Tray → Open AI Client for chat, tools, and documents; or press a hotkey anywhere for translation and Lens.

Kivio Desktop checks GitHub Releases for updates shortly after launch (can be disabled) and can download and install the update in-app.

## What's New — v2.9.1

- **DeepSeek Harness** — adds images, attachment paths, stop-task (including subagents), and slash discovery; writes model image/reasoning capabilities back onto existing routes; third-party provider handshake no longer spins. A blank machine can one-click install and save the official key. Credentials follow the official layers (env, credentials file, `.env`); third-party routes use their own key env vars instead of being blocked.
- **External CLI system prompt** — a collection's system prompt is injected into the external CLI, so switching collections no longer drops that identity layer.
- **Provider model lists** — listing models authenticates per protocol and parses Gemini's native ListModels.
- **Launch** — can minimize to the tray after start; launch-at-startup no longer forces the chat window open.
- **Chat** — the workbench path moves after tools, so switching conversations no longer busts the prefix cache.

## v2.9.0

- **DeepSeek Harness** — dsh joins as an external CLI: official provider, four Agent presets, a plugins page, slash commands (known ones highlighted), tool cards, compaction, and `/compact` `/goal` `/feedback`. Restarts and launch-config changes keep the native session; the title bar shows the model and effort actually in use. Background subagents show live progress and write the report back into the parent conversation when they finish, matching official DSH web. The usage strip no longer subtracts already-excluded cached input a second time.
- **More reliable external CLIs** — each CLI remembers its last model and thinking level; switching away and back no longer resets the pills to Auto. A mid-turn reconnect keeps the native session instead of opening a blank one. Grok's silent 503/429 retries surface as status notes. A failed send restores the draft. A missing native session degrades the ACP way instead of hard-failing. Messages cannot be deleted while generating, so attachments are not garbage-collected by mistake.
- **Todos, ask-user, and tool cards** — dsh / Claude Code todos land on the conversation list; Claude Task cards show the list immediately. Ask-user from external CLIs uses one shared card — the next CLI only needs a codec. Official dsh tool names, compaction, and job events reuse the existing cards and dividers instead of being treated as a prompt.
- **Pi** — built-in `/compact` uses native compact instead of sending a prompt. Import honors `PI_CODING_AGENT_DIR` and the shared MCP / skill layers; native session ids are shorter.
- **Kivio Chat** — no longer inherits Agent identity or the shell essay; the send path mounts the knowledge base; custom / assistant text cannot override the Chat contract; the dock, skills, and `/plan` stay hidden in Chat.
- **Notes** — the notes page can open the library folder; dropping in an external markdown file makes it appear automatically. Directory changes refresh the list; saving while editing does not yank the list back.
- **New model** — adds Grok 4.6 (500k context) and lifts the grok CLI default and fallback list from 4.5 to 4.6.
- **Conversation storage** — conversation JSON no longer stores image base64; images are content-addressed on disk, so reading the same file N times keeps one copy. Opening a conversation migrates existing oversized files. Startup reclaims empty workspaces and orphan attachments from deleted conversations; non-empty workspaces and user uploads are never auto-deleted. Image-heavy conversations no longer blow disk or memory on open.
- **Chat polish** — clicking an already-open conversation no longer reloads it; opening a conversation with diagrams waits for Mermaid before aligning; long text is no longer fully hashed. Pinning works while generating. Typing at the bottom no longer shakes the screen; switching conversations in dark mode no longer flashes white. The title typewriter works again. Adding a CLI model in settings no longer flashes away. Tool rounds default to unlimited (the old default of 20 migrates once).

## v2.8.9

- **Long-conversation performance overhaul** — the message list rebuilds virtualization and scroll follow: off-screen messages unmount, the streaming row renders independently, and heavy content mounts lazily. Long conversations open faster, streaming and scrolling back through history are smoother, and several scroll jumps and end-of-run flickers are fixed.
- **Kivio Chat as a separate runtime** — Kivio Chat becomes its own runtime, configured apart from Agent mode with its own prompt.
- **New models** — adds Claude Opus 5 and the latest Gemini Flash; external CLIs gain a Kimi CLI provider config.
- **Send and approval experience** — the composer keeps your draft and attachments until the message actually enters the send pipeline; approval cards disable their buttons while submitting and surface failures for retry.
- **Conversation library and search** — global search shows match snippets with highlight jump; fixes open order from the library; the context-usage panel collapses into three groups.
- **UI fixes** — restores thin capsule scrollbars and keeps the chat scrollbar visible, with the sidebar list auto-hiding on scroll; unifies the settings panel header and background; the CLI provider modal becomes card sections.
- **Security and stability** — sandboxes the untrusted HTML preview iframe; adds a backend force-close fallback for a dead Lens overlay webview; autosaved settings propagate to the chat view on close.

## v2.8.8

- **Conversation library** — new conversation manager with search, shelves, sort/group, bulk actions, and archive. Plugin management moves to Settings.
- **Prompt-cache duration** — set provider cache retention to off, short, or long.
- **Extension-center languages** — Assistant, Skill, MCP, Knowledge, Notes, and Plugin pages follow the app language.
- **Session usage** — the composer shows input, cache-hit, and output tokens for the current session.
- **Title animation** — generated conversation titles replace the temporary title with a typewriter effect.
- **Cursor Composer** — shows context window and pricing for Composer 1 / 1.5 / 2 / 2.5.
- **Sidebar actions** — pin and one-click archive; full menu on right-click; wave indicator while generating.
- **Settings and usage stats** — cleaner settings nav and memory layout; usage stats include request success rate.
- **Bug fixes** — lost content after stream failures, duplicate messages, scroll glitches, leftover bulk-delete state, and white flashes when switching shelves.

## v2.8.7

- **Expanded external-CLI agent support** — Claude Code now supports ask-user, plan approval, background tasks, and live subagent progress, with subagent calls rendered as dedicated cards. The built-in agent improves file-read output limits, context-window compaction, empty-response handling, and usage accounting.
- **Pi and pi-btw integration** — Pi provider configuration joins the shared external-CLI system, and pi-btw events map into the common run protocol. Mid-stream upstream failures retain retry behavior and are presented separately from ordinary process exits.
- **Long-paste attachments** — oversized pasted text becomes an editable in-memory attachment, with complete support across persistence, regeneration, external CLIs, export, and steering, so its body is no longer silently lost.
- **Chat interaction and rendering fixes** — fixes raw `<details>` tags, adjacent Chinese bold text not rendering, ask-user answers collapsing, image context menus being swallowed by the WebView, and a window-resize race that disabled streaming scroll follow.
- **Status motion and dark mode** — the message stream gains a persistent run-status row, with more accurate background-task counts and sidebar activity. Status animation issues seen on some machines, especially in dark mode, are fixed, while WebView hover redraw cost is reduced.
- **Memory-command filtering** — tightens and corrects safety matching for command-like wording so normal content is not mistakenly filtered, and cleans up related composer edge states.
- **OpenCode, models, and usage** — completes native OpenCode provider configuration; external-CLI context usage now updates per turn, records time to first token and effort level, and fixes model mapping and accounting boundaries.
- **More polish** — supports queued messages and immediate steering during generation, runs update checks in place, leaves reading space below newly sent messages, tightens the provider/CLI settings layout, and fixes duplicate password-visibility controls in Edge/WebView2.

## v2.8.6

- **Versioned realtime protocol** — chat realtime traffic moved onto a single `chat-protocol` channel: run events carry a version and a sequence number, conversation events carry a revision, a dropped connection replays from a snapshot, and the TypeScript types and JSON Schemas are generated from Rust and committed (`npm run protocol:check` gates CI). Along with it: conversation persistence is concurrency-safe, sidebar refresh no longer scans the whole disk or takes an exclusive lock, one throwing subscriber no longer poisons the realtime stream, and empty-delta segment placeholders are no longer swallowed.
- **Import conversations from a local CLI** — projects bound to a folder gain "import conversation from CLI": it lists native sessions whose working directory equals the project root and imports them as Kivio conversations, with the original CLI still driving the continuation, for claude / codex / grok / kimi / opencode. An imported conversation is pinned to its original CLI and working directory, and history is a one-time snapshot rather than a live sync (with a staleness note); claude's project-directory encoding is lossy, so the plain `cwd` inside the jsonl is read instead.
- **Per-provider request configuration** — provider detail gains a "Request config ›" page below "test connection": custom headers (inline editing plus validation, importable by pasting JSON or cURL, so relay-required headers like X-Title / HTTP-Referer finally have a home), a follow-system-proxy toggle (off means a direct connection), prompt caching, and a full client-identity header set for Claude Code / Codex / Grok. Prompt caching is no longer Anthropic-only; the OpenAI protocols are covered too. Test-connection reads the config **being edited**, so "test passes, chat 403" is gone.
- **Hand-ordered sidebar and pinned conversations** — sets and projects are drag-reordered and the array order in the index is the display order (projects used to be re-sorted by `updated_at` on every read without persisting, and that field is only written on an explicit rename or color change, making it a fake recency signal). Expanded conversations can be dragged too: time order is still the base, a dragged conversation pins to the row you dropped it on, and the rest fill the remaining slots by update time. Dragging is insertion-line style and hit-tests real row positions, so rows need not be equal height; the flat "Recent" list stays a pure timeline.
- **Grok (xAI) is its own wire protocol** — xAI's Responses endpoint strictly rejects a set of OpenAI-only fields (instructions / store / prompt_cache_key / …) and has its own effort ladder. With the new `xai_responses` protocol no conversation burns a 400 to learn that, and the fork is **keyed on the protocol you picked, never guessed from `base_url`** (a relay can host grok on any domain). The system prompt is written as a `role:system` item in `input[0]`, and `store:false` is sent explicitly — xAI keeps responses for 30 days by default and Kivio never uses `previous_response_id`.
- **A batch of streaming, deletion, and rendering fixes** — the non-streaming fallback timeout went from 60s to 600s (high reasoning over a 100k-token input cannot possibly finish in 60 seconds, so three retries burned 195 seconds to fail for certain); reading the same image twice no longer leaves two base64 copies in history (measured at 74% of the request body, and the token estimate deliberately skips images so the compaction layer never saw it); a new Timeout failure class carries the provider's unwrapped raw error; streaming tables are no longer collapsed into a paragraph by a greedy regex that cut once across four rows; and deleting a conversation now detaches the file and index entry first, so on Windows a dev server pinning the workspace no longer aborts the whole deletion and leaves the conversation to reappear on refresh.
- **Switching into a heavy conversation dropped from ~1s to near-instant** — the render strategy is chosen by estimated cost instead of message count: one 14-message conversation packed 231 code blocks into 7 answers and rendered 5433 DOM nodes, so few messages does not mean cheap. Heavy conversations use progressive upward loading rather than virtualization — row heights here span three orders of magnitude (6–21px questions, 6885–11992px answers) and virtua accepts a single scalar `itemSize`, so per-message virtualization structurally cannot avoid jumping. Code-block chrome is slimmer (language label and copy icon are pseudo-elements) and highlight results are LRU-cached.
- **Pin-to-bottom no longer twitches or locks up** — scroll follow is decided by event source: dragging the native scrollbar, in-page find, focus scrolling, and scroll chaining out of an iframe all produce no wheel event, so `following` stayed true forever and the 32px re-pin fought whoever else was writing `scrollTop` — the list twitched at the bottom and the scrollbar would not move.
- **The chat UI honors the language switch** — the chat window never had i18n (only 78 of 687 keys, all in the dock, were used), so switching settings to EN left chat in Chinese. The persistent shell moved into the string table: sidebar, titlebar and window buttons, composer, top-bar pickers, four context menus, the background-command indicator, and turn navigation — 145 new keys in both languages, read through a new `LangContext` rather than adding a prop to nineteen components.
- **No more orphaned child processes on exit** — the exit cleanup passed `tokio::time::timeout(..)` as an argument to `block_on`; arguments are evaluated **before** entering the runtime, and constructing `Sleep` requires the time driver to be present, so every exit panicked with "there is no reactor running" and exit code 101 — and the panic meant everything after that line never ran: external CLI sessions, background command process groups, the OCR sidecar, and plugin previews leaked on every window close, while each of those cleanups carried a comment saying it must be awaited or it orphans children.
- **External CLI model menus, and more** — Claude Code no longer piles up aliases and point versions, keeping four family entries and applying settings/env mapping, while Codex leads with four curated models; Claude's multi-turn model mapping and Codex effort bounds are fixed. HTML preview renders as source while generating and only mounts the iframe after 600ms of silence, and scrolling with the pointer inside the preview moves the chat list too. The Off effort level sends `none`/`disabled` explicitly instead of defaulting to high, the settings page fills edge to edge, Mica no longer flashes while dragging or resizing, and the maximize glyph no longer loses an edge at fractional display scaling.
- **Removed (breaking)** — the "streaming" and "thinking mode" toggles under AI-client Response settings are gone and both behaviors are permanently on; anyone who had turned one off will have it set back on by `sanitize_settings` at next launch, and there is no longer any UI to disable them (the struct fields remain for serde compatibility).

Full history: [GitHub Releases](https://github.com/ZMGID/kivio/releases).

## Development

| Layer | Stack |
|---|---|
| Backend | Rust · Tauri v2 |
| Frontend | React 18 · TypeScript · Vite · TailwindCSS v4 |
| OCR | Apple Vision (Swift sidecar) · Windows.Media.Ocr · RapidOCR (ONNX) |
| Python sandbox | Pyodide, bundled offline |

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
