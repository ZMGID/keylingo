# Model database refresh — 2026-09-05

Six entries verified from vendor documentation and channel catalogs. Existing models and user overrides are preserved. Prices are USD per million tokens; the database supports only a flat baseline, not tiered or scheduled pricing.

## Sources and limitations

### gpt-6-astra

Remove temperature, top_p, top_logprobs; Chat Completions also logprobs. Prompts >272K tokens have 2x input/cache and 1.5x output rates. Only gpt-6-astra is documented; no evidence for gpt-6 base alias, pro, or ultra API effort. Access rollout is account-dependent.

### gemini-3.8-flash

Only low/medium/high; minimal errors. Remove deprecated temperature/top_p/top_k; use thinking_level rather than thinking_budget. Intro pricing through 2026-12-31, then input1.50/output7.50/cache0.15. Google Search support is official Gemini API capability, not Antigravity channel capability.

### claude-fable-5-1

Adaptive thinking always on; disabled or manual budget thinking errors. Non-default temperature/top_p/top_k errors. Forced tool any/tool errors. Default effort high. Context and output documented as 1M/128K; decimal representation follows Anthropic conventions.

### muse-spark-1.3

Exact context1048576 and maxOutput943718 are OpenRouter live API fields context_length and top_provider.max_completion_tokens, not a direct Meta guarantee. Pricing likewise OpenRouter channel baseline. Vercel official gateway confirms same prices (1.25/4.25/cache0.15; contributor0.1/0.2/cache0.002). Contributor permits training on submitted usage. OpenRouter currently lists mandatory reasoning and minimal/low/medium/high/xhigh/max; Meta Sep2 announcement says max coming later. Conservative common efforts omit max and omit minimal unsupported by this project. No direct Meta sampling constraint confirmed.

### claude-mythos-5-1

Same specs/pricing/efforts as Fable5.1, but access requires Project Glasswing invitation.

The Contributor model is catalogued only; this update does not select it or send it any data. Model availability still comes from the configured provider.

## References

- https://developers.openai.com/api/docs/models/gpt-6-astra
- https://developers.openai.com/api/docs/guides/latest-model?model=gpt-6-astra
- https://ai.google.dev/gemini-api/docs/models/gemini-3.8-flash
- https://ai.google.dev/gemini-api/docs/latest-model?hl=en
- https://ai.google.dev/gemini-api/docs/pricing
- https://platform.claude.com/docs/en/models/fable-5-1/overview
- https://platform.claude.com/docs/en/models/fable-5-1/whats-new-fable-5-1
- https://platform.claude.com/docs/en/build-with-claude/effort
- https://research.meta.ai/blog/introducing-muse-spark-1-3
- https://openrouter.ai/api/v1/models
- https://openrouter.ai/meta/muse-spark-1.3
- https://vercel.com/ai-gateway/models/muse-spark-1.3
- https://vercel.com/ai-gateway/models/muse-spark-1.3-contributor/providers
- https://platform.claude.com/docs/en/models/mythos-5-1/overview
