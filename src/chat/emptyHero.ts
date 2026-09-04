import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import type { Lang } from '../settings/i18n'
import type { BlobAntic } from './kivioBlobSim'

/** 空会话标题：短、跟墨团配。换句间隔随机，大约一分钟上下。 */
export const EMPTY_HERO_ROTATE_MIN_MS = 45_000
export const EMPTY_HERO_ROTATE_MAX_MS = 105_000
export const EMPTY_HERO_ROTATE_MS = EMPTY_HERO_ROTATE_MAX_MS

export function nextEmptyHeroRotateMs(random = Math.random): number {
  const u = random()
  const t = 1 - (1 - u) * (1 - u)
  return EMPTY_HERO_ROTATE_MIN_MS + (EMPTY_HERO_ROTATE_MAX_MS - EMPTY_HERO_ROTATE_MIN_MS) * t
}

const GREETINGS: Record<Lang, readonly string[]> = {
  zh: [
    '想做什么',
    '有活吗',
    '从哪开始',
    '整点啥',
    '说吧',
    '你说我写',
    '有事尽管说',
    '别空转了',
    '先干为敬',
    '来了来了',
    '今天搞点什么',
    '闲着也是闲着',
  ],
  en: [
    "What's next?",
    'Got work?',
    'Where to?',
    "Let's cook.",
    'Say it.',
    'You talk. I type.',
    'Go ahead.',
    'Quit idling.',
    'Work first.',
    'Here I am.',
    "What're we doing?",
    'Might as well.',
  ],
}

export function emptyHeroGreetings(lang: Lang): readonly string[] {
  return GREETINGS[lang]
}

function greetingIndex(seed: string | null | undefined, length: number): number {
  if (!seed || length <= 0) return 0
  let h = 0
  for (let i = 0; i < seed.length; i++) h = (h * 31 + seed.charCodeAt(i)) | 0
  return Math.abs(h) % length
}

export function emptyHeroPinnedLine(opts: {
  lang: Lang
  assistantName?: string | null
  projectName?: string | null
  setName?: string | null
}): string | null {
  const assistant = opts.assistantName?.trim()
  if (assistant) return assistant
  const project = opts.projectName?.trim()
  if (project) return opts.lang === 'zh' ? `在「${project}」` : `In “${project}”`
  const set = opts.setName?.trim()
  if (set) return opts.lang === 'zh' ? `在「${set}」` : `In “${set}”`
  return null
}

export function emptyHeroLine(opts: {
  lang: Lang
  assistantName?: string | null
  projectName?: string | null
  setName?: string | null
  seed?: string | null
}): string {
  const pinned = emptyHeroPinnedLine(opts)
  if (pinned) return pinned
  const list = GREETINGS[opts.lang]
  return list[greetingIndex(opts.seed, list.length)]
}

/** 空态闲置时轮换问候；助手 / 项目 / 集名钉住不转。 */
export function useEmptyHeroLine(opts: {
  lang: Lang
  assistantName?: string | null
  projectName?: string | null
  setName?: string | null
  seed?: string | null
  active?: boolean
}): string {
  const pinned = emptyHeroPinnedLine(opts)
  const greetings = GREETINGS[opts.lang]
  const startIndex = useMemo(
    () => greetingIndex(opts.seed, greetings.length),
    [opts.seed, greetings.length],
  )
  const [tick, setTick] = useState(0)

  useEffect(() => {
    setTick(0)
  }, [startIndex, pinned])

  useEffect(() => {
    if (pinned || opts.active === false) return
    let id = 0
    const arm = () => {
      id = window.setTimeout(() => {
        setTick((n) => n + 1)
        arm()
      }, nextEmptyHeroRotateMs())
    }
    arm()
    return () => window.clearTimeout(id)
  }, [pinned, opts.active, startIndex])

  if (pinned) return pinned
  return greetings[(startIndex + tick) % greetings.length]
}

export const EMPTY_HERO_JAB_MS = 5000

const JABS: Record<Lang, {
  mild: readonly string[]
  mid: readonly string[]
  hot: readonly string[]
  melt: readonly string[]
}> = {
  zh: {
    mild: ['？', '哦', '行', '嗯嗯', '看见了', '你继续'],
    mid: ['挺闲的', '没事吧', '懂了懂了', '很有素质', '您先请', '手速可以', '谢谢啊'],
    hot: ['急了', '典', '收收味', '差不多得了', '您赢了', '哈人', '纯路人问一句'],
    melt: ['绷不住了', '乐', '孝', '没事的', '建议歇会'],
  },
  en: {
    mild: ['?', 'Oh.', 'Sure.', 'Mm.', 'Seen.', 'Go on.'],
    mid: ['Must be bored.', 'You good?', 'Got it.', 'Very classy.', 'After you.', 'Fast hands.', 'Thanks.'],
    hot: ['Mad?', 'Classic.', 'Touch grass.', "That's enough.", 'You win.', 'Weird.', 'Just asking.'],
    melt: ["I'm dead.", 'Lmao.', 'Sure buddy.', "It's fine.", 'Take a walk.'],
  },
}

export function emptyHeroJabPool(lang: Lang, streak: number): readonly string[] {
  const packs = JABS[lang]
  if (streak >= 9) return packs.melt
  if (streak >= 6) return packs.hot
  if (streak >= 3) return packs.mid
  return packs.mild
}

export function emptyHeroJab(
  lang: Lang,
  streak: number,
  last?: string | null,
  random = Math.random,
): string {
  const pool = emptyHeroJabPool(lang, streak)
  const choices = last ? pool.filter((line) => line !== last) : pool
  const list = choices.length > 0 ? choices : pool
  return list[Math.floor(random() * list.length)]
}

/** 闲置小动作时嘟囔一句（变云 / 变方 / 竖起来 / 蹦一下），说完收回。不是每次都说：
 *  蹦是最常见的小动作（每半分钟左右一次），说得少些；变形态本身就稀罕，多半会念一句。 */
export const EMPTY_HERO_MUTTER_MS = 3200
export const EMPTY_HERO_MUTTER_CHANCE = 0.75
export const EMPTY_HERO_MUTTER_HOP_CHANCE = 0.3

const MUTTERS: Record<Lang, Partial<Record<BlobAntic, readonly string[]>>> = {
  zh: {
    cloud: ['走神了', '飘一会', '在想别的', '云一下'],
    squircle: ['今天装方的', '方一下', '换个形状', '有棱有角'],
    egg: ['竖起来听', '有动静？', '站直了', '警觉'],
    hop: ['活动一下', '蹦', '腿麻了', '抖抖'],
  },
  en: {
    cloud: ['Zoning out.', 'Drifting.', 'Elsewhere.', 'Cloud mode.'],
    squircle: ['Boxy today.', 'Squared up.', 'New shape.', 'Edgy.'],
    egg: ['Ears up.', 'Heard something?', 'Standing tall.', 'Alert.'],
    hop: ['Stretching.', 'Boing.', 'Legs asleep.', 'Shake it off.'],
  },
}

export function emptyHeroMutter(
  lang: Lang,
  kind: BlobAntic,
  random = Math.random,
): string | null {
  const pool = MUTTERS[lang][kind]
  if (!pool || pool.length === 0) return null
  if (random() >= (kind === 'hop' ? EMPTY_HERO_MUTTER_HOP_CHANCE : EMPTY_HERO_MUTTER_CHANCE)) return null
  return pool[Math.min(pool.length - 1, Math.floor(random() * pool.length))]
}

export function useEmptyHeroMutter(lang: Lang) {
  const [mutter, setMutter] = useState<string | null>(null)
  const timerRef = useRef(0)

  useEffect(() => () => window.clearTimeout(timerRef.current), [])

  useEffect(() => {
    setMutter(null)
  }, [lang])

  const onAntic = useCallback((kind: BlobAntic) => {
    const line = emptyHeroMutter(lang, kind)
    if (!line) return
    setMutter(line)
    window.clearTimeout(timerRef.current)
    timerRef.current = window.setTimeout(() => setMutter(null), EMPTY_HERO_MUTTER_MS)
  }, [lang])

  return { mutter, onAntic }
}

export function useEmptyHeroJab(lang: Lang) {
  const [jab, setJab] = useState<string | null>(null)
  const lastRef = useRef<string | null>(null)
  const timerRef = useRef(0)

  useEffect(() => () => window.clearTimeout(timerRef.current), [])

  useEffect(() => {
    setJab(null)
    lastRef.current = null
  }, [lang])

  const onPoke = useCallback((streak: number) => {
    const line = emptyHeroJab(lang, streak, lastRef.current)
    lastRef.current = line
    setJab(line)
    window.clearTimeout(timerRef.current)
    timerRef.current = window.setTimeout(() => {
      setJab(null)
      lastRef.current = null
    }, EMPTY_HERO_JAB_MS)
  }, [lang])

  return { jab, onPoke }
}
