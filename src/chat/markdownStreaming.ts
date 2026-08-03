import { createContext } from 'react'

// 这条消息是否还在流式生成。ChatMarkdown 里的 HTML 预览用它判断「首次挂载时内容是否已定稿」：
// 历史消息立刻挂 iframe，生成中的消息等内容静默再挂（见 ChatMarkdown 的 HtmlCodePreview）。
export const MarkdownStreamingContext = createContext(false)
