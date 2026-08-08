import { useMemo, useState } from 'react'
import { AlertCircle, CheckCircle2, ChevronDown, ExternalLink, Loader2, Search } from 'lucide-react'
import type { LensWebSearchState } from '../api/tauri'

function sourceHost(url: string): string {
  try {
    return new URL(url).hostname.replace(/^www\./, '')
  } catch {
    return url
  }
}

function compactText(text: string, max = 180): string {
  const cleaned = text.replace(/\s+/g, ' ').trim()
  if (cleaned.length <= max) return cleaned
  return `${cleaned.slice(0, max).trimEnd()}...`
}

export function WebSearchBlock({
  search,
  labels,
  onOpen,
}: {
  search: LensWebSearchState
  labels: {
    searching: string
    results: string
    citations: string
    noResults: string
    error: string
    skipped: string
  }
  onOpen: (url: string) => void
}) {
  const results = search.results ?? []
  const hasDetails = results.length > 0 || Boolean(search.error)
  const [open, setOpen] = useState(false)
  const countLabel = useMemo(() => {
    if (search.status === 'searching') return labels.searching
    if (search.status === 'error') return labels.error
    if (search.status === 'skipped') return labels.skipped
    if (results.length === 0) return labels.noResults
    return labels.results.replace('{count}', String(results.length))
  }, [labels, results.length, search.status])

  return (
    <div className="not-prose mb-2 text-[11.5px] leading-5 text-neutral-500 dark:text-neutral-400">
      <button
        type="button"
        onClick={() => {
          if (hasDetails) setOpen(v => !v)
        }}
        className={`max-w-full min-w-0 inline-flex items-center gap-1.5 rounded-md py-0.5 transition-colors ${
          hasDetails
            ? 'hover:text-neutral-700 dark:hover:text-neutral-200'
            : 'cursor-default'
        }`}
      >
        {search.status === 'searching' ? (
          <Loader2 className="animate-spin shrink-0" size={12} />
        ) : search.status === 'error' ? (
          <AlertCircle className="shrink-0 text-red-500" size={12} strokeWidth={1.9} />
        ) : results.length > 0 ? (
          <CheckCircle2 className="shrink-0 text-[#2f6ff0] dark:text-[#5c8df7]" size={12} strokeWidth={1.9} />
        ) : (
          <Search className="shrink-0" size={12} strokeWidth={1.85} />
        )}
        <span className="font-medium truncate">{countLabel}</span>
        {search.query && (
          <span className="min-w-0 truncate text-neutral-400 dark:text-neutral-500">
            · {search.query}
          </span>
        )}
        {hasDetails && (
          <ChevronDown size={11} strokeWidth={2} className={`shrink-0 transition-transform ${open ? 'rotate-180' : ''}`} />
        )}
      </button>
      {open && hasDetails && (
        <div className="mt-1.5 ml-1.5 border-l border-black/[0.08] dark:border-white/[0.1] pl-2.5">
          {search.error && (
            <div className="text-red-500 whitespace-pre-wrap break-words">
              {search.error}
            </div>
          )}
          {!search.error && results.length > 0 && (
            <div className="space-y-1">
              <div className="text-[10.5px] text-neutral-400 dark:text-neutral-500">
                {labels.citations.replace('{count}', String(results.length))}
              </div>
              {results.map((result, idx) => (
                <button
                  key={`${result.url}-${idx}`}
                  type="button"
                  onClick={() => onOpen(result.url)}
                  className="group block w-full min-w-0 text-left rounded-md py-1 transition-colors hover:text-neutral-700 dark:hover:text-neutral-200"
                >
                  <div className="flex items-center gap-1.5 min-w-0">
                    <span className="shrink-0 w-4 text-[10.5px] font-medium tabular-nums text-[#2f6ff0] dark:text-[#5c8df7]">
                      {idx + 1}
                    </span>
                    <span className="min-w-0 flex-1 truncate text-[11.5px] font-medium text-neutral-700 dark:text-neutral-200">
                      {result.title || sourceHost(result.url)}
                    </span>
                    <ExternalLink size={10.5} className="shrink-0 text-neutral-300 dark:text-neutral-600 group-hover:text-neutral-500 dark:group-hover:text-neutral-300" />
                  </div>
                  <div className="mt-0.5 pl-5 text-[10.5px] leading-4 text-neutral-400 dark:text-neutral-500 truncate">
                    {sourceHost(result.url)}
                    {result.publishedDate ? ` · ${result.publishedDate}` : ''}
                  </div>
                  {result.content && (
                    <div className="mt-0.5 pl-5 text-[11px] leading-5 text-neutral-500 dark:text-neutral-400">
                      {compactText(result.content)}
                    </div>
                  )}
                </button>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  )
}
