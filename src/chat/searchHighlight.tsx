import { Fragment, type ReactNode } from 'react'

/** 大小写不敏感拆分：按 needle 切开，保留原文大小写。空 query 原样返回。 */
export function splitHighlightParts(text: string, query: string): Array<{ text: string; match: boolean }> {
  const needle = query.trim()
  if (!text || !needle) return [{ text, match: false }]
  const lowerText = text.toLowerCase()
  const lowerNeedle = needle.toLowerCase()
  if (!lowerNeedle) return [{ text, match: false }]

  const parts: Array<{ text: string; match: boolean }> = []
  let cursor = 0
  while (cursor < text.length) {
    const found = lowerText.indexOf(lowerNeedle, cursor)
    if (found < 0) {
      parts.push({ text: text.slice(cursor), match: false })
      break
    }
    if (found > cursor) {
      parts.push({ text: text.slice(cursor, found), match: false })
    }
    parts.push({ text: text.slice(found, found + needle.length), match: true })
    cursor = found + needle.length
    if (needle.length === 0) break
  }
  return parts.length > 0 ? parts : [{ text, match: false }]
}

/** 渲染关键字高亮（黄底，对齐 Cherry Studio 搜索观感）。 */
export function HighlightText({
  text,
  query,
  className,
}: {
  text: string
  query: string
  className?: string
}): ReactNode {
  const parts = splitHighlightParts(text, query)
  if (parts.length === 1 && !parts[0].match) {
    return className ? <span className={className}>{text}</span> : text
  }
  return (
    <span className={className}>
      {parts.map((part, index) => (
        part.match
          ? (
            <mark
              key={index}
              className="rounded-[2px] bg-amber-300/90 px-0.5 text-neutral-900 dark:bg-amber-400/85 dark:text-neutral-950"
            >
              {part.text}
            </mark>
          )
          : <Fragment key={index}>{part.text}</Fragment>
      ))}
    </span>
  )
}
