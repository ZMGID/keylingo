# 技术设计

## 分层与契约

改动局限在出图**端点选择**层(`image_generation.rs` + `model_metadata.rs` 判定 + `state.rs` 缓存)。不碰 model 契约层、不碰 agent loop 内联出图旁路。

```
resolve_image_route(provider, model)                     ← 单一事实源
   ├─ 会话缓存命中 (provider_id, normalized_model) → route   ← R3
   ├─ api_format==Gemini            → GeminiNative
   ├─ base_url 含 openrouter.ai     → Chat
   └─ 归一化名字启发式(合并表)      → Chat | ImagesApi
                    │
generate_image_with_provider: 按 route 调对应 generate_with_*
                    │  端点错配错误?
                    └─ 换另一端点重试一次 → 成功则写缓存        ← R3
```

## 现状锚点(file:line)

- 顶层分流:`image_generation.rs:83` `if Gemini {gemini_native} else if uses_openrouter_chat {chat} else {images_api}`
- `uses_openrouter_chat_image_generation`:`image_generation.rs:747`(base_url + 模型名前缀/子串 3 类判据)
- `uses_xai_images_api`:`:794`;`uses_gpt_image_api_model`:`:805`;`openrouter_modalities`:`:809`
- `has_known_direct_image_generation_route`:`:164`(总闸,仅 OpenAiChat)
- 三个 `generate_with_*`:`generate_with_images_api :237`(→ `/images/generations`)、`generate_with_openrouter_chat :332`(→ `/chat/completions`)、`generate_with_gemini_native :455`(→ `:generateContent`)
- `image_generation_model_name_heuristic`:`model_metadata.rs:290`;`is_image_output_model`:`:325`;override 精确取值 `model_metadata.rs:238`

## R1 `resolve_image_route`

```rust
enum ImageRoute { GeminiNative, Chat, ImagesApi }
fn resolve_image_route(provider: &ModelProvider, model: &str) -> ImageRoute
```
- `generate_image_with_provider`(`:83`)改为 `match resolve_image_route(...)` 三分支,分别调既有 `generate_with_*`。
- `resolve_image_route` 内部吸收 `uses_openrouter_chat_image_generation` 的判据(openrouter base、api.openai.com/api.x.ai 早退、归一化名字启发式合并 3 张表)。`uses_xai_images_api` / `uses_gpt_image_api_model` 降级为「ImagesApi body 变体判定」的私有 helper,不再参与端点选择(端点已由 resolver 定)。
- `has_known_direct_image_generation_route`(`model_metadata.rs:245` 用)保持对外语义:改为 `resolve_image_route(...) != GeminiNative-only-且不可直连`……实际:让它 = 「resolver 能给出可用端点」——OpenAiChat/Responses 恒 true(总有 chat/images 兜底),对齐现有「OpenAiChat 才直连」但把判据来源换成 resolver,不扩大直连范围。

## R2 `normalize_model_name`

```rust
fn normalize_model_name(model: &str) -> String  // lower + strip "models/" + trim
```
- 放 `model_metadata.rs`(与 override 读取同处),`image_generation.rs` 复用。
- `model_supports_image_generation`(`:233`)读 override 前先 normalize key:优先精确 `get(model)`,未命中再按归一化名遍历匹配(小 HashMap,O(n) 可接受),消除大小写/前缀静默失效。
- 名字启发式表与 `is_image_output_model` 统一先 normalize。

## R3 自愈 + 缓存

- `state.rs` 新增 `image_route_cache: Mutex<HashMap<(String,String), ImageRoute>>`(仿 `key_cooldowns`,内存态)。`ImageRoute` 需 `Clone+Copy+Eq+Hash` 或缓存存判别值。
- `generate_image_with_provider`:先查缓存 → 否则 `resolve_image_route`。调用失败且错误串命中 `is_endpoint_mismatch_error(err)`(匹配 `only supported on /v1/images/`、`/chat/completions`、`must be used with`、含 4xx)→ 取 `alternate_route(route)`(Chat↔ImagesApi,GeminiNative 无 alt)重试一次;成功写缓存。
- `is_endpoint_mismatch_error` 独立纯函数,便于单测。

## 测试

- `resolve_image_route`:gemini provider→GeminiNative;openrouter base→Chat;cpa 代理裸名 gemini-image→Chat、grok-imagine-image→ImagesApi、gpt-image→ImagesApi。
- 归一化:`Gemini-3.1-Flash-Image` / `models/…` 与裸名同结果;override image_generation 开关按归一化名命中。
- 自愈:`is_endpoint_mismatch_error` 对样例错误串判真/对普通错误判假;`alternate_route` 映射正确。
- 迁移既有 `bare_image_model_names_route_through_chat_on_generic_proxy` 等到 resolver 断言。

## 风险与回滚

- 风险:resolver 收敛后某冷门模型端点判定漂移 → 自愈兜底 + 单测覆盖主要模型族。
- `has_known_direct_image_generation_route` 语义若变动影响 `model_can_generate_images_directly`(直连出图总闸)→ 保持「不扩大直连范围」,仅换判据来源,加断言锁定。
- 回滚:单任务提交,revert 即回滚;无数据/配置迁移。
