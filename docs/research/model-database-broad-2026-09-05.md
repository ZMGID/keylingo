# Model catalog follow-up — 2026-09-05

- One-pass scan of requested nine major providers, focused on current chat/coding/translation models; separate audio/video/OCR endpoints are not represented as chat models.
- OpenRouter current model catalog supplies channel-specific exact context_length/top_provider.max_completion_tokens and prices where official vendor pages only publish rounded context and no separate output cap. These must not be presented as direct vendor guarantees.
- Mistral old generic mistral-large/mistral-small entries are stale; add exact current IDs and latest aliases rather than letting newer IDs inherit128000/8192 and old pricing.
- Qwen3.8-Max-0902 is also officially aliased qwen3.8-max-2026-09-02.
- Grok Build official aliases grok-code-fast-1/grok-code-fast/grok-code-fast-1-0825; base official price doubles for input context>=200K.
- Tencent HY-MT2 entries are specialist text translation models; reasoning false describes lack of exposed reasoning protocol, not intrinsic ability. No tool use exposed by the channel.

## Kimi Code aliases

Kimi Code k3/k3-256k and kimi-for-coding variants have provider-scoped entries. Actual request IDs are unchanged. Subscription aliases intentionally have no token pricing or undocumented output cap. k3 defaults to the 256K allowance available to Moderato; Allegretto or above may set a 1048576 context override. Do not infer a membership tier from model availability. Low/high/max are supported for both K3 IDs. The public kimi-k3 API entry is separate.

## Already covered

- DeepSeek: Official base API IDs already exist; dated versions Flash0731/Pro0813 can use family matching. https://api-docs.deepseek.com/quick_start/pricing/
- Z.ai: Latest core models already exist; GLM5.3Flash promotional 0.075/0.25/cache0.015 expires Sep9 24:00 UTC+8. https://docs.z.ai/guides/overview/pricing
- MiniMax: Latest text family already exists. https://platform.minimax.io/docs/guides/pricing-paygo
- ByteDance: Official latest core family matches existing catalog. https://seed.bytedance.com/en/seed2_1
- Xiaomi MiMo: No new text family established in one-pass official scan. https://mimo.mi.com/docs/en-US/

## Sources

- https://help.aliyun.com/en/model-studio/qwen3-8-max
- https://huggingface.co/Qwen/Qwen3.8-2.4T-A95B
- https://docs.mistral.ai/models
- https://docs.mistral.ai/models/mistral-medium-3-5-26-04
- https://docs.mistral.ai/models/mistral-small-4-0-26-03
- https://docs.mistral.ai/models/mistral-large-3-25-12
- https://docs.mistral.ai/inference/pricing
- https://docs.mistral.ai/studio/conversations/reasoning
- https://docs.x.ai/developers/models/grok-build-0.1
- https://docs.x.ai/developers/pricing
- https://github.com/Tencent-Hunyuan/Hy-MT2
- https://api-docs.deepseek.com/quick_start/pricing/
- https://seed.bytedance.com/en/seed2_1
- https://mimo.mi.com/docs/en-US/
- https://www.kimi.com/code/docs/en/kimi-code/models.html

## Verified current aliases

Mistral medium-3/medium-latest → Medium 3.5, small-latest → Small 4, large-latest → Large 3. Source: https://docs.mistral.ai/vibe/code/cli/configuration . Grok code-fast aliases → Grok Build 0.1 per https://docs.x.ai/developers/models/grok-build-0.1 . These are current alias targets as of this review date. Existing HY-MT2 entries were already present and were preserved.
