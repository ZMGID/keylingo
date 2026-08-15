import Schema from '@deepseek-ai/schemastery'
import { HarnessSdkJsonRpcServer } from '@deepseek-ai/dsh-sdk-jsonrpc-server'
import { JsonRpcLineTransport } from '@deepseek-ai/dsh-sdk-protocol'

class KivioHarnessSdkJsonRpcServer extends HarnessSdkJsonRpcServer {
  async initialize(params) {
    const result = await super.initialize(params)
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
    const record = {
      handle: await this.ctx.agents.create({
        sessionId,
        meta: { cwd: this.cwd, agentPreset: preset },
        agentOptions: this.agentOptions(),
        setup: (agentCtx) => this.mountPreset(agentCtx, preset),
      }),
      resumed: false,
    }
    this.sessions.set(sessionId, record)
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
    return record
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
    record.handle.agent.cancel({ kind: 'user' })
    await record.handle.agent.whenIdle()
    return { sessionId, cancelled: true }
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

  async handleRequest(method, params) {
    switch (method) {
      case 'session/open':
        return this.open(params)
      case 'session/cancel':
        return this.cancel(params)
      case 'session/command':
        return this.command(params)
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

export const name = 'kivio-dsh-jsonrpc-bridge'
export const inject = ['agents', 'sessionPersistence', 'agentPresets', 'userQuestions', 'commands']
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
