/**
 * 用本仓库协作者身份拉 stargazers，画出 README 里的星标历史。
 * 公开 star-history.com 图在 GitHub 限制 stargazers API 之后会变成占位图。
 *
 *   node scripts/update-star-history.mjs
 */
import { execFileSync } from 'node:child_process'
import { writeFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const ROOT = join(dirname(fileURLToPath(import.meta.url)), '..')
const OUT = join(ROOT, 'docs', 'star-history.svg')
const REPO = 'ZMGID/kivio'

const pages = JSON.parse(
  execFileSync(
    'gh',
    [
      'api',
      '--paginate',
      '--slurp',
      '-H',
      'Accept: application/vnd.github.star+json',
      `repos/${REPO}/stargazers?per_page=100`,
    ],
    { encoding: 'utf8' },
  ),
)
const starredAt = pages
  .flat()
  .map((row) => row.starred_at)
  .filter(Boolean)
  .sort()

if (starredAt.length === 0) {
  throw new Error('no starred_at values; is this token a collaborator on the repo?')
}

const byDay = new Map()
for (const iso of starredAt) {
  const day = iso.slice(0, 10)
  byDay.set(day, (byDay.get(day) ?? 0) + 1)
}

const days = [...byDay.keys()].sort()
let total = 0
const points = days.map((day) => {
  total += byDay.get(day)
  return { day, total }
})

const width = 800
const height = 280
const pad = { l: 48, r: 24, t: 28, b: 40 }
const innerW = width - pad.l - pad.r
const innerH = height - pad.t - pad.b
const t0 = Date.parse(points[0].day)
const t1 = Date.parse(points[points.length - 1].day) || t0 + 1
const yMax = Math.max(points[points.length - 1].total, 1)

const xOf = (day) => pad.l + ((Date.parse(day) - t0) / (t1 - t0)) * innerW
const yOf = (n) => pad.t + innerH - (n / yMax) * innerH

const line = points
  .map((p, i) => `${i === 0 ? 'M' : 'L'}${xOf(p.day).toFixed(1)},${yOf(p.total).toFixed(1)}`)
  .join(' ')

const ticks = 4
const yTicks = Array.from({ length: ticks + 1 }, (_, i) => Math.round((yMax * i) / ticks))
const xLabels = [points[0].day, points[Math.floor(points.length / 2)].day, points[points.length - 1].day]

const svg = `<?xml version="1.0" encoding="UTF-8"?>
<svg xmlns="http://www.w3.org/2000/svg" width="${width}" height="${height}" viewBox="0 0 ${width} ${height}" role="img" aria-label="${REPO} star history">
  <title>${REPO} · ${yMax} stars</title>
  <rect width="100%" height="100%" fill="#fff"/>
  ${yTicks
    .map((n) => {
      const y = yOf(n)
      return `<line x1="${pad.l}" y1="${y.toFixed(1)}" x2="${width - pad.r}" y2="${y.toFixed(1)}" stroke="#e5e7eb"/>
  <text x="${pad.l - 8}" y="${(y + 4).toFixed(1)}" text-anchor="end" font-size="11" fill="#6b7280" font-family="ui-sans-serif,system-ui,sans-serif">${n}</text>`
    })
    .join('\n  ')}
  <path d="${line}" fill="none" stroke="#4f46e5" stroke-width="2.25" stroke-linejoin="round" stroke-linecap="round"/>
  ${xLabels
    .map((day, i) => {
      const x = i === 0 ? pad.l : i === 2 ? width - pad.r : width / 2
      const anchor = i === 0 ? 'start' : i === 2 ? 'end' : 'middle'
      return `<text x="${x}" y="${height - 12}" text-anchor="${anchor}" font-size="11" fill="#6b7280" font-family="ui-sans-serif,system-ui,sans-serif">${day}</text>`
    })
    .join('\n  ')}
  <text x="${pad.l}" y="18" font-size="13" fill="#111827" font-family="ui-sans-serif,system-ui,sans-serif" font-weight="600">${REPO} · ${yMax} stars</text>
</svg>
`

writeFileSync(OUT, svg)
console.log(`wrote ${OUT} (${yMax} stars, ${points.length} days)`)
