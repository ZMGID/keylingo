// Presets only prefill provider metadata. Models are fetched from the provider API
// and explicitly enabled by the user.

export type ProviderPreset = {
  name: string
  /** OpenAI-compatible base URL, usually including /v1. */
  baseUrl: string
  /** 申请 API Key 的页面（在 API 密钥区显示「获取 API Key」引导链接）。本地/无需 key 的可省略。 */
  apiKeyUrl?: string
  /** 接口协议，省略即 openai_chat。Grok 之类有专属协议的必须写明，否则一键添加出来是错的。 */
  apiFormat?: string
}

/** 顺序：国内 Coding / Token 套餐 → 一线实验室 → 云厂商/聚合 → 本地 → 少用的。 */
export const PROVIDER_PRESETS: ProviderPreset[] = [
  {
    name: 'Kimi for Coding',
    baseUrl: 'https://api.kimi.com/coding/v1',
    apiKeyUrl: 'https://www.kimi.com/code/console',
  },
  {
    name: 'GLM Coding Plan',
    baseUrl: 'https://open.bigmodel.cn/api/coding/paas/v4',
    apiKeyUrl: 'https://www.bigmodel.cn/coding-plan/personal/overview',
  },
  {
    name: 'Xiaomi Token Plan',
    baseUrl: 'https://token-plan-cn.xiaomimimo.com/v1',
    apiKeyUrl: 'https://mimo.mi.com',
  },
  {
    name: 'MiniMax Token Plan',
    baseUrl: 'https://api.minimaxi.com/v1',
    apiKeyUrl: 'https://platform.minimaxi.com/user-center/payment/token-plan',
  },
  {
    name: 'OpenCode Go',
    baseUrl: 'https://opencode.ai/zen/go/v1',
    apiKeyUrl: 'https://opencode.ai/auth',
  },
  {
    name: 'DeepSeek',
    baseUrl: 'https://api.deepseek.com/v1',
    apiKeyUrl: 'https://platform.deepseek.com/api_keys',
  },
  {
    name: 'OpenAI',
    baseUrl: 'https://api.openai.com/v1',
    apiKeyUrl: 'https://platform.openai.com/api-keys',
  },
  {
    name: 'Anthropic',
    baseUrl: 'https://api.anthropic.com',
    apiKeyUrl: 'https://console.anthropic.com/settings/keys',
    apiFormat: 'anthropic_messages',
  },
  {
    name: 'Gemini',
    baseUrl: 'https://generativelanguage.googleapis.com/v1beta',
    apiKeyUrl: 'https://aistudio.google.com/apikey',
    apiFormat: 'gemini',
  },
  {
    name: 'Moonshot',
    baseUrl: 'https://api.moonshot.cn/v1',
    apiKeyUrl: 'https://platform.moonshot.cn/console/api-keys',
  },
  {
    name: 'GLM',
    baseUrl: 'https://open.bigmodel.cn/api/paas/v4',
    apiKeyUrl: 'https://open.bigmodel.cn/usercenter/apikeys',
  },
  {
    name: 'Qwen',
    baseUrl: 'https://dashscope.aliyuncs.com/compatible-mode/v1',
    apiKeyUrl: 'https://bailian.console.aliyun.com/?tab=model#/api-key',
  },
  {
    name: 'Doubao',
    baseUrl: 'https://ark.cn-beijing.volces.com/api/v3',
    apiKeyUrl: 'https://console.volcengine.com/ark/region:ark+cn-beijing/apiKey',
  },
  {
    name: 'SiliconFlow',
    baseUrl: 'https://api.siliconflow.cn/v1',
    apiKeyUrl: 'https://cloud.siliconflow.cn/account/ak',
  },
  {
    name: 'OpenRouter',
    baseUrl: 'https://openrouter.ai/api/v1',
    apiKeyUrl: 'https://openrouter.ai/keys',
  },
  {
    name: 'Grok',
    baseUrl: 'https://api.x.ai/v1',
    apiKeyUrl: 'https://console.x.ai/',
    apiFormat: 'xai_responses',
  },
  {
    name: 'Groq',
    baseUrl: 'https://api.groq.com/openai/v1',
    apiKeyUrl: 'https://console.groq.com/keys',
  },
  {
    name: 'StepFun',
    baseUrl: 'https://api.stepfun.com/v1',
    apiKeyUrl: 'https://platform.stepfun.com/interface-key',
  },
  {
    name: '302.AI',
    baseUrl: 'https://api.302.ai/v1',
    apiKeyUrl: 'https://302.ai',
  },
  {
    name: 'Mistral',
    baseUrl: 'https://api.mistral.ai/v1',
    apiKeyUrl: 'https://console.mistral.ai/api-keys',
  },
  {
    name: 'Together',
    baseUrl: 'https://api.together.xyz/v1',
    apiKeyUrl: 'https://api.together.ai/settings/api-keys',
  },
  {
    name: 'Fireworks',
    baseUrl: 'https://api.fireworks.ai/inference/v1',
    apiKeyUrl: 'https://fireworks.ai/account/api-keys',
  },
  {
    name: 'Perplexity',
    baseUrl: 'https://api.perplexity.ai',
    apiKeyUrl: 'https://www.perplexity.ai/settings/api',
  },
  {
    name: 'Ollama',
    baseUrl: 'https://ollama.com/v1',
    apiKeyUrl: 'https://ollama.com/settings/keys',
  },
  {
    name: 'Ollama Local',
    baseUrl: 'http://127.0.0.1:11434/v1',
  },
  {
    name: 'LM Studio',
    baseUrl: 'http://127.0.0.1:1234/v1',
  },
  {
    name: 'ModelScope',
    baseUrl: 'https://api-inference.modelscope.cn/v1',
    apiKeyUrl: 'https://modelscope.cn/my/myaccesstoken',
  },
  {
    name: 'GitHub Models',
    baseUrl: 'https://models.github.ai/inference',
    apiKeyUrl: 'https://github.com/settings/tokens',
  },
]
