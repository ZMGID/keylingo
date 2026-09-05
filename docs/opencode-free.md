# OpenCode Free

The Models preset **OpenCode Free** uses the anonymous Zen API at `https://opencode.ai/zen/v1`. No OpenCode installation, account, OAuth flow or API key is required. This is separate from the existing OpenCode external agent (ACP), which does require its CLI, and the paid OpenCode Go preset.

Add the preset, open Manage models, refresh and enable a model. The catalog is fetched from the live `/models` endpoint and saved through the existing provider model list. It includes `*-free` and `big-pickle`, excluding the known keyed `ox-alpha-free` subscription alias. Paid model IDs are rejected in anonymous chat and connection tests. Catalog entries can still be temporarily unavailable upstream; refreshing discovers catalog changes but does not guarantee availability.

Anonymous mode applies only to the exact official HTTPS Zen endpoint using the chat-completions protocol with no keys and no OAuth configuration. It omits authorization headers, uses Kivio's user agent, and sends the existing opaque conversation ID as `x-opencode-session` for cache affinity. Normal keyed providers retain their existing behavior. Existing authenticated Zen configurations remain keyed.

References inspected on 2026-09-06: installed [Hermes provider implementation](https://github.com/NousResearch/hermes-agent/blob/main/hermes_cli/models.py), [OpenCode endpoint documentation](https://opencode.ai/docs/zen/) and the [live model catalog](https://opencode.ai/zen/v1/models). The integration is independently implemented using Kivio's existing HTTP transport and model picker.

Live verification: anonymous `big-pickle` returned HTTP 200 with streaming chat chunks. `deepseek-v4-flash-free` was listed but returned an upstream “Model is unavailable” error. A Python default user agent was rejected with HTTP 403; the explicit Kivio user agent reached inference successfully. No account credentials were used.

## Model metadata and icons

The eight current free IDs now have exact database entries based on the OpenCode provider in models.dev, including gateway-specific limits, zero token pricing and supported reasoning-effort choices. The inspected source fields are retained in `research/opencode-free-models-2026-09-06.json`. These entries take precedence over paid-model prefix matches; for example Muse Contributor Free has a 131072-token output limit, and DeepSeek V4 Flash Free has a 200000-token context. The upstream catalog remains the source for availability, including models that the metadata registry marks deprecated.

Muse uses the Meta icon, Nemotron uses NVIDIA, Ling uses Ant Group, and the opaque Big Pickle model uses its OpenCode host icon without guessing its underlying model vendor. Existing MiMo and DeepSeek icons are retained. Verification: 55 focused frontend tests cover exact/free pricing matches, icon mapping and model selection.
