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
    const record = await super.createSession(sessionId)
    record.resumed = false
    return record
  }

  async resumeSession(sessionId) {
    const handle = await this.ctx.agents.resume({
      resumeSessionId: sessionId,
      agentOptions: this.agentOptions(),
    })
    const record = { handle, resumed: true }
    this.sessions.set(sessionId, record)
    return record
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

  async handleRequest(method, params) {
    switch (method) {
      case 'session/open':
        return this.open(params)
      case 'session/cancel':
        return this.cancel(params)
      default:
        return super.handleRequest(method, params)
    }
  }
}

function requireSessionId(value) {
  if (typeof value !== 'string' || value.trim() === '') {
    throw new TypeError('sessionId must be a non-empty string')
  }
  return value.trim()
}

export const name = 'kivio-dsh-jsonrpc-bridge'
export const inject = ['agents', 'sessionPersistence']
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
    transport.start()
    return async () => {
      await server.shutdown()
      transport.close()
    }
  }, 'kivio-jsonrpc.serve')
}
