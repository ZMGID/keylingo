# 出图接口路由执行契约

> **适用**:改动 `src-tauri/src/chat/image_generation.rs`、`model_metadata.rs` 的生图判定、或新增出图 provider/端点时**必读**。

## 背景红线:端点**不能**从模型名猜

出图有三个端点,历史上靠 `base_url` + 模型名子串猜,散在 6+ 函数、3 张硬编码子串表,极脆:

- 同一模型换名字/大小写/`models/` 前缀 → 路由错;
- 同一模型在不同代理挂在不同端点(实测 `grok-imagine-image`:某代理只支持 `/v1/images/generations`,名字却推向 chat)——**接口本质无法从模型名可靠推断**。

**任何时候都不要再新增「按模型名子串选端点」的分支。** 端点选择只有一个事实源。

## 单一事实源:`resolve_image_route`

`image_generation.rs::resolve_image_route(provider, model) -> ImageRoute { GeminiNative | Chat | ImagesApi }` 是端点判定的唯一入口。优先级:

1. `api_format_kind()==Gemini` → `GeminiNative`(`:generateContent`,强信号)
2. `base_url` 含 `openrouter.ai` → `Chat`
3. 归一化名字启发式(vendor 前缀表 + 裸名子串,**合并为一处**)→ `Chat`(chat/completions,OpenRouter 风格 `message.images`)或 `ImagesApi`(`/images/generations`,b64_json/url)

`generate_image_with_provider` 按 route `match` 调对应 `generate_with_*`。`uses_xai_images_api` / `uses_gpt_image_api_model` 只作 **ImagesApi body 变体**判定(`response_format:b64_json`+aspect_ratio / `size`+`background`),**不再参与端点选择**。

## 模型名归一化:`normalize_model_name`

小写 + 去 `models/` 前缀 + trim。用于:读 `model_overrides` 判生图能力(`model_supports_image_generation` 精确 `get` 未命中时按归一化名回退遍历,消除大小写/前缀静默失效)、名字启发式、`is_image_output_model`。**新增任何按模型名的判定都要先归一化。**

## 猜错自愈 + 会话缓存

`generate_image_with_provider` 先查 `AppState.image_route_cache`(内存 `Mutex<HashMap<(provider_id, normalized_model), ImageRoute>>`,仿 `key_cooldowns`,重启即清)→ 否则 `resolve_image_route`。调用失败且 `is_endpoint_mismatch_error(err)` 命中 → 换 `alternate_route`(**仅 Chat↔ImagesApi**,GeminiNative 无 alt)重试一次 → 成功写缓存,下次直达。

`is_endpoint_mismatch_error` 必须覆盖真机两向措辞(已由真实错误串单测锁定):
- Chat 走错:`only supported on /v1/images/...`
- ImagesApi 走错:`... is **not** supported on /v1/images/...`
- 用 `supported on /v1/images/` 同时匹配 only/not;另含 `/chat/completions`、`must be used with`。**不得**匹配超时/网络/5xx(会多打一次错端点)。

## 无图返回文字兜底

生图模型对笼统提示常返回 200 + 纯文字(澄清/拒绝)无图。`generate_with_openrouter_chat` / `generate_with_gemini_native` 返回 `(images, Option<fallback_text>)`;`generate_image_with_provider` 无图但有文字 → 返回成功结果(content=文字,artifacts 空);`direct_image.rs` artifacts 空时用 `output.content` 渲染。图和文字都无才报硬错误。

## 不碰

- agent loop **内联出图旁路**(`model/openai.rs` `message.images` / `model/gemini.rs` `inlineData` → `GenerateOutput.images` → artifacts)是聊天端点旁路,与本路由是两套,勿混。
- `has_known_direct_image_generation_route` 判据可换来源(现走 resolver),但**不得扩大直连出图范围**——须与旧 `openrouter_chat || xai_images || openai_images_model` 逐例等价(有断言锁定)。
- 不新增持久化配置字段:`ImageRoute` 仅运行时枚举(用户明确决定,勿加 UI/settings 字段)。
