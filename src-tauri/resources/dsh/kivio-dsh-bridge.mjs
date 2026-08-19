import Schema from '@deepseek-ai/schemastery'
import { HarnessSdkJsonRpcServer } from '@deepseek-ai/dsh-sdk-jsonrpc-server'
import { JsonRpcLineTransport } from '@deepseek-ai/dsh-sdk-protocol'

class KivioHarnessSdkJsonRpcServer extends HarnessSdkJsonRpcServer {
  async initialize(params) {
    const result = await super.initialize(params)
    // Session tools read header.cwd; sandbox-policy falls back to process.cwd()
    // when a header has none. Align the process with initialize.cwd so this
    // one-session process is actually rooted in the Kivio project directory.
    try {
      process.chdir(this.cwd)
    } catch {
      // header.cwd still drives tools even if chdir is refused.
    }
    return {
      ...result,
      serverInfo: {
        name: 'kivio-dsh-sdk-runtime',
        version: '1.0.0',
      },
      capabilities: {
        resume: true,
        cancel: true,
      },
    }
  }

  async open(params) {
    const sessionId = requireSessionId(params.sessionId)
    const resume = params.resume === true
    const existing = this.sessions.get(sessionId)
    if (existing) {
      return { sessionId, resumed: existing.resumed === true }
    }
    const pending = this.sessionCreations.get(sessionId)
    if (pending) {
      const record = await pending
      return { sessionId, resumed: record.resumed === true }
    }

    const creation = resume
      ? this.resumeSession(sessionId)
      : this.createFreshSession(sessionId)
    this.sessionCreations.set(sessionId, creation)
    try {
      const record = await creation
      return { sessionId, resumed: record.resumed === true }
    } finally {
      this.sessionCreations.delete(sessionId)
    }
  }

  async createFreshSession(sessionId) {
    const preset = this.agentPreset()
    const cwd = this.cwd
    const record = {
      handle: await this.ctx.agents.create({
        sessionId,
        meta: { cwd, agentPreset: preset },
        agentOptions: this.agentOptions(),
        setup: (agentCtx) => this.mountPreset(agentCtx, preset),
      }),
      resumed: false,
    }
    this.sessions.set(sessionId, record)
    await this.attachToWorkspace(sessionId, cwd)
    return record
  }

  async resumeSession(sessionId) {
    const handle = await this.ctx.agents.resume({
      resumeSessionId: sessionId,
      agentOptions: this.agentOptions(),
      setup: (agentCtx) => {
        const recorded = agentCtx.agent?.session?.header?.agentPreset
        return this.mountPreset(agentCtx, recorded || this.agentPreset())
      },
    })
    const record = { handle, resumed: true }
    this.sessions.set(sessionId, record)
    await this.attachToWorkspace(
      sessionId,
      handle.agent?.session?.header?.cwd || this.cwd,
    )
    return record
  }

  /**
   * Official web groups sessions by Host Workspace membership, not by
   * session.header.cwd. `agents.create({ meta.cwd })` alone leaves the
   * session in the ungrouped bucket ("其他项目" / 未分组). Mirror
   * session.create: ensure the directory is a workspace, then attach.
   *
   * Per-conversation Kivio workbenches stay ungrouped so they do not
   * flood the web sidebar with uuid folders.
   */
  async attachToWorkspace(sessionId, cwd) {
    if (!shouldAttachWorkspace(cwd)) return
    try {
      const workspace = await this.ctx.workspaceRegistry.create(cwd)
      await workspace.attachSession(sessionId)
    } catch (error) {
      // Grouping is best-effort; a failed attach must not drop the turn.
      console.error('[kivio-dsh] workspace attach failed:', error)
    }
  }

  agentPreset() {
    const raw = process.env.DSH_AGENT_PRESET
    if (raw === 'code' || raw === 'minimal' || raw === 'cordis') return raw
    return 'standard'
  }

  async mountPreset(agentCtx, presetId) {
    const presets = this.ctx.get('agentPresets')
    if (!presets) return
    await presets.mount(agentCtx, presetId)
  }

  agentOptions() {
    return {
      provider: this.provider,
      model: this.model,
      ...(this.maxTokens === undefined ? {} : { maxTokens: this.maxTokens }),
    }
  }

  async getOrCreateSession(sessionId) {
    const normalized = requireSessionId(sessionId)
    const existing = this.sessions.get(normalized)
    if (existing) return existing
    const pending = this.sessionCreations.get(normalized)
    if (pending) return pending
    throw new Error(`session "${normalized}" is not open; call session/open first`)
  }

  async cancel(params) {
    const sessionId = requireSessionId(params.sessionId)
    const record = this.sessions.get(sessionId)
    if (!record) throw new Error(`session "${sessionId}" is not open`)
    const agent = record.handle.agent
    return withExclusiveAgentCall(agent, async () => {
      agent.cancel({ kind: 'user' })
      await agent.whenIdle()
      return { sessionId, cancelled: true }
    })
  }

  async command(params) {
    const sessionId = requireSessionId(params.sessionId)
    const line = typeof params.line === 'string' ? params.line.trim() : ''
    if (!line.startsWith('/')) {
      throw new Error('session/command line must start with /')
    }
    const record = await this.getOrCreateSession(sessionId)
    const commands = this.ctx.commands
    if (!commands || typeof commands.execute !== 'function') {
      throw new Error('commands registry is not available')
    }
    const execution = await commands.execute(
      record.handle.agent,
      line,
      new AbortController().signal,
    )
    if (!execution) {
      throw new Error(`unregistered command: ${line}`)
    }
    return {
      commandId: execution.commandId,
      kind: execution.result.kind,
      text: execution.result.text ?? '',
    }
  }

  async listCommands(params) {
    const sessionId = requireSessionId(params.sessionId)
    const record = await this.getOrCreateSession(sessionId)
    const commands = this.ctx.commands
    if (!commands || typeof commands.list !== 'function') {
      return { commands: [] }
    }
    const listed = commands.list(record.handle.agent)
    return {
      commands: (Array.isArray(listed) ? listed : []).flatMap((command) => {
        const name = typeof command?.name === 'string' ? command.name.trim() : ''
        if (!name) return []
        const description =
          typeof command.description === 'string' ? command.description : ''
        const hint =
          typeof command.input?.hint === 'string'
            ? command.input.hint
            : typeof command.argumentHint === 'string'
              ? command.argumentHint
              : undefined
        return [
          {
            name,
            description,
            ...(hint ? { argumentHint: hint } : {}),
          },
        ]
      }),
    }
  }

  async stopJob(params) {
    const sessionId = requireSessionId(params.sessionId)
    const jobId = typeof params.jobId === 'string' ? params.jobId.trim() : ''
    if (!jobId) {
      throw new TypeError('jobId must be a non-empty string')
    }
    const record = await this.getOrCreateSession(sessionId)
    const agent = record.handle.agent
    const jobs = this.ctx.jobs
    if (jobs && typeof jobs.kill === 'function') {
      try {
        const outcome = jobs.kill(jobId, agent, 'kivio-stop-task')
        return { sessionId, jobId, outcome, target: 'job' }
      } catch {
        // Unknown job ids include childSessionId from subagent.started.
      }
    }
    let subagents
    try {
      subagents = this.ctx.subagents
    } catch {
      subagents = undefined
    }
    if (subagents && typeof subagents.interrupt === 'function') {
      try {
        subagents.interrupt(jobId, { kind: 'user', parentSessionId: sessionId })
        return { sessionId, jobId, outcome: 'requested', target: 'subagent' }
      } catch {
        // Fall through to a direct cancel of a live child agent.
      }
    }
    let child
    try {
      child = this.ctx.agents?.get?.(jobId)
    } catch {
      child = undefined
    }
    if (child && child !== agent && typeof child.cancel === 'function') {
      child.cancel({ kind: 'user' }, { keepInbox: true })
      return { sessionId, jobId, outcome: 'requested', target: 'agent' }
    }
    throw new Error(`unknown job or subagent: ${jobId}`)
  }

  async prompt(params) {
    const contentBlocks = await materializeContentBlocks(this.ctx, params?.contentBlocks)
    const patched = { ...params, contentBlocks }
    if (typeof params?.sessionId !== 'string' || params.sessionId.trim() === '') {
      return super.prompt(patched)
    }
    const record = await this.getOrCreateSession(params.sessionId)
    return withExclusiveAgentCall(record.handle.agent, () => super.prompt(patched))
  }

  // Official SDK only exposes session/prompt → agent.followup() (next turn).
  // dsh itself has agent.steer() (next-step inbox). Reuse prompt()'s
  // UserMessage construction by routing followup to steer for this one call.
  // @deepseek-ai/dsh-llm is a peer of the SDK server, not a kivio profile dep.
  // prompt() and steer() share a per-agent queue so a concurrent prompt cannot
  // run while followup is patched to steer.
  async steer(params) {
    const sessionId = requireSessionId(params.sessionId)
    const contentBlocks = await materializeContentBlocks(this.ctx, params?.contentBlocks)
    if (contentBlocks.length === 0) {
      throw new TypeError('session/steer requires contentBlocks')
    }
    const record = await this.getOrCreateSession(sessionId)
    if (this.ctx.agents.get(record.handle.agent.id) !== record.handle.agent) {
      throw new Error(`session agent was disposed outside the server: ${sessionId}`)
    }
    const agent = record.handle.agent
    return withExclusiveAgentCall(agent, async () => {
      const enqueueFollowup = agent.followup.bind(agent)
      agent.followup = agent.steer.bind(agent)
      try {
        return await super.prompt({ sessionId, contentBlocks })
      } finally {
        agent.followup = enqueueFollowup
      }
    })
  }

  async handleRequest(method, params) {
    switch (method) {
      case 'session/open':
        return this.open(params)
      case 'session/cancel':
        return this.cancel(params)
      case 'session/command':
        return this.command(params)
      case 'session/commands':
        return this.listCommands(params)
      case 'session/stop-job':
        return this.stopJob(params)
      case 'session/steer':
        return this.steer(params)
      default:
        return super.handleRequest(method, params)
    }
  }

  currentSessionId() {
    const ids = [...this.sessions.keys()]
    return ids.length === 0 ? '' : ids[ids.length - 1]
  }
}

function requireSessionId(value) {
  if (typeof value !== 'string' || value.trim() === '') {
    throw new TypeError('sessionId must be a non-empty string')
  }
  return value.trim()
}

const agentCallTails = new WeakMap()

function withExclusiveAgentCall(agent, fn) {
  const prev = agentCallTails.get(agent) ?? Promise.resolve()
  const next = prev.then(fn, fn)
  agentCallTails.set(
    agent,
    next.then(
      () => undefined,
      () => undefined,
    ),
  )
  return next
}

function shouldAttachWorkspace(cwd) {
  if (typeof cwd !== 'string' || cwd.trim() === '') return false
  const normalized = cwd.replace(/\\/g, '/')
  return !/(^|\/)chat-workspaces\//.test(normalized)
}

const IMAGE_MEDIA_TYPES = new Set(['image/png', 'image/jpeg', 'image/webp', 'image/gif'])

function normalizeImageMediaType(value) {
  const mediaType = typeof value === 'string' ? value.trim().toLowerCase() : ''
  if (mediaType === 'image/jpg') return 'image/jpeg'
  if (IMAGE_MEDIA_TYPES.has(mediaType)) return mediaType
  throw new TypeError(`unsupported image media type: ${value}`)
}

function decodeImageData(data) {
  if (typeof data !== 'string' || data.trim() === '') {
    throw new TypeError('image block data must be a base64 string')
  }
  return Uint8Array.from(Buffer.from(data, 'base64'))
}

async function materializeContentBlocks(ctx, blocks) {
  if (!Array.isArray(blocks)) return []
  const attachments = ctx.get('attachments')
  const out = []
  for (const block of blocks) {
    if (!block || block.type !== 'image') {
      out.push(block)
      continue
    }
    if (block.attachment && typeof block.attachment.attachmentId === 'string') {
      out.push(block)
      continue
    }
    if (!attachments || typeof attachments.saveImage !== 'function') {
      throw new Error('dsh image input requires the durable attachment service')
    }
    const mediaType = normalizeImageMediaType(block.mediaType || block.mimeType)
    const ref = await attachments.saveImage({
      data: decodeImageData(block.data),
      mediaType,
      ...(typeof block.name === 'string' && block.name.trim() !== ''
        ? { name: block.name.trim() }
        : {}),
    })
    out.push({ type: 'image', attachment: ref })
  }
  return out
}

export const name = 'kivio-dsh-jsonrpc-bridge'
export const inject = ['agents', 'sessionPersistence', 'agentPresets', 'userQuestions', 'commands', 'attachments', 'jobs', 'subagents', 'workspaceRegistry']
export const Config = Schema.object({
  maxTokensAsSuccess: Schema.boolean().default(false),
})

export function apply(ctx, config) {
  const rootFiber = ctx.root.fiber
  const input = config.input ?? process.stdin
  const output = config.output ?? process.stdout
  const exit = config.exit ?? ((code) => process.exit(code))
  const transport = new JsonRpcLineTransport(input, output)
  const server = new KivioHarnessSdkJsonRpcServer(ctx, transport, {
    maxTokensAsSuccess: config.maxTokensAsSuccess,
  })
  const parentPid = process.ppid
  let parentWatchdog
  let exitTask
  const disposeAndExit = () => {
    exitTask ??= (async () => {
      if (parentWatchdog) clearInterval(parentWatchdog)
      await Promise.allSettled([Promise.resolve().then(() => transport.flush())])
      await Promise.allSettled([Promise.resolve().then(() => rootFiber.dispose())])
      exit(0)
    })()
    return exitTask
  }

  // Tauri dev hot reloads and hard crashes can bypass Rust destructors. Exit after reparenting so
  // the dsh runtime flushes its persistent session instead of surviving as an orphan process.
  parentWatchdog = setInterval(() => {
    if (process.ppid <= 1 || process.ppid !== parentPid) void disposeAndExit()
  }, 1000)
  parentWatchdog.unref()
  // Pipe EOF is the cross-platform parent-death signal (Windows does not reparent to PID 1).
  // Defer disposal one turn so already-buffered shutdown/prompt handlers can settle first.
  const onInputClosed = () => setImmediate(() => void disposeAndExit())
  input.once('end', onInputClosed)
  input.once('error', onInputClosed)

  transport.onRequest(async (method, params) => {
    const result = await server.handleRequest(method, params)
    if (method === 'shutdown') setImmediate(() => disposeAndExit())
    return result
  })
  ctx.effect(() => {
    return ctx.userQuestions.registerProvider({
      ask: (request) => askViaHost(transport, server, request),
    })
  }, 'kivio-jsonrpc.user-questions')
  ctx.effect(() => {
    transport.start()
    return async () => {
      await server.shutdown()
      transport.close()
    }
  }, 'kivio-jsonrpc.serve')
}

function wireQuestions(questions) {
  if (!Array.isArray(questions)) return []
  return questions.map((question) => {
    const item = {
      id: question.id,
      question: question.question,
    }
    if (question.detail !== undefined) item.detail = question.detail
    if (question.header !== undefined) item.header = question.header
    if (question.options !== undefined) item.options = question.options
    if (question.multiSelect !== undefined) item.multiSelect = question.multiSelect
    if (question.intent !== undefined) item.intent = question.intent
    return item
  })
}

function asAskAnswer(result) {
  const answers = Array.isArray(result?.answers) ? result.answers : []
  return {
    answers: answers
      .map((item) => {
        const id = typeof item?.id === 'string' ? item.id : ''
        const selected = Array.isArray(item?.selected)
          ? item.selected.filter((label) => typeof label === 'string')
          : []
        const custom = typeof item?.custom === 'string' ? item.custom : undefined
        return custom === undefined ? { id, selected } : { id, selected, custom }
      })
      .filter((item) => item.id !== ''),
  }
}

async function askViaHost(transport, server, request) {
  const sessionId =
    typeof request.agent?.id === 'string' && request.agent.id.trim() !== ''
      ? request.agent.id.trim()
      : server.currentSessionId()
  if (!sessionId) {
    throw new Error('ask_user_question requires an agent-owned session')
  }
  const result = await transport.request(
    'session/ask',
    {
      sessionId,
      questions: wireQuestions(request.questions),
    },
    request.signal,
  )
  return asAskAnswer(result)
}
