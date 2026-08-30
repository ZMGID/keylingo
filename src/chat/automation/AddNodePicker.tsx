import { useMemo, useState } from 'react'
import { Search, X } from 'lucide-react'
import { IconButton } from '../../components/Button'
import { useT } from '../../settings/i18n'
import { catalogGroups, type NodeCatalogEntry } from './nodeCatalog'

export function AddNodePicker({
  kind,
  presentTypes = [],
  onPick,
  onCancel,
}: {
  kind: 'trigger' | 'action'
  presentTypes?: string[]
  onPick: (entry: NodeCatalogEntry) => void
  onCancel?: () => void
}) {
  const t = useT()
  const [query, setQuery] = useState('')
  const title = kind === 'trigger' ? t.chatAutomationAddTrigger : t.chatAutomationWhatNext
  const kicker = kind === 'trigger' ? t.chatAutomationKindTrigger : t.chatAutomationKindAction
  const q = query.trim().toLowerCase()
  const groups = useMemo(() => {
    return catalogGroups(kind, presentTypes).map((group) => ({
      ...group,
      entries: q
        ? group.entries.filter((entry) => {
          const hay = `${entry.label(t)} ${entry.hint(t)}`.toLowerCase()
          return hay.includes(q)
        })
        : group.entries,
    })).filter((group) => group.entries.length > 0)
  }, [kind, presentTypes, q, t])

  return (
    <aside className="kv-automation-inspector kv-automation-picker custom-scrollbar" aria-label={title}>
      <header className="kv-automation-inspector-head">
        <div className="kv-automation-picker-heading">
          <div className="kv-automation-inspector-kicker">{kicker}</div>
          <h2 className="kv-automation-inspector-title">{title}</h2>
        </div>
        {onCancel ? (
          <IconButton size="sm" variant="ghost" label={t.cancel} onClick={onCancel}>
            <X size={14} />
          </IconButton>
        ) : null}
      </header>
      <label className="kv-automation-picker-search">
        <Search size={14} strokeWidth={2} aria-hidden />
        <input
          className="kv-input"
          value={query}
          onChange={(event) => setQuery(event.target.value)}
          placeholder={t.chatAutomationSearchNodes}
        />
      </label>
      <div className="kv-automation-picker-list">
        {groups.map((group) => (
          <section key={group.id} className="kv-automation-picker-group">
            {kind === 'action' ? (
              <header className="kv-automation-picker-group-head">
                <h3>{group.title(t)}</h3>
                <p>{group.hint(t)}</p>
              </header>
            ) : null}
            {group.entries.map((entry) => {
              const Icon = entry.icon
              return (
                <button
                  key={entry.type}
                  type="button"
                  className="kv-automation-picker-item"
                  onClick={() => onPick(entry)}
                >
                  <span className="kv-automation-picker-icon" aria-hidden>
                    <Icon size={16} strokeWidth={1.75} />
                  </span>
                  <span className="kv-automation-picker-copy">
                    <span className="kv-automation-picker-title">{entry.label(t)}</span>
                    <span className="kv-automation-picker-hint">{entry.hint(t)}</span>
                  </span>
                </button>
              )
            })}
          </section>
        ))}
      </div>
    </aside>
  )
}
