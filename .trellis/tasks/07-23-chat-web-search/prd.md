# Chat 内置/第三方联网搜索

## Goal

让 Chat 会话可以在「关闭 / 模型内置搜索 / 第三方搜索」之间切换。第三方路径已存在,本任务
补齐**模型内置(provider-native)联网搜索**,并把 composer 的二元开关升级为三态选择。

## Background / 现状(已确认)

第三方搜索在 chat 里**已完整实现**:
- 原生工具 `search_web`(wire 名),def `native_web_search_tool`(`mcp/types.rs:232`),
  注册于 `mcp/native_registry.rs:123`,gated by `native.web_search && web_search_configured`。
- 调用体 `call_web_search`(`native_registry.rs:457`)→ `web_search::search_web`,复用
  Lens 第三方配置 `settings.lens.web_search`(Tavily/Exa/ExaMcp/Ollama/Grok)。
- 全局设置 `nativeTools.webSearch`(`settings.rs:145` `web_search: Option<bool>`)。
- UI:来源弹层 `SourcesButton.tsx:206` 的「网络搜索」二元开关(`onToggleWebSearch`)。
- 第三方返回带 provider 标签的结构化结果,已有工具卡/来源展示。

模型内置搜索**完全没接**:四个适配器(`openai.rs`/`anthropic.rs`/`gemini.rs`/`responses.rs`)
均未注入 provider-native 搜索工具。唯一用到模型内置搜索的是 Lens 的 Grok
(`web_search.rs:490`,走 xAI Responses `tools:[{type:"web_search"}]`),与 chat 无关。

各家内置搜索 wire 形态(已联网核实 2026-07):
| Provider | 声明 | 引用取处 | 端点约束 |
|---|---|---|---|
| OpenAI (`responses.rs`) | `tools:[{type:"web_search"}]` | 正文 `url_citation` 注解 + `web_search_call` 输出项 | 必须 Responses API;gpt-5 在 Chat Completions 上开会 400 |
| Gemini (`gemini.rs`) | `tools:[{google_search:{}}]` | `candidates[].groundingMetadata`(`webSearchQueries`+`groundingChunks[].web`) | 必须原生端点(compat 端点对未知字段 400) |
| Anthropic (`anthropic.rs`) | `tools:[{type:"web_search_20250305",name:"web_search",max_uses}]` | `web_search_tool_result` block + 正文引用 | 需组织 Console 开启 + 限模型 |

注入点:`responses.rs:326` `request_body` 已有 `provider_options` 合并机制,但会整包覆盖
`body["tools"]`,不能直接塞;需专门 flag,往 tools 数组 append。flag 通路:`GenerateOptions`
(`types.rs:138`)→ `stream_scoped_chat_completion_inner`(`planning.rs:587`,主回合流式)。

## Requirements

- R1 会话级三态「联网搜索」控件:关闭 / 内置 / 第三方(替换现有二元开关),持久化为
  `Conversation.web_search_mode`(会话级,老数据 None 回退全局 `nativeTools.webSearch`)。
- R2 「第三方」= 现有 `search_web` 工具行为(基本不改,仅接到新控件)。
- R3 「内置」= 在模型请求体注入 provider-native 搜索工具。**MVP 覆盖 OpenAI(Responses)+
  Gemini + Anthropic**;Chat Completions/其他不支持,Grok chat 留待后续。
- R4 「内置」返回的引用(url_citation / groundingMetadata / web_search_tool_result)解析成
  统一结构,前端渲染可点来源列表(与第三方来源卡风格一致);解析失败静默降级为无来源,不阻断答案。
- R5 当前模型不支持内置搜索时(按 `api_format` 判断):控件「内置」项**置灰 + 提示**,
  后端防御性降级为不注入。
- R6 内置搜索的 MVP 只做来源脚注,不做正文 `[n]` 内联锚定(后续增强)。

## Acceptance Criteria

- [ ] AC1 composer 来源弹层出现三态控件,切换即时生效并持久化。
- [ ] AC2 选「内置」+ GPT(Responses)模型:请求体带 `{type:"web_search"}`,模型能用实时网络信息作答。
- [ ] AC3 内置搜索的答案渲染出可点击来源(至少 GPT 的 url_citation)。
- [ ] AC4 选「第三方」:行为与现状一致(`search_web` 可被调用,来源卡正常)。
- [ ] AC5 选「关闭」:两条路径都不触发(不注入内置工具,且 `search_web` 不可用)。
- [ ] AC6 不支持内置的模型下,控件按 OQ2 决议降级,不报硬错误。
- [ ] AC7 现有 Rust/前端测试保持绿;为内置注入与引用解析补最小测试。

## Out of Scope

- Deep Research(o3-deep-research / background 异步轮询)——不做。
- Grok chat 内置搜索、Lens 侧联网搜索改动——本次不动。
- 内置搜索的高级参数(search_context_size / max_uses / 域名过滤)——MVP 用默认值。
- 正文 `[n]` 内联引用锚定——MVP 只做来源脚注。
