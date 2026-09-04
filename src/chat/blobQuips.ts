import { useEffect, useRef, useState } from 'react'
import type { Lang } from '../settings/i18n'
import type { BlobMood } from './kivioBlobSim'

/**
 * 流状态行上墨团偶尔冒的一句嘴（「1s · 72 tokens · 1 running · 琢磨中」的最后一截）。
 * 不是每时每刻都说：想 / 搜 / 干活 / 说话这四个阶段要先待够一会才开口，说完收回、隔一阵再来；
 * 翻车 / 收工 / 等你这三种是边沿事件，进去就说、一直挂着。
 */
export type QuipMood = Exclude<BlobMood, 'idle'>

export const STATUS_QUIPS: Record<Lang, Record<QuipMood, readonly string[]>> = {
  zh: {
    think: ['琢磨中', '让我想想', '脑子在转', '嗯……', '别急', '在憋大招', '有点意思'],
    search: ['翻翻看', '找找找', '哪呢哪呢', '让我搜搜', '眼睛都瞪大了'],
    work: ['干活中', '手没停', '别催', '搬砖ing', '专心呢', '在动手了'],
    speak: ['在说了在说了', '边想边写', '听我说完', '有话直说'],
    error: ['翻车了', '啊这', '这波不行', '出事了', '别看我'],
    done: ['搞定', '完事', '交作业', '收工', '还行吧', '就这'],
    wait: ['等你呢', '看你了', '你说', '卡在你这了', '选一个'],
  },
  en: {
    think: ['Thinking…', 'Hmm.', 'Gimme a sec.', 'Cooking.', 'Hold on.', 'Interesting.'],
    search: ['Looking…', 'Where is it.', 'Digging.', 'Searching.', 'Eyes peeled.'],
    work: ['On it.', 'Busy hands.', "Don't rush me.", 'Grinding.', 'Focused.'],
    speak: ['Talking here.', 'Writing as I think.', 'Let me finish.', 'Straight up.'],
    error: ['Welp.', 'Oops.', 'That broke.', 'Not my day.', "Don't look at me."],
    done: ['Done.', 'Shipped.', 'There.', 'Wrapped.', 'Not bad.', "That's it."],
    wait: ['Your turn.', 'Waiting on you.', 'Go on.', 'Stuck on you.', 'Pick one.'],
  },
}

/**
 * 何时开口。`first` = 进入该心情多久后第一句；`show` = 一句挂多久（null = 常驻）；
 * `gap` = 收回（或常驻换句）前隔多久。
 */
export interface QuipPlan {
  first: number
  show: number | null
  gap: [number, number]
}

export function quipPlan(mood: BlobMood): QuipPlan | null {
  switch (mood) {
    case 'think':
      return { first: 6000, show: 4500, gap: [7000, 14000] }
    case 'search':
      return { first: 3500, show: 3500, gap: [6000, 12000] }
    case 'work':
      return { first: 5000, show: 4000, gap: [7000, 14000] }
    case 'speak':
      return { first: 9000, show: 3500, gap: [12000, 20000] }
    case 'error':
      return { first: 500, show: null, gap: [0, 0] }
    case 'done':
      return { first: 0, show: null, gap: [0, 0] }
    case 'wait':
      return { first: 900, show: null, gap: [9000, 9000] }
    default:
      return null
  }
}

export function pickQuip(
  lang: Lang,
  mood: BlobMood,
  last?: string | null,
  random: () => number = Math.random,
): string | null {
  if (mood === 'idle') return null
  const pool = STATUS_QUIPS[lang][mood]
  if (!pool || pool.length === 0) return null
  const choices = last ? pool.filter((line) => line !== last) : pool
  const list = choices.length > 0 ? choices : pool
  return list[Math.min(list.length - 1, Math.floor(random() * list.length))]
}

function within([a, b]: [number, number], random: () => number): number {
  return a + (b - a) * random()
}

/** 按 quipPlan 的节拍说话；心情一换就闭嘴重排。`enabled=false` 全程不吭声。 */
export function useStatusQuip(mood: BlobMood, lang: Lang, enabled = true): string | null {
  const [quip, setQuip] = useState<string | null>(null)
  const lastRef = useRef<string | null>(null)

  useEffect(() => {
    setQuip(null)
    lastRef.current = null
    const plan = quipPlan(mood)
    if (!plan || !enabled) return
    let id = 0
    const say = () => {
      const line = pickQuip(lang, mood, lastRef.current)
      lastRef.current = line
      setQuip(line)
      if (plan.show == null) {
        if (plan.gap[1] > 0) id = window.setTimeout(say, within(plan.gap, Math.random))
        return
      }
      id = window.setTimeout(() => {
        setQuip(null)
        id = window.setTimeout(say, within(plan.gap, Math.random))
      }, plan.show)
    }
    id = window.setTimeout(say, plan.first)
    return () => window.clearTimeout(id)
  }, [mood, lang, enabled])

  return quip
}
