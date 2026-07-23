/** Run `transform` only outside fenced/inline code so LaTeX/table fixes never touch code. */
function outsideCode(content: string, transform: (text: string) => string): string {
  // split keeps the code segments (odd indices) verbatim; even indices are prose.
  return content
    .split(/(```[\s\S]*?```|`[^`\n]*`)/g)
    .map((part, i) => (i % 2 === 0 ? transform(part) : part))
    .join('')
}

/**
 * Models sometimes emit GFM tables on one line (`| a | b | | c | d |`) when asked
 * to avoid blank lines. Restore row breaks so remark-gfm can parse tables.
 *
 * They also emit LaTeX with `\[...\]` / `\(...\)` delimiters, which remark-math
 * does not recognize — convert to `$$...$$` / `$...$`. Non-greedy pairing leaves
 * an unclosed `\[` untouched (correct during streaming); inline bodies are trimmed
 * because remark-math rejects `$` followed by whitespace.
 */
export function normalizeMarkdownForRender(content: string): string {
  return outsideCode(content, (text) =>
    text
      .replace(/\\\[([\s\S]*?)\\\]/g, (_m, body) => `\n$$\n${body.trim()}\n$$\n`)
      .replace(/\\\(([\s\S]*?)\\\)/g, (_m, body) => `$${body.trim()}$`)
      .replace(/(\|(?:[^|\n]+\|){2,})\s*(\|)/g, '$1\n$2'),
  )
}
