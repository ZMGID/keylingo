// Keep the public plugin reference aligned with the repository specifications.
// Run from any directory: node scripts/sync-plugin-docs.mjs
import { readFileSync, writeFileSync, mkdirSync, copyFileSync } from 'node:fs'
import { fileURLToPath } from 'node:url'
import path from 'node:path'
import { marked } from 'marked'
import { JSDOM } from 'jsdom'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const target = path.join(root, 'website/docs/index.html')
let html = readFileSync(target, 'utf8')
for (const [page, title, source] of [
  ['plugins', '插件与格式', 'kivio-plugin-format.md'],
  ['workflow-hooks', '工作流 Hooks', 'plugin-hooks.md'],
]) {
  const fragment = JSDOM.fragment(marked.parse(readFileSync(path.join(root, 'docs/agents', source), 'utf8')))
  fragment.querySelector('h1').textContent = title
  fragment.querySelectorAll('h2').forEach((heading, index) => { heading.id = `${page}-${index + 1}` })
  const links = {
    'plugin-hooks.md': '#/workflow-hooks',
    'kivio-plugin-format.md': '#/plugins',
    '../schemas/kivio-plugin.v1.schema.json': './schemas/kivio-plugin.v1.schema.json',
  }
  fragment.querySelectorAll('a[href]').forEach(link => {
    const replacement = links[link.getAttribute('href')]
    if (replacement) link.setAttribute('href', replacement)
  })
  const container = fragment.ownerDocument.createElement('div')
  container.append(fragment)
  const article = `<article data-page="${page}" class="hidden">\n<div class="crumb">智能体</div>\n${container.innerHTML}\n</article>`
  const existing = new RegExp(`<article data-page="${page}"[^>]*>[\\s\\S]*?</article>`)
  if (existing.test(html)) html = html.replace(existing, () => article)
  else html = html.replace('    <article data-page="hooks"', () => `${article}\n\n    <article data-page="hooks"`)
}
writeFileSync(target, html)
const schemaDir = path.join(root, 'website/docs/schemas')
mkdirSync(schemaDir, { recursive: true })
copyFileSync(path.join(root, 'docs/schemas/kivio-plugin.v1.schema.json'), path.join(schemaDir, 'kivio-plugin.v1.schema.json'))
