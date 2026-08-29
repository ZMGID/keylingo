import type { Lang } from '../settings/i18n'

/** 空会话标题：短、跟墨团配。 */
const GREETINGS: Record<Lang, readonly string[]> = {
  zh: ['想做什么', '从哪开始', '说吧', '有事尽管说'],
  en: ["What's next?", 'Where to?', 'Say it.', 'Go ahead.'],
}

function pickGreeting(lang: Lang, seed: string | null | undefined): string {
  const list = GREETINGS[lang]
  if (!seed) return list[0]
  let h = 0
  for (let i = 0; i < seed.length; i++) h = (h * 31 + seed.charCodeAt(i)) | 0
  return list[Math.abs(h) % list.length]
}

export function emptyHeroLine(opts: {
  lang: Lang
  assistantName?: string | null
  projectName?: string | null
  setName?: string | null
  seed?: string | null
}): string {
  const assistant = opts.assistantName?.trim()
  if (assistant) return assistant
  const project = opts.projectName?.trim()
  if (project) return opts.lang === 'zh' ? `在「${project}」` : `In “${project}”`
  const set = opts.setName?.trim()
  if (set) return opts.lang === 'zh' ? `在「${set}」` : `In “${set}”`
  return pickGreeting(opts.lang, opts.seed)
}
