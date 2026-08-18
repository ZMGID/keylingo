import type { WebSearchConfig, WebSearchProviderId } from '../api/tauri'

/** 当前选中的搜索源是否已具备调用条件（有 key / 实例地址）。 */
export function isWebSearchConfigured(webSearch: WebSearchConfig | undefined): boolean {
  if (!webSearch) return false
  switch (webSearch.provider) {
    case 'tavily':
      return webSearch.tavilyApiKey.trim() !== ''
    case 'exa':
      return webSearch.exaApiKey.trim() !== ''
    case 'exa_mcp':
      return (webSearch.exaMcpUrl ?? '').trim() !== ''
    case 'ollama':
      return (webSearch.ollamaApiKey ?? '').trim() !== ''
    case 'grok':
      return (webSearch.grokApiKey ?? '').trim() !== ''
    case 'brave':
      return (webSearch.braveApiKey ?? '').trim() !== ''
    case 'serper':
      return (webSearch.serperApiKey ?? '').trim() !== ''
    case 'bocha':
      return (webSearch.bochaApiKey ?? '').trim() !== ''
    case 'zhipu':
      return (webSearch.zhipuApiKey ?? '').trim() !== ''
    case 'tinyfish':
      return (webSearch.tinyfishApiKey ?? '').trim() !== ''
    case 'tinyfish_mcp':
      return (webSearch.tinyfishMcpUrl ?? '').trim() !== ''
        && (webSearch.tinyfishMcpAuth?.accessToken ?? '').trim() !== ''
    case 'searxng':
      return (webSearch.searxngBaseUrl ?? '').trim() !== ''
    default:
      return false
  }
}

export function webSearchKeyField(
  provider: WebSearchProviderId,
): keyof Pick<
  WebSearchConfig,
  | 'tavilyApiKey'
  | 'exaApiKey'
  | 'ollamaApiKey'
  | 'grokApiKey'
  | 'braveApiKey'
  | 'serperApiKey'
  | 'bochaApiKey'
  | 'zhipuApiKey'
  | 'tinyfishApiKey'
> | null {
  switch (provider) {
    case 'tavily':
      return 'tavilyApiKey'
    case 'exa':
      return 'exaApiKey'
    case 'ollama':
      return 'ollamaApiKey'
    case 'grok':
      return 'grokApiKey'
    case 'brave':
      return 'braveApiKey'
    case 'serper':
      return 'serperApiKey'
    case 'bocha':
      return 'bochaApiKey'
    case 'zhipu':
      return 'zhipuApiKey'
    case 'tinyfish':
      return 'tinyfishApiKey'
    case 'exa_mcp':
    case 'tinyfish_mcp':
    case 'searxng':
      return null
  }
}
