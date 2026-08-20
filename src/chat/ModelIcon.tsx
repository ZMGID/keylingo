import type { ComponentType, CSSProperties } from 'react'
// Import the exact Color/Mono leaf component FILE (not the brand index, which also
// pulls .Avatar -> features/IconAvatar -> @lobehub/ui -> antd6/React19). This keeps
// us React-18-clean and guarantees antd never enters the bundle. See task research.
import OpenAI from '@lobehub/icons/es/OpenAI/components/Mono'
import Grok from '@lobehub/icons/es/Grok/components/Mono'
import Moonshot from '@lobehub/icons/es/Moonshot/components/Mono'
import Claude from '@lobehub/icons/es/Claude/components/Color'
import Gemini from '@lobehub/icons/es/Gemini/components/Color'
import Gemma from '@lobehub/icons/es/Gemma/components/Color'
import DeepSeek from '@lobehub/icons/es/DeepSeek/components/Color'
import Qwen from '@lobehub/icons/es/Qwen/components/Color'
import ChatGLM from '@lobehub/icons/es/ChatGLM/components/Color'
import Zhipu from '@lobehub/icons/es/Zhipu/components/Color'
import Kimi from '@lobehub/icons/es/Kimi/components/Color'
import Mistral from '@lobehub/icons/es/Mistral/components/Color'
import Meta from '@lobehub/icons/es/Meta/components/Color'
import Yi from '@lobehub/icons/es/Yi/components/Color'
import Doubao from '@lobehub/icons/es/Doubao/components/Color'
import Wenxin from '@lobehub/icons/es/Wenxin/components/Color'
import Minimax from '@lobehub/icons/es/Minimax/components/Color'
import Cohere from '@lobehub/icons/es/Cohere/components/Color'
import Microsoft from '@lobehub/icons/es/Microsoft/components/Color'
import Stepfun from '@lobehub/icons/es/Stepfun/components/Color'
// Provider-only brands (no model-id counterpart above).
import OpenRouter from '@lobehub/icons/es/OpenRouter/components/Mono'
import SiliconCloud from '@lobehub/icons/es/SiliconCloud/components/Color'
import Ollama from '@lobehub/icons/es/Ollama/components/Mono'
import Google from '@lobehub/icons/es/Google/components/Color'
import Nvidia from '@lobehub/icons/es/Nvidia/components/Color'
import Groq from '@lobehub/icons/es/Groq/components/Mono'
import Together from '@lobehub/icons/es/Together/components/Color'
import Fireworks from '@lobehub/icons/es/Fireworks/components/Color'
import Perplexity from '@lobehub/icons/es/Perplexity/components/Color'
import Azure from '@lobehub/icons/es/Azure/components/Color'
import Volcengine from '@lobehub/icons/es/Volcengine/components/Color'
import Bailian from '@lobehub/icons/es/Bailian/components/Color'
import Baichuan from '@lobehub/icons/es/Baichuan/components/Color'
import Hunyuan from '@lobehub/icons/es/Hunyuan/components/Color'
import Spark from '@lobehub/icons/es/Spark/components/Color'
import ModelScope from '@lobehub/icons/es/ModelScope/components/Color'
import GiteeAI from '@lobehub/icons/es/GiteeAI/components/Mono'
import Novita from '@lobehub/icons/es/Novita/components/Color'
import PPIO from '@lobehub/icons/es/PPIO/components/Color'
import Infinigence from '@lobehub/icons/es/Infinigence/components/Color'
import DeepInfra from '@lobehub/icons/es/DeepInfra/components/Color'
import Cerebras from '@lobehub/icons/es/Cerebras/components/Color'
import Hyperbolic from '@lobehub/icons/es/Hyperbolic/components/Color'
import LmStudio from '@lobehub/icons/es/LmStudio/components/Mono'
import Vllm from '@lobehub/icons/es/Vllm/components/Color'
import Xinference from '@lobehub/icons/es/Xinference/components/Color'
import Github from '@lobehub/icons/es/Github/components/Mono'
import Ai302 from '@lobehub/icons/es/Ai302/components/Color'
import AiHubMix from '@lobehub/icons/es/AiHubMix/components/Color'
import SenseNova from '@lobehub/icons/es/SenseNova/components/Color'
import Jina from '@lobehub/icons/es/Jina/components/Mono'
import Voyage from '@lobehub/icons/es/Voyage/components/Color'
import OpenCode from '@lobehub/icons/es/OpenCode/components/Mono'

// lobehub leaf icons declare `size?: string | number`; widen via a loose cast so the
// map stays typed without fighting their prop types.
type Glyph = ComponentType<{ size?: number; style?: CSSProperties }>
const G = (icon: unknown) => icon as Glyph

/** lobehub 的 XiaomiMiMo Mono 是整段字标，18px 格子里会糊成「XIAOMI MIMO」字。橙色 Mi 标在格子里才认得出。 */
function XiaomiMiMoMark({ size = 16, style }: { size?: number; style?: CSSProperties }) {
  return (
    <span
      style={{
        width: size,
        height: size,
        borderRadius: Math.max(3, Math.round(size * 0.22)),
        background: '#FF6900',
        color: '#fff',
        display: 'inline-flex',
        alignItems: 'center',
        justifyContent: 'center',
        fontSize: Math.max(7, Math.round(size * 0.42)),
        fontWeight: 700,
        lineHeight: 1,
        letterSpacing: '-0.05em',
        flexShrink: 0,
        ...style,
      }}
      aria-hidden
    >
      Mi
    </span>
  )
}

// First match wins; tested case-insensitively against the model id.
const MODEL_ICON_MAP: Array<[RegExp, Glyph]> = [
  [/gpt|chatgpt|openai|codex|dall[-·]?e|(?:^|[-/])o[134](?:-|$)/, G(OpenAI)],
  [/claude|anthropic/, G(Claude)],
  [/gemma/, G(Gemma)],
  [/gemini|palm|bison/, G(Gemini)],
  [/deepseek/, G(DeepSeek)],
  [/qwen|qwq|qvq|tongyi|wanx/, G(Qwen)],
  [/grok/, G(Grok)],
  [/kimi/, G(Kimi)],
  [/moonshot/, G(Moonshot)],
  [/glm|chatglm|zhipu/, G(Zhipu)],
  [/mistral|mixtral|codestral|pixtral|ministral|magistral|devstral/, G(Mistral)],
  [/llama|llava/, G(Meta)],
  [/(?:^|[-/])yi-/, G(Yi)],
  [/doubao/, G(Doubao)],
  [/ernie|wenxin/, G(Wenxin)],
  [/minimax|abab/, G(Minimax)],
  [/mimo/, XiaomiMiMoMark],
  [/cohere|command/, G(Cohere)],
  [/(?:^|[-/])phi-|wizardlm/, G(Microsoft)],
  [/(?:^|[-/])step-/, G(Stepfun)],
]

function matchGlyph(model: string): Glyph | null {
  const id = model.toLowerCase()
  for (const [re, glyph] of MODEL_ICON_MAP) {
    if (re.test(id)) return glyph
  }
  return null
}

interface ModelIconProps {
  model: string
  size?: number
  className?: string
}

export function ModelIcon({ model, size = 18, className }: ModelIconProps) {
  const Brand = matchGlyph(model)
  if (Brand) {
    return (
      <span className={className} style={{ display: 'inline-flex', flexShrink: 0 }} aria-hidden="true">
        <Brand size={size} />
      </span>
    )
  }
  // Fallback placeholder — mirrors AgentIcon's initial chip.
  return (
    <span
      className={`inline-flex shrink-0 items-center justify-center rounded-md bg-neutral-200 text-[9px] font-semibold uppercase text-neutral-600 dark:bg-neutral-700 dark:text-neutral-300 ${className ?? ''}`}
      style={{ width: size, height: size }}
      aria-hidden="true"
    >
      {model.replace(/[^a-z0-9]/gi, '').slice(0, 2) || '?'}
    </span>
  )
}

// eslint-disable-next-line react-refresh/only-export-components -- test-only helper
export { matchGlyph as _matchGlyphForTest }

// 供应商图标注册表。key 即持久化到设置里的图标标识（settings.providerIcons）。
// eslint-disable-next-line react-refresh/only-export-components -- 图标表与组件同源，拆文件只为 HMR 不值当
export const PROVIDER_BRANDS: Record<string, Glyph> = {
  OpenAI: G(OpenAI),
  Claude: G(Claude),
  Gemini: G(Gemini),
  Google: G(Google),
  DeepSeek: G(DeepSeek),
  Qwen: G(Qwen),
  Zhipu: G(Zhipu),
  ChatGLM: G(ChatGLM),
  Kimi: G(Kimi),
  Moonshot: G(Moonshot),
  Grok: G(Grok),
  Mistral: G(Mistral),
  Meta: G(Meta),
  Cohere: G(Cohere),
  Minimax: G(Minimax),
  Stepfun: G(Stepfun),
  Doubao: G(Doubao),
  Wenxin: G(Wenxin),
  Yi: G(Yi),
  Microsoft: G(Microsoft),
  Gemma: G(Gemma),
  OpenRouter: G(OpenRouter),
  SiliconCloud: G(SiliconCloud),
  Ollama: G(Ollama),
  Nvidia: G(Nvidia),
  Groq: G(Groq),
  Together: G(Together),
  Fireworks: G(Fireworks),
  Perplexity: G(Perplexity),
  Azure: G(Azure),
  Volcengine: G(Volcengine),
  Bailian: G(Bailian),
  Baichuan: G(Baichuan),
  Hunyuan: G(Hunyuan),
  Spark: G(Spark),
  ModelScope: G(ModelScope),
  GiteeAI: G(GiteeAI),
  Novita: G(Novita),
  PPIO: G(PPIO),
  Infinigence: G(Infinigence),
  DeepInfra: G(DeepInfra),
  Cerebras: G(Cerebras),
  Hyperbolic: G(Hyperbolic),
  LmStudio: G(LmStudio),
  Vllm: G(Vllm),
  Xinference: G(Xinference),
  Github: G(Github),
  Ai302: G(Ai302),
  AiHubMix: G(AiHubMix),
  SenseNova: G(SenseNova),
  Jina: G(Jina),
  Voyage: G(Voyage),
  XiaomiMiMo: XiaomiMiMoMark,
  OpenCode: G(OpenCode),
}

/** 图标选择器顺序：Coding 套餐靠前，ChatGLM 旧标不展示，魔搭/GitHub 沉底。ChatGLM 仍留在 PROVIDER_BRANDS 里，旧的手选记录还能显示。 */
export const PROVIDER_PICKER_KEYS: string[] = [
  'Kimi',
  'Zhipu',
  'XiaomiMiMo',
  'Minimax',
  'OpenCode',
  'DeepSeek',
  'OpenAI',
  'Claude',
  'Gemini',
  'Google',
  'Qwen',
  'Moonshot',
  'Doubao',
  'SiliconCloud',
  'OpenRouter',
  'Grok',
  'Groq',
  'Stepfun',
  'Ai302',
  'Volcengine',
  'Bailian',
  'Wenxin',
  'Hunyuan',
  'Spark',
  'Baichuan',
  'Yi',
  'SenseNova',
  'Mistral',
  'Meta',
  'Cohere',
  'Together',
  'Fireworks',
  'Perplexity',
  'Azure',
  'Nvidia',
  'Ollama',
  'LmStudio',
  'Vllm',
  'Xinference',
  'Cerebras',
  'Hyperbolic',
  'DeepInfra',
  'Novita',
  'PPIO',
  'Infinigence',
  'AiHubMix',
  'Gemma',
  'Microsoft',
  'Jina',
  'Voyage',
  'ModelScope',
  'GiteeAI',
  'Github',
]

// 自动匹配：先按 baseUrl 的域名，再按名字。用户改名成「小白」也能靠域名认出来。
const PROVIDER_ICON_MAP: Array<[RegExp, string]> = [
  [/openrouter/, 'OpenRouter'],
  [/opencode/, 'OpenCode'],
  [/xiaomi|xiaomimimo|mimo\.mi|token-plan/, 'XiaomiMiMo'],
  [/siliconflow|siliconcloud|硅基/, 'SiliconCloud'],
  [/ollama/, 'Ollama'],
  [/bigmodel|zhipu|glm|智谱/, 'Zhipu'],
  [/deepseek/, 'DeepSeek'],
  [/kimi/, 'Kimi'],
  [/moonshot/, 'Moonshot'],
  [/qwen/, 'Qwen'],
  [/doubao|豆包/, 'Doubao'],
  [/anthropic|claude/, 'Claude'],
  [/generativelanguage|googleapis|aistudio|\bgoogle\b|gemini/, 'Google'],
  [/nvidia|英伟达/, 'Nvidia'],
  [/groq/, 'Groq'],
  [/together/, 'Together'],
  [/fireworks/, 'Fireworks'],
  [/perplexity/, 'Perplexity'],
  [/azure/, 'Azure'],
  [/volces|volcengine|ark\.cn|火山/, 'Volcengine'],
  [/dashscope|aliyun|bailian|百炼|通义/, 'Bailian'],
  [/baichuan|百川/, 'Baichuan'],
  [/hunyuan|混元|tencent/, 'Hunyuan'],
  [/xf-yun|iflytek|spark|讯飞|星火/, 'Spark'],
  [/modelscope|魔搭/, 'ModelScope'],
  [/gitee/, 'GiteeAI'],
  [/novita/, 'Novita'],
  [/ppio|派欧/, 'PPIO'],
  [/infini|infinigence|无问/, 'Infinigence'],
  [/deepinfra/, 'DeepInfra'],
  [/cerebras/, 'Cerebras'],
  [/hyperbolic/, 'Hyperbolic'],
  [/lmstudio|lm-studio|127\.0\.0\.1:1234|localhost:1234/, 'LmStudio'],
  [/vllm/, 'Vllm'],
  [/xinference/, 'Xinference'],
  [/github/, 'Github'],
  [/302\.ai/, 'Ai302'],
  [/aihubmix/, 'AiHubMix'],
  [/sensenova|sensetime|商汤/, 'SenseNova'],
  [/jina/, 'Jina'],
  [/voyage/, 'Voyage'],
  [/minimax/, 'Minimax'],
  [/stepfun|阶跃/, 'Stepfun'],
  [/mistral/, 'Mistral'],
  [/cohere/, 'Cohere'],
  [/x\.ai|grok/, 'Grok'],
  [/openai/, 'OpenAI'],
]

function matchProviderGlyph(haystack: string): Glyph | null {
  const s = haystack.toLowerCase()
  for (const [re, key] of PROVIDER_ICON_MAP) {
    if (re.test(s)) return PROVIDER_BRANDS[key]
  }
  return matchGlyph(s)
}

interface ProviderIconProps {
  name: string
  baseUrl?: string
  /** 用户手选的图标 key（PROVIDER_BRANDS 的键）；未设置时自动匹配。 */
  iconKey?: string
  size?: number
  className?: string
}

export function ProviderIcon({ name, baseUrl, iconKey, size = 16, className }: ProviderIconProps) {
  const Brand = (iconKey ? PROVIDER_BRANDS[iconKey] : undefined) ?? matchProviderGlyph(`${baseUrl ?? ''} ${name}`)
  if (Brand) {
    return (
      <span className={className} style={{ display: 'inline-flex', flexShrink: 0 }} aria-hidden="true">
        <Brand size={size} />
      </span>
    )
  }
  return (
    <span
      className={`inline-flex shrink-0 items-center justify-center rounded-md bg-neutral-200 text-[9px] font-semibold uppercase text-neutral-600 dark:bg-neutral-700 dark:text-neutral-300 ${className ?? ''}`}
      style={{ width: size, height: size }}
      aria-hidden="true"
    >
      {name.trim().slice(0, 1) || '?'}
    </span>
  )
}

// eslint-disable-next-line react-refresh/only-export-components -- test-only helper
export { matchProviderGlyph as _matchProviderGlyphForTest }
// eslint-disable-next-line react-refresh/only-export-components -- test-only helper
export const _providerIconMapKeysForTest = () => PROVIDER_ICON_MAP.map(([, key]) => key)
