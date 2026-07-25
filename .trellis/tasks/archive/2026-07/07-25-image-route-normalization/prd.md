# 出图接口路由规范化

## Goal

把「聊天/工具出图走哪个接口」从**靠 base_url + 模型名子串猜**改为**归一化 + 单一解析器 + 猜错自愈**,消除「同名模型换名字/大小写/前缀就路由错」「同模型不同代理挂不同端点猜不出」「override 精确匹配静默失效」三类脆弱。

## Background

现状端点判定散在 6+ 函数、3 张硬编码子串表(`uses_openrouter_chat_image_generation` / `uses_xai_images_api` / `uses_gpt_image_api_model` / `openrouter_modalities` / `image_generation_model_name_heuristic` / `is_image_output_model`),无单一事实源。实测 grok-imagine-image 在某代理只支持 `/v1/images/generations`,名字却被猜到 chat/completions。`model_overrides.get(model)` 精确大小写匹配,与内置数据库模糊匹配不一致。

调研(LiteLLM/OpenRouter/网关):正确做法是配置/归一化 + 猜错自动桥接,而非更聪明地猜名字。**用户决定:不加新配置字段**,走归一化 + 单一解析器 + 自愈。

## Requirements

### R1 单一端点解析器
- 新增内部枚举 `ImageRoute { GeminiNative, Chat, ImagesApi }`(仅运行时,不持久化)与 `resolve_image_route(provider, model) -> ImageRoute`。
- 收敛顶层分流 + 所有 `uses_*` 端点判定到此一处。优先级:① `api_format==Gemini`→GeminiNative;② base_url 含 `openrouter.ai`→Chat;③ 归一化名字启发式(3 张子串表合并为一张)→ Chat/ImagesApi。
- `generate_with_images_api` body 变体(xai b64_json / gpt-image size+background)保留,但读同一套归一化信号。

### R2 模型名归一化
- 新增 `normalize_model_name`(小写 + 去 `models/` 前缀 + trim)。
- 用于:读 `model_overrides` 判定生图能力(修静默失效)、R1 名字启发式、`is_image_output_model`。

### R3 猜错自愈 + 会话记忆
- `generate_image_with_provider`:选定 route 调用后若返回**端点错配**错误(错误体含 `only supported on /v1/images/(generations|edits)` / `/chat/completions` / `must be used with ... endpoint`,或 4xx + 这些短语),自动换到另一端点(Chat↔ImagesApi)重试一次;成功则记 `(provider_id, normalized_model)→route` 到会话内存缓存,下次同模型先读缓存。
- GeminiNative 由 api_format 决定,不参与 chat↔images 摆动。

## Acceptance Criteria

- [ ] `resolve_image_route` 单一函数决定端点;旧 `uses_*` 端点判定收敛,无重复子串表。
- [ ] 名字归一化:`Gemini-3.1-Flash-Image` / `models/gemini-3.1-flash-image` 与裸名路由结果一致;override 生图开关按归一化名生效。
- [ ] 自愈:模拟端点错配错误 → 自动换端点重试;缓存命中二次调用直达正确端点。
- [ ] `cargo test --lib` 全绿(对照 baseline);既有 grok/gemini 用例迁移到新解析器仍绿。
- [ ] 手动:gemini-3.1-flash-image(chat)、grok-imagine-image(images_api)出图正常;改名/加前缀仍正确路由。
- [ ] OpenAI/Anthropic/Responses 适配器零行为变化;不新增持久化字段。

## Non-Goals

- 新增每模型「出图接口」配置字段 / UI(用户明确不加)。
- 改动 agent loop 内联出图(openai.rs message.images / gemini.rs inlineData → artifacts)的解析契约。
- images/edits 图生图、Vertex AI 鉴权、OpenRouter 新版 /images 专用 API。
