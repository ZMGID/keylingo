# Implement — Chat 内置/第三方联网搜索

分层实现,每步可独立验证。顺序:先打通 GPT(端到端骨架)→ 复制到 Gemini/Anthropic → 前端控件 → 引用渲染。

## 阶段 A:状态与骨架(会话级模式 + 能力判断)

- [ ] A1 `chat/types.rs`:加 `WebSearchMode` 枚举(serde snake_case)+ `Conversation.web_search_mode: Option<WebSearchMode>`。
- [ ] A2 helper `effective_web_search_mode(conv, settings) -> WebSearchMode`(None → 依全局 `nativeTools.webSearch` 回退)。放 `chat/` 合适模块。
- [ ] A3 helper `builtin_web_search_supported(provider) -> bool`(`model_metadata.rs`,按 `api_format`)。表驱动单测。
- [ ] A4 `GenerateOptions` 加 `builtin_web_search: bool`(serde default false);`AgentRunConfig` 加 `web_search_mode`。
- 验证:`cargo build`;A3 单测过。

## 阶段 B:GPT 内置端到端(先跑通一条链)

- [ ] B1 `commands/reply.rs:542` 组装 Config:算 effective_mode + supported,按 design §4 决定 search_web 去留 + 设 `config.web_search_mode`。
- [ ] B2 `planning.rs:587` / synthesis 请求构建:`options.builtin_web_search = (mode==Builtin && supported)`。
- [ ] B3 `responses.rs:326` request_body:flag on ⇒ 往 `body["tools"]` append `{"type":"web_search"}`(数组不存在则建)。单测。
- [ ] B4 手动冒烟:GPT(Responses)模型 + Builtin 模式,问一个需实时信息的问题,确认联网作答。
- 验证:B3 单测过;B4 冒烟通过。

## 阶段 C:Gemini + Anthropic 内置

- [ ] C1 `gemini.rs:294`:flag on ⇒ tools 数组追加 `{"google_search":{}}`。注意与 functionDeclarations 并存(不行则走 design 退路)。单测。
- [ ] C2 `anthropic.rs` request_body:flag on ⇒ append `{"type":"web_search_20250305","name":"web_search"}`。单测。
- 验证:各单测过;两家各一次冒烟。

## 阶段 D:引用解析 + 渲染

- [ ] D1 定义 `WebCitation`(优先复用 KB source-hit 结构);`GenerateOutput` 加 citations 字段 + finish 时 StreamPart/事件带出。
- [ ] D2 三家解析:OpenAI `url_citation` 注解 / Gemini `groundingChunks[].web` / Anthropic `web_search_tool_result`。各 fixture 单测。
- [ ] D3 前端:助手消息下「来源」脚注渲染(对齐来源卡样式)。解析失败静默降级。
- 验证:D2 fixture 单测过;前端能看到来源列表可点。

## 阶段 E:前端三态控件

- [ ] E1 前端 `Conversation` 类型 + api 加 `webSearchMode`;读写命令接通。
- [ ] E2 `SourcesButton.tsx:206`:把二元「网络搜索」改为三态(关闭/内置/第三方);内置项按 `builtin_web_search_supported` 置灰+提示。
- [ ] E3 `InputBar.tsx` 透传新回调;切换即时持久化到会话。
- 验证:三态切换即时生效并持久化;不支持模型下内置置灰。

## 阶段 F:收尾

- [ ] F1 全量测试:`cargo test`(对齐 --lib 既有基线,勿把预存 14 例算回归)、`npm test`、`npm run lint`、`npm run typecheck`。
- [ ] F2 三态 × 三家(支持的)矩阵手动冒烟一遍,核对 AC1–AC7。
- [ ] F3 更新 CLAUDE.md 相关段(chat 工具/联网搜索)。

## 校验命令

- `cargo test --manifest-path src-tauri/Cargo.toml`(Windows 用 `scripts/win-cargo-test.ps1`)
- `npm test` / `npm run lint` / `npm run typecheck`

## 风险 / 回滚点

- 高风险文件:三个 `model/*.rs` 适配器(动 request_body + 响应解析)。每家改动独立,可单家回滚。
- 引用解析是最脆环节 → 全程"解析失败即降级为无来源",绝不阻断答案。
- 工具门控改动(reply.rs)影响所有会话 → A/B 阶段务必回归"老对话 None 回退全局"逐字节一致。
