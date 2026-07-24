# Design — Chat 内置/第三方联网搜索

## First principles

- 基本事实 1:第三方搜索已作为**客户端工具** `search_web` 跑通,与任意模型无关。
- 基本事实 2:内置搜索是**各家服务端特性**,wire 形态与引用位置各不同,且并非所有模型支持。
- 基本事实 3:控件是会话级三态 off/builtin/third_party(OQ3)。
- 结论:第三方几乎零改;工作集中在(a)内置注入 (b)引用解析渲染 (c)三态控件+能力判断+工具门控。

## 架构与边界

四块改动,层次清晰:

### 1. 状态:会话级 `web_search_mode`

- 新增枚举 `WebSearchMode { Off, Builtin, ThirdParty }`(serde `snake_case`),放 `chat/types.rs`。
- `Conversation`(`chat/types.rs:419`)加字段 `web_search_mode: Option<WebSearchMode>`。
  - **用 `Option` 保后向兼容**:`None` = 老数据/从未设置 → 运行时回退全局
    `nativeTools.webSearch`(`settings.rs:145`):on ⇒ 视作 `ThirdParty`,off ⇒ `Off`。
  - 有效模式解析集中到一个 helper `effective_web_search_mode(conv, settings) -> WebSearchMode`。
- 前端 `Conversation` 类型同步加字段;命令层已有会话字段读写路径(照 `thinking_level`/
  `force_knowledge_search`)。

### 2. 能力判断(OQ2 置灰依赖)

- helper `builtin_web_search_supported(provider: &ModelProvider) -> bool`,放 `model_metadata.rs`。
  - 判据 = `provider.api_format`:`"openai_responses" | "gemini" | "anthropic"` ⇒ true;
    其余(含 Chat Completions `"openai"`,gpt-5 在其上会 400)⇒ false。
- 前端已有 provider 列表(含 `apiFormat`),据此把「内置」选项置灰 + 提示"当前模型不支持"。
- 后端防御:若 `effective_mode==Builtin` 但 `!supported`,当作 `Off` 处理(不注入),不硬报错。

### 3. 内置注入(backend)

- `GenerateOptions`(`model/types.rs:138`)加 `builtin_web_search: bool`(`#[serde(default)]` false)。
- 透传路径**照 `thinking_level`**:`AgentRunConfig`(`agent/types.rs`)加 `web_search_mode` →
  主回合请求构建器 `stream_scoped_chat_completion_inner`(`planning.rs:587`)+ synthesis 各点,
  当有效模式==Builtin 且模型支持时置 `options.builtin_web_search=true`。
  - 压缩/摘要调用不开(与 thinking_level 一致,`compaction`/无头路径保持 false)。
- 各适配器 `request_body` 注入(仅在 flag=true 时,append 到 tools 数组,与函数工具并存):
  | 适配器 | 注入 |
  |---|---|
  | `responses.rs:326` | `{"type":"web_search"}` |
  | `gemini.rs:294` | tools 数组追加 `{"google_search":{}}`(与 `functionDeclarations` 对象并列) |
  | `anthropic.rs` request_body | `{"type":"web_search_20250305","name":"web_search"}` |
  | `openai.rs`(Chat Completions) | 不支持,no-op |

### 4. 工具门控(`search_web`)按模式

- 现状:`search_web` 由 `native.web_search && web_search_configured` 决定(`native_registry.rs:125`,
  装配于 `registry.rs:191`)。
- 改为按有效模式在 `AgentRunConfig.tools` 层面收敛(`commands/reply.rs:542` 附近):
  - `ThirdParty` ⇒ 暴露 `search_web`(仍需 `web_search_configured`,无 key 则空搜索/提示)。
  - `Builtin` ⇒ **不**暴露 `search_web`(避免和内置重复),改由 body 注入。
  - `Off` ⇒ 两者都不给。
- ponytail:门控在 Config 组装处做一次过滤即可,不动 `native_registry` 的全局 gate 语义。

### 5. 引用渲染(MVP = 来源脚注)

- 新增 `WebCitation { title, url }`(优先复用 KB 现有 source-hit 结构,若形状不合再新建)。
- 各适配器从响应解析原生引用 → `Vec<WebCitation>`,挂到 `GenerateOutput`(加字段)并在
  finish 时经一个 `StreamPart`/既有事件带到前端。解析点(impl 时按真实 wire 校准):
  - OpenAI Responses:输出 message content 的 `annotations[type=url_citation]`(+`web_search_call` 项)。
  - Gemini:`candidates[].groundingMetadata.groundingChunks[].web.{uri,title}`。
  - Anthropic:`web_search_tool_result` block 内结果 + 正文引用。
- 前端:助手消息下方渲染「来源」列表(标题+链接,可点),风格对齐第三方来源卡/`SourcesButton`。
- **ponytail 简化**:MVP 不做正文内 `[n]` 上标锚定(url_citation 的 start/end 索引映射后置);
  只做来源列表。`// ponytail: sources footer only; inline [n] anchoring if users ask`。

## 数据流

```
InputBar/SourcesButton 三态控件
  → 写 conversation.web_search_mode (会话级持久化)
  → commands/reply.rs 组装 AgentRunConfig:
       effective_mode = effective_web_search_mode(conv, settings)
       supported = builtin_web_search_supported(provider)
       · Builtin&supported → config.web_search_mode=Builtin, 过滤掉 search_web
       · ThirdParty        → 保留 search_web
       · Off/不支持        → 两者皆无
  → planning/synthesis 构建请求: options.builtin_web_search = (mode==Builtin)
  → 适配器 request_body 注入原生工具 (仅支持的家)
  → 响应解析 citations → GenerateOutput → StreamPart → chat 事件
  → 前端来源脚注渲染
```

## 兼容 / 回退

- 老对话无 `web_search_mode`(None)→ 回退全局开关,行为与现状**逐字节一致**(第三方按全局 on/off)。
- 全局 `nativeTools.webSearch` 保留(作为 None 时的默认来源);不删,避免破坏其他读取点。
- 不支持内置的模型:前端置灰 + 后端防御降级为不注入。
- 不动 Lens 联网搜索、不动 `web_search.rs`、不做 Deep Research。

## 主要权衡 / 风险

- **最大风险 = 流式下三家引用 wire 形状**。MVP 只取 {title,url} 列表,降低对精确锚定的依赖;
  解析失败时静默降级为"无来源"(答案照常),不阻断。
- Gemini `google_search` 与 `functionDeclarations` 能否同请求并存随模型版本有差异;若并存报错,
  design 退路:Builtin 模式下 Gemini 只挂 `google_search`、暂不挂函数工具(记 ponytail 注释)。
- Anthropic 需组织 Console 开启 web search,客户端无法预判 → 前端不因此置灰,发送后如报错照常透出。

## 测试

- Rust:`responses.rs`/`gemini.rs`/`anthropic.rs` 各加一个 request_body 注入单测(flag on ⇒ tools 含原生项;
  off ⇒ 不含);引用解析各一个 fixture 解析单测。能力判断 helper 一个表驱动单测。
- 前端:SourcesButton 三态渲染/置灰的最小组件测试(若既有测试框架覆盖)。
- 回归:`cargo test`(对齐 --lib 既有基线)、`npm test`/`lint`/`typecheck` 保持绿。
