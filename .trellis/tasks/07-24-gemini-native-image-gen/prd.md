# Gemini 原生接口生图接入

## Goal

让 Gemini 原生 provider(`api_format = gemini`,谷歌官方 `generateContent` 接口)下的生图模型(gemini-*-image / nano-banana / imagen 系)在 Kivio 中可用,覆盖两条路径:

1. **Mixer `generate_image` 工具**:当生图模型配置指向 Gemini 原生 provider 时,走原生 `generateContent` 请求生图,不再报 "requires an OpenAI-compatible provider"。
2. **聊天内直接出图**:会话选择 Gemini 原生生图模型时,请求携带 `responseModalities: ["TEXT","IMAGE"]`,响应/流式解析 `inlineData` 图片 part,图片作为消息 artifacts 落地并在聊天 UI 中渲染。

## Background

- 现状:`image_generation.rs::validate_provider` 拒绝 `ProviderApiFormat::Gemini`;`model/gemini.rs` 的 `inlineData` 只处理用户上行图片,响应中的图片 part 被静默丢弃;请求不带 `responseModalities`。
- OpenRouter 中转的 `google/gemini-*-image` 已可用(走 OpenAI 兼容 chat 路径),本任务不改动该路径。

## Requirements

### R1 Mixer 工具走 Gemini 原生生图
- `validate_provider` 放行 `ProviderApiFormat::Gemini`。
- 新增 Gemini 原生生图请求路径:`POST {base_url}/models/{model}:generateContent`,body 含 prompt 文本 part + `generationConfig.responseModalities: ["TEXT","IMAGE"]`(可选 `imageConfig.aspectRatio`,由 size 参数映射,未知 size 不传)。
- 解析响应 `candidates[0].content.parts[*].inlineData`(`mimeType` + base64 `data`)→ `GeneratedImage`,复用现有 artifacts 组装逻辑。
- 多图:`n > 1` 用 `generationConfig.candidateCount` 或循环调用(以实测/文档为准,循环兜底)。
- key 复用 `send_with_failover`,与现有 Gemini 聊天适配器同款鉴权方式(`x-goog-api-key` header)。

### R2 聊天内直接出图(Gemini 原生适配器)
- 仅当模型判定为生图模型(`model_metadata` 已有 image 模型判定或按名称包含 `image`/`imagen`)时,`gemini.rs` 请求体带 `responseModalities: ["TEXT","IMAGE"]`;普通文本模型请求体**字节不变**(回归红线:Gemini 对未知字段 400)。
- 非流式与流式解析均处理 `inlineData` part:图片进入 `GenerateOutput.images`(新增字段,`Vec<GeneratedImageData>{ mime_type, base64 }`,默认空,serde default,其它适配器不受影响)。
- 流式:图片 part 到达时通过新增 `StreamPart::ImageData { mime_type, data }` 发射;所有既有 sink 用兜底分支忽略之,仅 `AgentStreamSink` 消费。
- loop 侧:assistant 消息把 images 转成 `ChatToolArtifact`(`data_url`)挂到 `ChatMessage.artifacts`,前端沿用现有 artifacts 渲染(MessageBubble 已支持)。
- 工具调用回放:assistant 历史消息中的图片 part 不回放(与现有思维文本处理一致,丢弃可接受)。

### R3 兼容与红线
- `chat-stream`/`chat-tool` 事件 payload 保持向后兼容(新增字段可选)。
- OpenAI/Anthropic/Responses 适配器零行为变化。
- Gemini 文本模型请求体与现有字节一致(有测试锁定)。
- `model/README.md` 契约补充 images 语义。

## Acceptance Criteria

- [ ] `validate_provider` 接受 Gemini provider;Mixer generate_image 配 Gemini 原生 provider + gemini-image 模型可出图(单测:请求体形状 + 响应解析)。
- [ ] `gemini.rs` 生图模型请求含 `responseModalities`;文本模型请求体不变(单测对比)。
- [ ] 非流式 + 流式响应中的 `inlineData` 都被解析为图片(单测:mock 响应)。
- [ ] 聊天消息 artifacts 含生成图片,`cargo test` 全绿(对照 Windows 基线),`npm test`/`lint`/`typecheck` 全绿。
- [ ] `StreamPart::ImageData` 新变体不破坏 Lens 等其它 sink(编译期兜底分支验证)。
- [ ] 手动验收:真实 Gemini key 聊天内出图 + Mixer 工具出图各一次(由用户执行)。

## Non-Goals

- OpenRouter 路径改动;Imagen 专用 `:predict` 接口(仅 generateContent 系);图片编辑(上行图+改图 prompt 属自然衍生,能工作即可不专门保证);Vertex AI 鉴权。
