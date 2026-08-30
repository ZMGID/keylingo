import { X } from 'lucide-react'
import { IconButton } from '../../components/Button'
import { useT } from '../../settings/i18n'
import { ACTION_CATALOG, TRIGGER_CATALOG, type NodeCatalogEntry } from './nodeCatalog'

export function AddNodePicker({
  kind,
  onPick,
  onCancel,
}: {
  kind: 'trigger' | 'action'
  onPick: (entry: NodeCatalogEntry) => void
  onCancel?: () => void
}) {
  const t = useT()
  const entries = kind === 'trigger' ? TRIGGER_CATALOG : ACTION_CATALOG
  const title = kind === 'trigger' ? t.chatAutomationAddTrigger : t.chatAutomationAddStep
  const kicker = kind === 'trigger' ? t.chatAutomationKindTrigger : t.chatAutomationKindAction
  return (
    <aside className="kv-automation-inspector kv-automation-picker" aria-label={title}>
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
      <div className="kv-automation-picker-list">
        {entries.map((entry) => {
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
      </div>
    </aside>
  )
}
