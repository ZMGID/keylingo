import { FieldBlock, Select } from '../../settings/components'
import { useT } from '../../settings/i18n'
import { catalogEntry } from './nodeCatalog'
import type { FlowNode } from './types'

export function NodeInspector({
  node,
  onChange,
}: {
  node: FlowNode
  onChange: (next: FlowNode) => void
}) {
  const t = useT()
  const entry = catalogEntry(node.type)
  const Icon = entry?.icon
  const patchData = (data: FlowNode['data']) => onChange({ ...node, data })
  const kind = node.type.startsWith('trigger.')
    ? t.chatAutomationKindTrigger
    : t.chatAutomationKindAction

  return (
    <aside className="kv-automation-inspector">
      <div className="kv-automation-inspector-head">
        {Icon ? (
          <span className="kv-automation-inspector-icon" aria-hidden>
            <Icon size={16} strokeWidth={1.75} />
          </span>
        ) : null}
        <div className="min-w-0">
          <div className="kv-automation-inspector-kicker">{kind}</div>
          <h2 className="kv-automation-inspector-title">{node.data.label}</h2>
        </div>
      </div>

      {node.type === 'trigger.manual' ? (
        <p className="kv-automation-inspector-copy">{t.chatAutomationTriggerManualHint}</p>
      ) : null}

      {node.type === 'trigger.schedule' ? (
        <>
          <FieldBlock label={t.chatAutomationScheduleKind}>
            <Select
              value={node.data.schedule?.kind ?? 'daily'}
              onChange={(kindValue) =>
                patchData({
                  ...node.data,
                  schedule: {
                    kind: kindValue as 'daily' | 'weekdays' | 'interval',
                    hour: node.data.schedule?.hour ?? 9,
                    minute: node.data.schedule?.minute ?? 0,
                    intervalMinutes: node.data.schedule?.intervalMinutes ?? 60,
                  },
                })
              }
              options={[
                { value: 'daily', label: t.chatAutomationScheduleDaily },
                { value: 'weekdays', label: t.chatAutomationScheduleWeekdays },
                { value: 'interval', label: t.chatAutomationScheduleInterval },
              ]}
            />
          </FieldBlock>
          {node.data.schedule?.kind === 'interval' ? (
            <FieldBlock label={t.chatAutomationScheduleEvery}>
              <input
                className="kv-input"
                type="number"
                min={5}
                max={1440}
                value={node.data.schedule.intervalMinutes}
                onChange={(event) =>
                  patchData({
                    ...node.data,
                    schedule: {
                      ...node.data.schedule!,
                      intervalMinutes: Number(event.target.value) || 60,
                    },
                  })
                }
              />
            </FieldBlock>
          ) : (
            <FieldBlock label={t.chatAutomationScheduleTime}>
              <div className="kv-automation-time">
                <input
                  className="kv-input"
                  type="number"
                  min={0}
                  max={23}
                  value={node.data.schedule?.hour ?? 9}
                  onChange={(event) =>
                    patchData({
                      ...node.data,
                      schedule: {
                        kind: node.data.schedule?.kind ?? 'daily',
                        hour: Number(event.target.value) || 0,
                        minute: node.data.schedule?.minute ?? 0,
                        intervalMinutes: node.data.schedule?.intervalMinutes ?? 60,
                      },
                    })
                  }
                />
                <span>:</span>
                <input
                  className="kv-input"
                  type="number"
                  min={0}
                  max={59}
                  value={node.data.schedule?.minute ?? 0}
                  onChange={(event) =>
                    patchData({
                      ...node.data,
                      schedule: {
                        kind: node.data.schedule?.kind ?? 'daily',
                        hour: node.data.schedule?.hour ?? 9,
                        minute: Number(event.target.value) || 0,
                        intervalMinutes: node.data.schedule?.intervalMinutes ?? 60,
                      },
                    })
                  }
                />
              </div>
            </FieldBlock>
          )}
        </>
      ) : null}

      {node.type === 'trigger.hotkey' ? (
        <FieldBlock label={t.chatAutomationHotkey}>
          <input
            className="kv-input"
            value={node.data.hotkey?.accelerator ?? ''}
            placeholder="Control+Shift+A"
            onChange={(event) =>
              patchData({ ...node.data, hotkey: { accelerator: event.target.value } })
            }
          />
        </FieldBlock>
      ) : null}

      {node.type === 'action.agent' ? (
        <FieldBlock label={t.chatAutomationNodePrompt}>
          <textarea
            className="kv-textarea"
            rows={8}
            value={node.data.agent?.prompt ?? ''}
            placeholder={t.chatAutomationNodePromptPlaceholder}
            onChange={(event) =>
              patchData({
                ...node.data,
                agent: { prompt: event.target.value, skillId: node.data.agent?.skillId ?? null },
              })
            }
          />
        </FieldBlock>
      ) : null}

      {node.type === 'action.notify' ? (
        <FieldBlock label={t.chatAutomationNotifyBody}>
          <textarea
            className="kv-textarea"
            rows={4}
            value={node.data.notify?.body ?? ''}
            placeholder={t.chatAutomationNotifyPlaceholder}
            onChange={(event) => patchData({ ...node.data, notify: { body: event.target.value } })}
          />
        </FieldBlock>
      ) : null}

      {node.type.startsWith('trigger.') && node.type !== 'trigger.manual' ? (
        <p className="kv-automation-inspector-note">{t.chatAutomationEnableHint}</p>
      ) : null}

      {node.type === 'trigger.schedule' || node.type === 'trigger.hotkey' ? (
        <p className="kv-automation-inspector-note">{t.chatAutomationTrayOnly}</p>
      ) : null}
    </aside>
  )
}
