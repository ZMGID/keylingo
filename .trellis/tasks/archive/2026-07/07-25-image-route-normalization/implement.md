# 执行计划

按依赖顺序 3 步,每步可独立编译/测试。

## Step 1 归一化 + override 修复(R2)

- [ ] `model_metadata.rs` 新增 `normalize_model_name`(pub(crate))。
- [ ] `model_supports_image_generation`(`:233`)读 override:精确 `get` 未命中时按归一化名回退匹配。
- [ ] `is_image_output_model`(`:325`)、名字启发式表统一走归一化。
- [ ] 单测:大小写 / `models/` 前缀 / override 开关按归一化名命中。
- 验证:`cargo test --lib model_metadata`(或相应模块)全绿。

## Step 2 单一解析器 resolve_image_route(R1)

- [ ] `image_generation.rs` 定义 `ImageRoute` 枚举 + `resolve_image_route`,吸收 `uses_openrouter_chat_image_generation` 端点判据(合并 3 张子串表为一张,走 normalize)。
- [ ] `generate_image_with_provider`(`:83`)改 `match resolve_image_route(...)`。
- [ ] `uses_xai_images_api` / `uses_gpt_image_api_model` 降级为 ImagesApi body 变体私有判定。
- [ ] `has_known_direct_image_generation_route` 判据来源换成 resolver,**不扩大直连范围**,加断言锁定。
- [ ] 迁移既有 grok/gemini/裸名路由用例到 resolver 断言。
- 验证:`cargo test --lib image_generation` 全绿。

## Step 3 自愈 + 会话缓存(R3)

- [ ] `state.rs` 新增 `image_route_cache`(内存 Mutex map)。
- [ ] `image_generation.rs` 新增 `is_endpoint_mismatch_error` + `alternate_route`;`generate_image_with_provider` 先查缓存 → 调用失败且错配 → 换端点重试一次 → 成功写缓存。
- [ ] 单测:`is_endpoint_mismatch_error` 真/假样例、`alternate_route` 映射。
- 验证:`cargo test --lib image_generation` + 全量 `--lib`(对照 baseline)。

## 最终检查(trellis-check)

- [ ] `cargo test --manifest-path src-tauri/Cargo.toml --lib` 全量对照 baseline。
- [ ] `npm run lint` / `typecheck` / `test` 全绿(前端零改动预期)。
- [ ] 红线复查:非生图路径不变;OpenAI/Anthropic/Responses 适配器无关改动;无新增持久化字段;agent loop 内联出图旁路未动。
- [ ] spec 更新(并入本任务):把「出图端点路由 = resolve_image_route + 归一化 + 自愈」+「无图返回文字兜底」沉淀到 spec/guides。
- [ ] 手动(用户,真实 key):gemini(chat)、grok(images_api)出图;改名/加前缀仍对;制造端点错配看自愈 + 缓存。

## 回滚点

每步一 commit;Step 2/3 独立于 Step 1 之上,任一步失败 revert 该 commit。
