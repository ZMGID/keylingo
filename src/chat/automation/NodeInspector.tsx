import type { ReactNode } from 'react'
import { Play } from 'lucide-react'
import { Button } from '../../components/Button'
import { FieldBlock, Select, Toggle } from '../../settings/components'
import { useT } from '../../settings/i18n'
import { catalogEntry } from './nodeCatalog'
import { AgentInspector } from './AgentInspector'
import { slotForNodeType } from './agentModel'
import {
  isAttachmentType,
  isTriggerType,
  MAX_SWITCH_CASES,
  type ClipboardOp,
  type CommandData,
  type FileOp,
  type FlowNode,
  type IfOp,
  type SetField,
  type SwitchCase,
} from './types'

function InspectorSection({ title, children }: { title: string, children: ReactNode }) {
  return (
    <section className="kv-automation-inspector-section">
      <h3>{title}</h3>
      {children}
    </section>
  )
}

export function NodeInspector({
  node,
  onChange,
  onExecuteStep,
  running,
  lastOutput,
}: {
  node: FlowNode
  onChange: (next: FlowNode) => void
  onExecuteStep?: () => void
  running?: boolean
  lastOutput?: string
}) {
  const t = useT()
  const entry = catalogEntry(node.type)
  const Icon = entry?.icon
  const patchData = (data: FlowNode['data']) => onChange({ ...node, data })
  const slot = slotForNodeType(node.type)
  const kind = isTriggerType(node.type)
    ? t.chatAutomationKindTrigger
    : String(node.type).startsWith('logic.')
      ? t.chatAutomationKindLogic
      : slot
        ? t.chatAutomationKindSlot
        : t.chatAutomationKindAction
  const usesTemplates = node.type === 'action.notify'
    || node.type === 'action.http'
    || node.type === 'action.set'
    || node.type === 'action.clipboard'
    || node.type === 'action.file'
    || node.type === 'action.command'
    || node.type === 'logic.if'
    || node.type === 'logic.switch'
  const hasParams = node.type === 'trigger.schedule'
    || node.type === 'trigger.hotkey'
    || node.type === 'action.notify'
    || node.type === 'action.http'
    || node.type === 'logic.if'
    || node.type === 'logic.switch'
    || node.type === 'action.set'
    || node.type === 'logic.delay'
    || node.type === 'action.clipboard'
    || node.type === 'action.file'
    || node.type === 'action.command'
  const setFields: SetField[] = node.data.set?.fields?.length
    ? node.data.set.fields
    : [{ key: '', value: '' }]
  const patchSetFields = (fields: SetField[]) => patchData({ ...node.data, set: { fields } })
  const command: CommandData = {
    command: node.data.command?.command ?? '',
    cwd: node.data.command?.cwd ?? '',
    timeoutSeconds: node.data.command?.timeoutSeconds ?? 30,
    continueOnFail: node.data.command?.continueOnFail ?? false,
  }
  const patchCommand = (next: Partial<CommandData>) =>
    patchData({ ...node.data, command: { ...command, ...next } })
  const switchCases: SwitchCase[] = node.data.switch?.cases?.length
    ? node.data.switch.cases
    : [{ id: '1', op: 'equals', value: '' }]
  const patchSwitchCases = (cases: SwitchCase[]) =>
    patchData({ ...node.data, switch: { cases } })
  const ifOpOptions = [
    { value: 'contains', label: t.chatAutomationIfContains },
    { value: 'equals', label: t.chatAutomationIfEquals },
    { value: 'notEmpty', label: t.chatAutomationIfNotEmpty },
  ]

  return (
    <aside className="kv-automation-inspector custom-scrollbar">
      <header className="kv-automation-inspector-head">
        {Icon ? (
          <span className="kv-automation-inspector-icon" aria-hidden>
            <Icon size={16} strokeWidth={1.75} />
          </span>
        ) : null}
        <div className="min-w-0">
          <div className="kv-automation-inspector-kicker">{kind}</div>
          <h2 className="kv-automation-inspector-title">{node.data.label}</h2>
        </div>
      </header>

      {entry ? (
        <p className="kv-automation-inspector-lead">{entry.hint(t)}</p>
      ) : null}

      <InspectorSection title={t.chatAutomationNodeName}>
        <input
          className="kv-input"
          value={node.data.label}
          placeholder={entry?.label(t) ?? ''}
          onChange={(event) => patchData({ ...node.data, label: event.target.value })}
        />
      </InspectorSection>

      {node.type === 'action.agent' ? (
        <p className="kv-automation-inspector-note">{t.chatAutomationAgentSlotsHint}</p>
      ) : null}

      {slot ? (
        <AgentInspector node={node} onChange={onChange} slot={slot} />
      ) : null}

      {hasParams ? (
        <InspectorSection title={t.chatAutomationSectionParams}>
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

          {node.type === 'action.notify' ? (
            <FieldBlock label={t.chatAutomationNotifyBody}>
              <textarea
                className="kv-textarea custom-scrollbar"
                rows={4}
                value={node.data.notify?.body ?? ''}
                placeholder={t.chatAutomationNotifyPlaceholder}
                onChange={(event) => patchData({ ...node.data, notify: { body: event.target.value } })}
              />
            </FieldBlock>
          ) : null}

          {node.type === 'action.http' ? (
            <>
              <FieldBlock label={t.chatAutomationHttpMethod}>
                <Select
                  value={node.data.http?.method ?? 'GET'}
                  onChange={(method) =>
                    patchData({
                      ...node.data,
                      http: {
                        method: method as 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE',
                        url: node.data.http?.url ?? '',
                        headers: node.data.http?.headers ?? '',
                        body: node.data.http?.body ?? '',
                      },
                    })
                  }
                  options={['GET', 'POST', 'PUT', 'PATCH', 'DELETE'].map((value) => ({ value, label: value }))}
                />
              </FieldBlock>
              <FieldBlock label={t.chatAutomationHttpUrl}>
                <input
                  className="kv-input"
                  value={node.data.http?.url ?? ''}
                  placeholder="https://example.com/hook"
                  onChange={(event) =>
                    patchData({
                      ...node.data,
                      http: {
                        method: node.data.http?.method ?? 'GET',
                        url: event.target.value,
                        headers: node.data.http?.headers ?? '',
                        body: node.data.http?.body ?? '',
                      },
                    })
                  }
                />
              </FieldBlock>
              <FieldBlock label={t.chatAutomationHttpHeaders}>
                <textarea
                  className="kv-textarea custom-scrollbar"
                  rows={3}
                  value={node.data.http?.headers ?? ''}
                  placeholder={'Authorization: Bearer …\nAccept: application/json'}
                  onChange={(event) =>
                    patchData({
                      ...node.data,
                      http: {
                        method: node.data.http?.method ?? 'GET',
                        url: node.data.http?.url ?? '',
                        headers: event.target.value,
                        body: node.data.http?.body ?? '',
                      },
                    })
                  }
                />
              </FieldBlock>
              {(node.data.http?.method ?? 'GET') !== 'GET' ? (
                <FieldBlock label={t.chatAutomationHttpBody}>
                  <textarea
                    className="kv-textarea custom-scrollbar"
                    rows={4}
                    value={node.data.http?.body ?? ''}
                    placeholder='{"text":"{{output}}"}'
                    onChange={(event) =>
                      patchData({
                        ...node.data,
                        http: {
                          method: node.data.http?.method ?? 'POST',
                          url: node.data.http?.url ?? '',
                          headers: node.data.http?.headers ?? '',
                          body: event.target.value,
                        },
                      })
                    }
                  />
                </FieldBlock>
              ) : null}
            </>
          ) : null}

          {node.type === 'logic.if' ? (
            <>
              <FieldBlock label={t.chatAutomationIfOp}>
                <Select
                  value={node.data.if?.op ?? 'contains'}
                  onChange={(op) =>
                    patchData({
                      ...node.data,
                      if: { op: op as IfOp, value: node.data.if?.value ?? '' },
                    })
                  }
                  options={ifOpOptions}
                />
              </FieldBlock>
              {(node.data.if?.op ?? 'contains') !== 'notEmpty' ? (
                <FieldBlock label={t.chatAutomationIfValue}>
                  <input
                    className="kv-input"
                    value={node.data.if?.value ?? ''}
                    onChange={(event) =>
                      patchData({
                        ...node.data,
                        if: { op: node.data.if?.op ?? 'contains', value: event.target.value },
                      })
                    }
                  />
                </FieldBlock>
              ) : null}
            </>
          ) : null}

          {node.type === 'logic.switch' ? (
            <FieldBlock label={t.chatAutomationSwitchCases}>
              <div className="flex flex-col gap-3">
                {switchCases.map((item, index) => (
                  <div key={item.id} className="flex flex-col gap-2">
                    <div className="kv-automation-inspector-kicker">{t.chatAutomationSwitchCase} {item.id}</div>
                    <Select
                      value={item.op}
                      onChange={(op) =>
                        patchSwitchCases(switchCases.map((row, i) =>
                          i === index ? { ...row, op: op as IfOp } : row,
                        ))
                      }
                      options={ifOpOptions}
                    />
                    {item.op !== 'notEmpty' ? (
                      <input
                        className="kv-input"
                        value={item.value}
                        placeholder={t.chatAutomationIfValue}
                        onChange={(event) =>
                          patchSwitchCases(switchCases.map((row, i) =>
                            i === index ? { ...row, value: event.target.value } : row,
                          ))
                        }
                      />
                    ) : null}
                    {switchCases.length > 1 ? (
                      <button
                        type="button"
                        className="kv-automation-inspector-note cursor-pointer border-0 bg-transparent p-0 text-left"
                        onClick={() => patchSwitchCases(switchCases.filter((_, i) => i !== index))}
                      >
                        {t.chatDelete}
                      </button>
                    ) : null}
                  </div>
                ))}
                {switchCases.length < MAX_SWITCH_CASES ? (
                  <Button
                    size="sm"
                    onClick={() => {
                      const used = new Set(switchCases.map((row) => row.id))
                      let n = 1
                      while (used.has(String(n))) n += 1
                      patchSwitchCases([...switchCases, { id: String(n), op: 'equals', value: '' }])
                    }}
                  >
                    {t.chatAutomationSwitchAddCase}
                  </Button>
                ) : null}
                <p className="kv-automation-inspector-note">{t.chatAutomationSwitchDefaultHint}</p>
              </div>
            </FieldBlock>
          ) : null}

          {node.type === 'action.set' ? (
            <FieldBlock label={t.chatAutomationSetFields}>
              <div className="flex flex-col gap-3">
                {setFields.map((field, index) => (
                  <div key={index} className="flex flex-col gap-2">
                    <input
                      className="kv-input"
                      value={field.key}
                      placeholder={t.chatAutomationSetKey}
                      onChange={(event) =>
                        patchSetFields(setFields.map((item, i) =>
                          i === index ? { ...item, key: event.target.value } : item,
                        ))
                      }
                    />
                    <input
                      className="kv-input"
                      value={field.value}
                      placeholder={t.chatAutomationSetValue}
                      onChange={(event) =>
                        patchSetFields(setFields.map((item, i) =>
                          i === index ? { ...item, value: event.target.value } : item,
                        ))
                      }
                    />
                    {setFields.length > 1 ? (
                      <button
                        type="button"
                        className="kv-automation-inspector-note cursor-pointer border-0 bg-transparent p-0 text-left"
                        onClick={() => patchSetFields(setFields.filter((_, i) => i !== index))}
                      >
                        {t.chatDelete}
                      </button>
                    ) : null}
                  </div>
                ))}
                <Button size="sm" onClick={() => patchSetFields([...setFields, { key: '', value: '' }])}>
                  {t.chatAutomationSetAdd}
                </Button>
              </div>
            </FieldBlock>
          ) : null}

          {node.type === 'logic.delay' ? (
            <FieldBlock label={t.chatAutomationDelaySeconds}>
              <input
                className="kv-input"
                type="number"
                min={1}
                max={600}
                value={node.data.delay?.seconds ?? 5}
                onChange={(event) =>
                  patchData({
                    ...node.data,
                    delay: {
                      seconds: Math.min(600, Math.max(1, Number(event.target.value) || 1)),
                    },
                  })
                }
              />
            </FieldBlock>
          ) : null}

          {node.type === 'action.clipboard' ? (
            <>
              <FieldBlock label={t.chatAutomationClipboardOp}>
                <Select
                  value={node.data.clipboard?.op ?? 'copy'}
                  onChange={(op) =>
                    patchData({
                      ...node.data,
                      clipboard: {
                        op: op as ClipboardOp,
                        text: node.data.clipboard?.text ?? '',
                      },
                    })
                  }
                  options={[
                    { value: 'copy', label: t.chatAutomationClipboardCopy },
                    { value: 'read', label: t.chatAutomationClipboardRead },
                  ]}
                />
              </FieldBlock>
              {(node.data.clipboard?.op ?? 'copy') === 'copy' ? (
                <FieldBlock label={t.chatAutomationClipboardText}>
                  <textarea
                    className="kv-textarea custom-scrollbar"
                    rows={4}
                    value={node.data.clipboard?.text ?? ''}
                    onChange={(event) =>
                      patchData({
                        ...node.data,
                        clipboard: {
                          op: node.data.clipboard?.op ?? 'copy',
                          text: event.target.value,
                        },
                      })
                    }
                  />
                </FieldBlock>
              ) : null}
            </>
          ) : null}

          {node.type === 'action.file' ? (
            <>
              <FieldBlock label={t.chatAutomationFileOp}>
                <Select
                  value={node.data.file?.op ?? 'write'}
                  onChange={(op) =>
                    patchData({
                      ...node.data,
                      file: {
                        op: op as FileOp,
                        path: node.data.file?.path ?? '',
                        content: node.data.file?.content ?? '',
                      },
                    })
                  }
                  options={[
                    { value: 'read', label: t.chatAutomationFileRead },
                    { value: 'write', label: t.chatAutomationFileWrite },
                  ]}
                />
              </FieldBlock>
              <FieldBlock label={t.chatAutomationFilePath}>
                <input
                  className="kv-input"
                  value={node.data.file?.path ?? ''}
                  onChange={(event) =>
                    patchData({
                      ...node.data,
                      file: {
                        op: node.data.file?.op ?? 'write',
                        path: event.target.value,
                        content: node.data.file?.content ?? '',
                      },
                    })
                  }
                />
              </FieldBlock>
              {(node.data.file?.op ?? 'write') === 'write' ? (
                <FieldBlock label={t.chatAutomationFileContent}>
                  <textarea
                    className="kv-textarea custom-scrollbar"
                    rows={4}
                    value={node.data.file?.content ?? ''}
                    onChange={(event) =>
                      patchData({
                        ...node.data,
                        file: {
                          op: node.data.file?.op ?? 'write',
                          path: node.data.file?.path ?? '',
                          content: event.target.value,
                        },
                      })
                    }
                  />
                </FieldBlock>
              ) : null}
            </>
          ) : null}

          {node.type === 'action.command' ? (
            <>
              <p className="kv-automation-inspector-note">{t.chatAutomationCommandRunHint}</p>
              <FieldBlock label={t.chatAutomationCommand}>
                <textarea
                  className="kv-textarea custom-scrollbar"
                  rows={5}
                  placeholder={t.chatAutomationCommandPlaceholder}
                  value={command.command}
                  onChange={(event) => patchCommand({ command: event.target.value })}
                />
              </FieldBlock>
              <FieldBlock label={t.chatAutomationCommandCwd}>
                <input
                  className="kv-input"
                  placeholder={t.chatAutomationCommandCwdPlaceholder}
                  value={command.cwd ?? ''}
                  onChange={(event) => patchCommand({ cwd: event.target.value })}
                />
              </FieldBlock>
              <FieldBlock label={t.chatAutomationCommandTimeout}>
                <input
                  className="kv-input"
                  type="number"
                  min={1}
                  max={300}
                  value={command.timeoutSeconds ?? 30}
                  onChange={(event) =>
                    patchCommand({
                      timeoutSeconds: Math.min(300, Math.max(1, Number(event.target.value) || 30)),
                    })
                  }
                />
              </FieldBlock>
              <div className="kv-automation-inspector-toggle">
                <span>{t.chatAutomationCommandContinueOnFail}</span>
                <Toggle
                  checked={command.continueOnFail ?? false}
                  onChange={(continueOnFail) => patchCommand({ continueOnFail })}
                  ariaLabel={t.chatAutomationCommandContinueOnFail}
                />
              </div>
            </>
          ) : null}

          {usesTemplates ? (
            <p className="kv-automation-inspector-note">{t.chatAutomationVarsHint}</p>
          ) : null}
        </InspectorSection>
      ) : null}

      <InspectorSection title={t.chatAutomationSectionRun}>
        <div className="kv-automation-inspector-toggle">
          <span>{node.data.disabled ? t.chatAutomationNodeSkipped : t.chatAutomationNodeActive}</span>
          <Toggle
            checked={!node.data.disabled}
            onChange={(enabled) => patchData({ ...node.data, disabled: !enabled })}
            ariaLabel={t.chatAutomationNodeActive}
          />
        </div>
        {onExecuteStep && !isAttachmentType(node.type) ? (
          <Button size="sm" onClick={onExecuteStep} disabled={running}>
            <Play size={14} />
            {t.chatAutomationExecuteStep}
          </Button>
        ) : null}
        {lastOutput ? (
          <div className="kv-automation-output">
            <div className="kv-automation-inspector-kicker">{t.chatAutomationLastOutput}</div>
            <pre className="custom-scrollbar">{lastOutput}</pre>
          </div>
        ) : null}
      </InspectorSection>

      {node.type === 'trigger.schedule' || node.type === 'trigger.hotkey' ? (
        <p className="kv-automation-inspector-foot">
          {t.chatAutomationEnableHint}
          {' '}
          {t.chatAutomationTrayOnly}
        </p>
      ) : null}
    </aside>
  )
}
