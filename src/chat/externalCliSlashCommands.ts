import type { SlashCommandDefinition } from './slashCommands'

export type ExternalCliSlashCommandDto = {
  name: string
  slash: string
  description?: string | null
  argumentHint?: string | null
}

export type ExternalCliSlashCommandsResult = {
  supportsSlashCommands: boolean
  commands: ExternalCliSlashCommandDto[]
  message?: string | null
}

function agentDisplayName(agentId: string): string {
  switch (agentId) {
    case 'claude':
      return 'Claude Code'
    case 'pi':
      return 'Pi'
    case 'opencode':
      return 'OpenCode'
    case 'codex':
      return 'Codex'
    case 'cursor':
      return 'Cursor Agent'
    case 'gemini':
      return 'Gemini CLI'
    case 'kimi':
      return 'Kimi CLI'
    case 'hermes':
      return 'Hermes'
    case 'grok':
      return 'Grok CLI'
    case 'dsh':
      return 'DeepSeek Harness'
    default:
      return agentId
  }
}

export function externalCliAgentLabel(agentId: string | null | undefined): string {
  const id = agentId?.trim().toLowerCase()
  if (!id) return 'CLI'
  return agentDisplayName(id)
}

export function mapExternalCliSlashCommands(
  agentId: string | null | undefined,
  commands: ExternalCliSlashCommandDto[],
): SlashCommandDefinition[] {
  const id = agentId?.trim().toLowerCase()
  if (!id) return []
  const category = agentDisplayName(id)
  return commands.map((command) => {
    const commandName = command.name.trim()
    const slash = command.slash.trim().startsWith('/')
      ? command.slash.trim()
      : `/${commandName}`
    return {
      id: `cli:${id}:${commandName}`,
      slash: slash as `/${string}`,
      title: slash,
      description: command.description?.trim() || `${category} CLI command`,
      category,
      keywords: [commandName, commandName.split(':').pop() ?? commandName],
      kind: 'cli',
      argumentHint: command.argumentHint?.trim() || undefined,
    }
  })
}
