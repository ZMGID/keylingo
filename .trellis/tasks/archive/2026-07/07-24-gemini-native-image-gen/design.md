# 技术设计

## 分层与契约

保持「一环多宿主」与 model 层契约(`model/README.md`):runtime 不看 provider JSON;图片以协议无关形态穿过契约层。

```
gemini.rs (wire: inlineData) ──> GenerateOutput.images / StreamPart::ImageData (契约层,协议无关)
                                        │
                              agent/stream.rs sink 收集
                                        │
                              ChatMessage.artifacts (ChatToolArtifact, data_url)
                                        │
                              前端 MessageBubble 现有 artifacts 渲染
```

## 契约层改动 (`model/types.rs`)

```rust
pub struct GeneratedImageData { pub mime_type: String, pub data: String /* base64 */ }

pub struct GenerateOutput {
    ...
    #[serde(default)]
    pub images: Vec<GeneratedImageData>,   // 新增,默认空
}

pub enum StreamPart {
    ...
    ImageData { mime_type: String, data: String },  // 新增变体
}
```

- 所有构造 `GenerateOutput` 的适配器补 `images: Vec::new()`(编译器驱动,遗漏即报错)。
- 消费 `StreamPart` 的 sink:仅 `AgentStreamSink` 新增处理;其余(Lens、丢弃 sink、loop_tests fake)一律 `_ => {}` 兜底已存在则零改动,若是穷举 match 补空分支。

## gemini.rs 改动

1. **请求**:`build_request_body` 中,当 `request.model` 判定为生图模型时追加
   `generationConfig.responseModalities = ["TEXT","IMAGE"]`。判定函数 `is_image_output_model(model)`:名称含 `image` 或以 `imagen` 开头(与 image_generation.rs 现有判定对齐,放 model_metadata 或 gemini.rs 本地皆可,倾向 model_metadata 单一来源)。
   - 红线:非生图模型走到该分支之外,请求体与现状字节一致。现有请求体测试若无则补一条锁定。
2. **非流式解析**:`parse_parts` 系(约 L787 起)在 text/thought/functionCall 之外识别 `inlineData` part → 收集进 images。
3. **流式解析**:chunk 逐 part 扫描处(约 L227 起)同样识别 `inlineData` → 发 `StreamPart::ImageData` 并累计到最终 `GenerateOutput.images`。
4. **回放**:assistant 历史中的图片不回放(维持现有 `MessagePart::Image` 仅 User 上行的行为,已天然满足)。

## agent 侧改动

- `agent/stream.rs` `AgentStreamSink`:收到 `ImageData` 暂存;`rounds.rs`/`finalize.rs` 组装 assistant `ChatMessage` 时把 `GenerateOutput.images` 转为 `ChatToolArtifact { name: "generated-image-N.png", mime_type, data_url, size_bytes }` 挂 `artifacts`(复用 `image_generation.rs` 的 `extension_for_mime`/`decoded_base64_len`,必要时移到共享处)。
- 事件:artifacts 已在 `chat-stream`/`chat-tool` 消息结构中,前端 MessageBubble 现有 artifacts 渲染直接生效,无前端改动预期;若终帧消息未带 artifacts 则在 finish 帧补。

## image_generation.rs 改动 (Mixer)

1. `validate_provider`:`ProviderApiFormat::Gemini` 放行。
2. 路由:`generate_image_with_provider` 增加分支 `provider.api_format_kind() == Gemini` → `generate_with_gemini_native`。
3. `generate_with_gemini_native`:
   - URL:`{base}/models/{model}:generateContent`(base 处理与 gemini.rs 相同的尾斜杠/版本段规则,抽用或复制其 URL 组装逻辑,注意 `models/` 前缀已含于模型名的情况——现有 usage 里模型名形如 `models/gemini-3.1-flash-lite`,需归一)。
   - Header:`x-goog-api-key: {key}`(与 gemini.rs 一致),走 `send_with_failover` + `IMAGE_GENERATION_HTTP_TIMEOUT`。
   - Body:`{ contents: [{ role:"user", parts:[{text: prompt}] }], generationConfig: { responseModalities:["TEXT","IMAGE"], ...imageConfig? } }`;size→aspectRatio 映射复用现有 `map_size_to_aspect`(如 "1024x1024"→"1:1")。
   - `n > 1`:循环调用 n 次(candidateCount 对 image 输出支持不稳,循环最稳),失败即止、已得图返回。
   - 解析:`candidates[*].content.parts[*].inlineData` → `GeneratedImage { mime_type, base64, revised_prompt: None }`。

## 测试策略

- Rust 单测(gemini.rs 内既有测试模块风格):
  - 生图模型请求体含 responseModalities;文本模型请求体不含且与旧快照一致。
  - 非流式响应 mock(text part + inlineData part)→ text 与 images 都正确。
  - 流式 chunk mock → 发射 ImageData 且 finish 后 images 聚合正确。
  - image_generation:gemini 原生 body 形状、URL 归一(`models/` 前缀)、inlineData 解析。
- `loop_tests.rs`:一条用例——fake provider 返回带 images 的 output,断言 assistant 消息 artifacts 非空。
- 回归:`cargo test --lib` 全量;`npm test`(前端无改动预期,防波及)。

## 风险与回滚

- 最大风险:gemini 请求体意外变化导致普通文本模型 400。锁定测试 + 分支只在 is_image_output_model 内生效。
- StreamPart 新变体导致外部 match 不穷尽:编译期即暴露,逐个补兜底。
- 回滚:单任务分支式提交,revert 即回滚;无数据迁移。
