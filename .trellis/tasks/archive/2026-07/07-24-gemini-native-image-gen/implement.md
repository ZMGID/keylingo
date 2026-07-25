# 执行计划

按依赖顺序 4 步,每步可独立编译/测试,失败可就地回滚。

## Step 1 契约层 + 适配器空实现

- [ ] `model/types.rs`:新增 `GeneratedImageData`、`GenerateOutput.images`(serde default)、`StreamPart::ImageData`。
- [ ] 全仓修编译:所有 `GenerateOutput` 构造点补 `images: Vec::new()`;所有 `StreamPart` 穷举 match 补兜底/空分支(Lens、loop、tests)。
- [ ] `model/README.md` 补 images 契约一句话。
- 验证:`cargo check` + `cargo test --lib` 全绿(行为零变化)。

## Step 2 gemini.rs 出图解析

- [ ] `is_image_output_model` 判定(倾向放 `model_metadata.rs` 单一来源;image_generation.rs 若有等价判定则对齐)。
- [ ] 请求:生图模型追加 `generationConfig.responseModalities: ["TEXT","IMAGE"]`;补文本模型请求体锁定测试。
- [ ] 非流式 `inlineData` → images;流式 `inlineData` → `StreamPart::ImageData` + 聚合。
- [ ] 单测:请求体两态、非流式解析、流式解析(mock chunk)。
- 验证:`cargo test --lib chat::model::gemini`(或该文件测试模块)全绿。

## Step 3 agent loop 落地 artifacts

- [ ] `agent/stream.rs` sink 收集 ImageData;assistant 消息组装处(rounds/finalize/stream 终帧)把 images → `ChatToolArtifact`(data_url;`extension_for_mime`/`decoded_base64_len` 从 image_generation.rs 抽共享或复制)。
- [ ] `loop_tests.rs` 补:fake output 带 images → assistant 消息 artifacts 非空、data_url 前缀正确。
- 验证:`cargo test --lib` 全量绿;`npm test` 全绿(前端应零改动)。

## Step 4 Mixer 工具 Gemini 原生路径

- [ ] `validate_provider` 放行 Gemini。
- [ ] `generate_with_gemini_native`:URL 归一(处理 `models/` 前缀重复)、`x-goog-api-key`、body(prompt + responseModalities + size→aspectRatio)、n>1 循环、`send_with_failover`。
- [ ] 解析 `candidates[*].content.parts[*].inlineData` → GeneratedImage。
- [ ] 单测:body 形状、URL 归一、响应解析、n 钳制沿用。
- 验证:`cargo test --lib` 全绿。

## 最终检查(trellis-check)

- [ ] `cargo test --manifest-path src-tauri/Cargo.toml` 全量对照基线。
- [ ] `npm run lint` / `npm run typecheck` / `npm test` 全绿。
- [ ] 契约红线复查:非生图模型 gemini 请求体不变;OpenAI/Anthropic/Responses 适配器 diff 仅为 `images: Vec::new()` 一类机械补齐;`chat-stream`/`chat-tool` payload 兼容。
- [ ] 手动验收(用户):真实 key 聊天内出图 + Mixer 出图。

## 回滚点

每步一个 commit;Step 2-4 互相独立于 Step 1 之上,任一步失败 revert 该步 commit 即可。
