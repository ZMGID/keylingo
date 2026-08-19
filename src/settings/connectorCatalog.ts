// 连接器目录：curated 的外部数据源元数据。
// 每项最终物化成一条带 Authorization header 的 ChatMcpServer（见 ConnectorsPanel）。
// authKind:
//   - 'token'：用户粘贴 PAT/API key → headers.Authorization = 'Bearer <token>'（Phase A 已支持）。
//   - 'oauth'：OAuth 2.1 + PKCE 授权（Phase B 实现；Phase A 卡片连接按钮禁用）。
//   - 'vault'：本地笔记库路径（Obsidian）；写入 settings，注入系统提示，不走 MCP。

export type ConnectorAuthKind = 'oauth' | 'token' | 'vault'

export type ConnectorCatalogEntry = {
  id: string
  name: string
  /** 双语简介。 */
  description: { zh: string; en: string }
  /** NavIcons 之外的图标键，用于卡片渲染（见 ConnectorsPanel 的图标映射）。 */
  iconKey: string
  /** MCP（streamable_http）端点 URL；vault 类连接器可省略。 */
  url?: string
  authKind: ConnectorAuthKind
  /** 数据是否经第三方中转（Composio/Rube 等聚合服务）。 */
  composio?: boolean
  /** token 输入框的占位提示（双语）。 */
  tokenHint?: { zh: string; en: string }
  /** 详情面板「概览」要点（双语 bullet 列表）。 */
  overview?: { zh: string[]; en: string[] }
  /** 官网链接（详情面板「链接」区）。 */
  website?: string
  /** 支持/帮助链接（详情面板「链接」区）。 */
  support?: string
  /** 开发者/提供方名称（详情面板「开发者」区）。 */
  developer?: string
}

export const CONNECTOR_CATALOG: ConnectorCatalogEntry[] = [
  {
    id: 'obsidian',
    name: 'Obsidian',
    description: {
      zh: '告诉 AI 你的 Obsidian 笔记库本地路径，由 agent 用 read_file 等工具直接读取笔记。',
      en: 'Tell the agent where your Obsidian vault lives on disk; it reads notes via read_file and other native tools.',
    },
    iconKey: 'obsidian',
    authKind: 'vault',
    overview: {
      zh: [
        '选择本机 Obsidian 笔记库（vault）路径。',
        '路径写入设置并注入对话系统提示，无需 MCP。',
        'Agent 可用 read_file、glob_files、search_files 等工具检索与阅读 .md 笔记。',
      ],
      en: [
        'Pick your local Obsidian vault directory.',
        'The path is saved in settings and injected into the chat system prompt — no MCP.',
        'The agent can search and read .md notes with read_file, glob_files, search_files, etc.',
      ],
    },
    website: 'https://obsidian.md',
    support: 'https://help.obsidian.md',
    developer: 'Obsidian',
  },
  {
    id: 'notion',
    name: 'Notion',
    description: {
      zh: '读取与检索 Notion 页面、数据库。',
      en: 'Read and search Notion pages and databases.',
    },
    iconKey: 'notion',
    url: 'https://mcp.notion.com/mcp',
    authKind: 'oauth',
    overview: {
      zh: [
        '检索与读取 Notion 页面、数据库内容。',
        '按关键词在工作区内全文搜索。',
        '通过 OAuth 授权，无需手动管理 token。',
      ],
      en: [
        'Search and read Notion pages and database content.',
        'Full-text search across your workspace by keyword.',
        'Authorize via OAuth — no manual token management.',
      ],
    },
    website: 'https://www.notion.so',
    support: 'https://developers.notion.com',
    developer: 'Notion Labs, Inc.',
  },
  {
    id: 'github',
    name: 'GitHub',
    description: {
      zh: '访问仓库、Issue、PR 与代码搜索（使用 Personal Access Token）。',
      en: 'Access repos, issues, PRs, and code search (via Personal Access Token).',
    },
    iconKey: 'github',
    url: 'https://api.githubcopilot.com/mcp/',
    authKind: 'token',
    tokenHint: {
      zh: '粘贴 GitHub Personal Access Token（PAT）',
      en: 'Paste a GitHub Personal Access Token (PAT)',
    },
    overview: {
      zh: [
        '访问仓库、Issue、Pull Request 与代码搜索。',
        '读取文件内容、提交记录与分支信息。',
        '使用 Personal Access Token 鉴权，权限由 token scope 决定。',
      ],
      en: [
        'Access repositories, issues, pull requests, and code search.',
        'Read file contents, commit history, and branch info.',
        'Authenticated via a Personal Access Token; scope-limited.',
      ],
    },
    website: 'https://github.com',
    support: 'https://docs.github.com/copilot',
    developer: 'GitHub, Inc.',
  },
  {
    id: 'linear',
    name: 'Linear',
    description: {
      zh: '在 Linear 中管理项目、Issue 与工作流。',
      en: 'Manage projects, issues, and workflows in Linear.',
    },
    iconKey: 'linear',
    url: 'https://mcp.linear.app/mcp',
    authKind: 'oauth',
    overview: {
      zh: [
        '查询与管理 Linear 项目、Issue 与工作流。',
        '创建、更新 Issue 并跟踪状态变更。',
        '通过 OAuth 授权，无需手动管理 token。',
      ],
      en: [
        'Query and manage Linear projects, issues, and workflows.',
        'Create, update issues and track status changes.',
        'Authorize via OAuth — no manual token management.',
      ],
    },
    website: 'https://linear.app',
    support: 'https://linear.app/docs/mcp',
    developer: 'Linear',
  },
  {
    id: 'sentry',
    name: 'Sentry',
    description: {
      zh: '查询项目错误、Issue 与监控数据。',
      en: 'Query project errors, issues, and monitoring data.',
    },
    iconKey: 'sentry',
    url: 'https://mcp.sentry.dev/mcp',
    authKind: 'oauth',
    overview: {
      zh: [
        '查询项目错误、Issue 与监控数据。',
        '检索堆栈信息与错误发生趋势。',
        '通过 OAuth 授权，无需手动管理 token。',
      ],
      en: [
        'Query project errors, issues, and monitoring data.',
        'Inspect stack traces and error occurrence trends.',
        'Authorize via OAuth — no manual token management.',
      ],
    },
    website: 'https://sentry.io',
    support: 'https://github.com/getsentry/sentry-mcp',
    developer: 'Sentry',
  },
  {
    id: 'atlassian',
    name: 'Atlassian',
    description: {
      zh: '访问 Jira 工单与 Confluence 页面。',
      en: 'Access Jira issues and Confluence pages.',
    },
    iconKey: 'atlassian',
    url: 'https://mcp.atlassian.com/v1/mcp',
    authKind: 'oauth',
    overview: {
      zh: [
        '访问 Jira 工单并跟踪进度。',
        '检索与读取 Confluence 页面内容。',
        '通过 OAuth 授权，无需手动管理 token。',
      ],
      en: [
        'Access Jira issues and track their progress.',
        'Search and read Confluence page content.',
        'Authorize via OAuth — no manual token management.',
      ],
    },
    website: 'https://www.atlassian.com',
    support: 'https://www.atlassian.com/blog/announcements/remote-mcp-server',
    developer: 'Atlassian',
  },
  {
    id: 'composio',
    name: 'Composio',
    description: {
      zh: '通过 Composio 聚合接入 Gmail、Drive、Outlook 等长尾服务。数据经第三方中转，请确认 MCP 端点与 token。',
      en: 'Reach long-tail services (Gmail, Drive, Outlook…) through Composio. Data is relayed by a third party; confirm the MCP endpoint and token.',
    },
    iconKey: 'composio',
    // 占位端点：用户需自行填入/确认其 Composio (Rube) MCP 端点。
    url: 'https://mcp.composio.dev/mcp',
    authKind: 'token',
    composio: true,
    tokenHint: {
      zh: '粘贴 Composio API key / token',
      en: 'Paste your Composio API key / token',
    },
    overview: {
      zh: [
        '通过 Composio 聚合接入 Gmail、Drive、Outlook 等长尾服务。',
        '一个 token 复用多家服务，统一 MCP 端点。',
        '数据经第三方中转，请确认 MCP 端点与 token 来源可信。',
      ],
      en: [
        'Reach long-tail services (Gmail, Drive, Outlook…) through Composio.',
        'One token spans many services via a unified MCP endpoint.',
        'Data is relayed by a third party; verify the endpoint and token.',
      ],
    },
    website: 'https://composio.dev',
    support: 'https://docs.composio.dev',
    developer: 'Composio',
  },
]

/** 插件 MCP（`plugin-<id>` / `connectorId: plugin:<id>`）。
 *  连接器页排除；MCP 已安装列表与普通服务器一起展示，带「插件」标记，只读。 */
export function isPluginManagedServer(server: {
  id: string
  connectorId?: string | null
}): boolean {
  return server.id.startsWith('plugin-') || (server.connectorId?.startsWith('plugin:') ?? false)
}

/** MCP 页保存时钉住插件条目：开关 / 删除 / 改命令只走「扩展 → 插件」。 */
export function preservePluginManagedServers<T extends { id: string; connectorId?: string | null }>(
  previous: T[],
  next: T[],
): T[] {
  const locked = new Map(
    previous.filter(isPluginManagedServer).map((server) => [server.id, server] as const),
  )
  if (locked.size === 0) return next
  const out = next.map((server) => locked.get(server.id) ?? server)
  const present = new Set(out.map((server) => server.id))
  for (const [id, server] of locked) {
    if (!present.has(id)) out.push(server)
  }
  return out
}
