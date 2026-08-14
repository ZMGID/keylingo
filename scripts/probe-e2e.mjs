#!/usr/bin/env node
/**
 * 外部 CLI（claude）常驻会话改造的端到端验收套件。
 *
 * 驱动 debug 构建的无头测试通道（`src-tauri/src/chat/probe.rs`）：写
 * `<app_data>/chat_probe/request.json` → 等 `result-<id>.json`。走的是**聊天窗口完全相同**的
 * 生成路径，所以断言到的是生产行为，不是某个模块的近似。
 *
 * 先起 app（另一个终端）：
 *   npm run dev            # 等 stderr 出现 `[chat-probe] watching …`
 * 再跑：
 *   npm run probe:e2e                     # 全部场景
 *   npm run probe:e2e -- --list           # 只列场景
 *   npm run probe:e2e -- multi-turn cancel   # 按名字子串过滤
 *   npm run probe:e2e -- --agent codex    # 换外部 CLI（断言是按 claude 写的，仅供探索）
 *
 * 纪律（spec `guides/external-cli-agents.md` 第 15 条）：**环境问题诚实 skip，不 fail**。
 * claude 未安装 / 未登录 / 额度耗尽 / app 没在跑 —— 一律打印可操作排查提示并 skip，
 * 否则一个过期的凭据会伪装成代码回归。
 *
 * 注意：跑套件期间**不要改仓库里的文件**。`tauri dev` 的 watcher 会重建并重启 app，
 * 正在进行的那一轮会连 result 都写不出来（表现为「300000ms 内没拿到 result-…json」）。
 */

import fs from 'node:fs'
import os from 'node:os'
import path from 'node:path'
import { spawnSync } from 'node:child_process'

const APP_IDENTIFIER = 'com.zmair.kivio'
/**
 * 首次就绪握手的等待上限（秒）。默认 60s 够覆盖「app 正在启动」；`npm run dev` 还要先编译
 * Rust，那种情况用 `--wait 600` 让脚本一直等。等不到就 skip（不是 fail）。
 */
const READY_TIMEOUT_MS = Number(argValueEarly('--wait', '60')) * 1000
/** request.json 被消费的等待上限（app 已就绪之后）。 */
const CONSUME_TIMEOUT_MS = 60_000
/** 单轮生成的等待上限。Rust 侧硬超时是 360s，这里留点余量。 */
const TURN_TIMEOUT_MS = 300_000
const POLL_MS = 250

function argValueEarly(name, fallback) {
  const raw = process.argv.slice(2)
  const idx = raw.indexOf(name)
  return idx >= 0 && raw[idx + 1] ? raw[idx + 1] : fallback
}

// ---------------------------------------------------------------------------------------------
// CLI 参数
// ---------------------------------------------------------------------------------------------

const argv = process.argv.slice(2)
const flags = new Set(argv.filter((a) => a.startsWith('--')))
const filters = argv.filter((a) => !a.startsWith('--'))

function argValue(name, fallback) {
  const idx = argv.indexOf(name)
  return idx >= 0 && argv[idx + 1] ? argv[idx + 1] : fallback
}

const AGENT = argValue('--agent', 'claude')
const VERBOSE = flags.has('--verbose')

function resolveProbeDir() {
  const override = process.env.KIVIO_PROBE_DIR || argValue('--probe-dir', null)
  if (override) return override
  if (process.platform === 'win32') {
    const base = process.env.APPDATA || path.join(os.homedir(), 'AppData', 'Roaming')
    return path.join(base, APP_IDENTIFIER, 'chat_probe')
  }
  if (process.platform === 'darwin') {
    return path.join(os.homedir(), 'Library', 'Application Support', APP_IDENTIFIER, 'chat_probe')
  }
  const base = process.env.XDG_CONFIG_HOME || path.join(os.homedir(), '.config')
  return path.join(base, APP_IDENTIFIER, 'chat_probe')
}

const PROBE_DIR = resolveProbeDir()
const REQUEST_PATH = path.join(PROBE_DIR, 'request.json')
const RUN_ID = `${Date.now().toString(36)}${Math.floor(Math.random() * 1e4)}`
/**
 * 所有场景共用**同一个** cwd。
 *
 * 它是常驻会话注册表的复用判据之一（`LiveSession::is_reusable` 比 cwd），而 probe 的
 * 「Chat Probe」项目根是全局共享的、每次请求都会被改写 —— 中途换 cwd 等于把已建立的
 * 常驻会话判成不可复用，"同一个 pid" 这条断言会莫名其妙地红。
 */
const WORKDIR = path.join(os.tmpdir(), 'kivio-probe-e2e')
const SENTINEL_NAME = 'sentinel.txt'
const SENTINEL_TEXT = `KIVIO-PROBE-SENTINEL-${RUN_ID}`

// ---------------------------------------------------------------------------------------------
// 输出
// ---------------------------------------------------------------------------------------------

const C = process.stdout.isTTY
  ? { dim: '\x1b[2m', red: '\x1b[31m', green: '\x1b[32m', yellow: '\x1b[33m', cyan: '\x1b[36m', off: '\x1b[0m' }
  : { dim: '', red: '', green: '', yellow: '', cyan: '', off: '' }

const log = (...a) => console.log(...a)
const info = (...a) => console.log(`${C.dim}   ${a.join(' ')}${C.off}`)

function fmt(value) {
  if (value === undefined) return 'undefined'
  if (typeof value === 'string') return JSON.stringify(value.length > 400 ? `${value.slice(0, 400)}…` : value)
  return JSON.stringify(value, null, 2)
}

/** 断言失败 = 代码问题。消息里必须带**实际拿到的值**，否则定位还得再跑一遍。 */
class AssertError extends Error {}
/** 环境问题（未登录 / 未安装 / 限流）——诚实 skip。 */
class EnvError extends Error {}
/** app 没在跑 —— 整个套件 skip。 */
class AppDownError extends Error {}

function check(cond, message, actual) {
  if (!cond) throw new AssertError(`${message}\n     实际：${fmt(actual)}`)
}

/**
 * 软断言：不达标只 WARN，不判红。
 *
 * 用于**受模型延迟支配**的量（如「第 2 轮更快」）：常驻省下的是约 3.2s 的进程冷启动，
 * 而一轮的总耗时里模型生成本身是几秒到几十秒的抖动，硬断言必然偶发假红。
 * 「常驻真的生效了」这条由 `childPid` 相同 + `turnsServed` 递增硬断言，那两个不会抖。
 */
const warnings = []
function soft(cond, message, actual) {
  if (!cond) {
    warnings.push(message)
    log(`   ${C.yellow}WARN${C.off} ${message}`)
    info(`实际：${fmt(actual)}`)
  }
}

// ---------------------------------------------------------------------------------------------
// probe 通道
// ---------------------------------------------------------------------------------------------

const sleep = (ms) => new Promise((r) => setTimeout(r, ms))

async function waitUntil(predicate, timeoutMs) {
  const deadline = Date.now() + timeoutMs
  for (;;) {
    const value = predicate()
    if (value) return value
    if (Date.now() > deadline) return null
    await sleep(POLL_MS)
  }
}

function writeRequest(payload) {
  // 先写临时文件再 rename：watcher 是「mtime 变了就读」，直接写会被读到半截 JSON。
  const tmp = `${REQUEST_PATH}.tmp`
  fs.writeFileSync(tmp, JSON.stringify(payload, null, 2))
  fs.renameSync(tmp, REQUEST_PATH)
}

/**
 * 零成本就绪握手：故意发一个**缺 `prompt`** 的请求。
 *
 * watcher 会消费它、写一条 `invalid request.json` 的 result，**不会**拉起任何 CLI、
 * 不花 token。用真实请求探活的话，"app 没起来" 与 "第一轮很慢" 就分不开了。
 */
async function waitForApp() {
  const dirReady = await waitUntil(() => fs.existsSync(PROBE_DIR), READY_TIMEOUT_MS)
  if (!dirReady) {
    throw new AppDownError(`probe 目录不存在：${PROBE_DIR}`)
  }
  writeRequest({ __readiness_handshake__: true })
  const consumed = await waitUntil(() => !fs.existsSync(REQUEST_PATH), READY_TIMEOUT_MS)
  if (!consumed) {
    // 别把握手请求留在原地：下次 app 起来会消费它并写一条无 id 的错误 result，白添噪音。
    fs.rmSync(REQUEST_PATH, { force: true })
    throw new AppDownError('request.json 一直没被消费')
  }
}

const ENV_SIGNATURES = [
  /请先登录/,
  /claude \/login/i,
  /\bnot logged in\b/i,
  /\blogin\b/i,
  /authenticat/i,
  /oauth/i,
  /unauthorized/i,
  /invalid api key/i,
  /credit balance/i,
  /rate.?limit/i,
  /quota/i,
  /overloaded/i,
  /未安装/,
  /没有可用的/,
  /无法启动/,
  /ENOENT/,
]

/** 这条失败是环境问题还是代码问题？只看 error 与「失败轮」的正文，不看正常回答。 */
function envFailureReason(result) {
  const haystack = [result.error || '', result.streamOutcome === 'error' ? result.answer || '' : '']
    .join('\n')
    .trim()
  if (!haystack) return null
  const hit = ENV_SIGNATURES.find((re) => re.test(haystack))
  return hit ? haystack.slice(0, 600) : null
}

let turnSeq = 0

/**
 * 跑一轮。返回 probe 的 result 对象。
 *
 * `req` 里除 `prompt` 外常用：`conversationId`（续聊）、`cancelAfterMs`（取消）、
 * `computeContextStats`（回传分子/分母）、`externalSandbox` 等（触发指纹重连）。
 */
async function turn(scenario, req) {
  turnSeq += 1
  const id = `e2e-${scenario}-${turnSeq}`
  const payload = {
    id,
    cwd: WORKDIR,
    // 只在新建会话时有意义；续聊会话由后端忽略（有消息的外部会话禁切运行时）。
    externalAgentId: AGENT,
    ...req,
  }
  const resultPath = path.join(PROBE_DIR, `result-${id}.json`)
  fs.rmSync(resultPath, { force: true })

  if (VERBOSE) info(`→ ${JSON.stringify(payload)}`)
  writeRequest(payload)

  const consumed = await waitUntil(() => !fs.existsSync(REQUEST_PATH), CONSUME_TIMEOUT_MS)
  if (!consumed) throw new AppDownError('request.json 没被消费（app 退出了？）')

  const result = await waitUntil(() => {
    if (!fs.existsSync(resultPath)) return null
    try {
      // 写 result 不是原子的，读到半截 JSON 就再等一轮。
      const parsed = JSON.parse(fs.readFileSync(resultPath, 'utf8'))
      return parsed.id === id ? parsed : null
    } catch {
      return null
    }
  }, TURN_TIMEOUT_MS)
  if (!result) throw new AssertError(`${TURN_TIMEOUT_MS}ms 内没拿到 ${path.basename(resultPath)}`)

  const live = result.liveSession || {}
  log(
    `   ${C.cyan}turn${C.off} ${id} ${result.streamOutcome || '(no outcome)'} ` +
      `${result.durationMs}ms pid=${live.childPid ?? '-'} turns=${live.turnsServed ?? '-'} ` +
      `tools=${(result.toolCalls || []).length}`,
  )
  if (VERBOSE) info(`answer: ${fmt(result.answer)}`)

  const envReason = envFailureReason(result)
  if (envReason) throw new EnvError(envReason)
  return result
}

const answerOf = (r) => (r.answer || '').trim()

// ---------------------------------------------------------------------------------------------
// 场景
// ---------------------------------------------------------------------------------------------

const SCENARIOS = [
  {
    name: 'baseline',
    desc: '单轮基线：能回复、streamOutcome=completed、注册表里有一个活着的常驻进程',
    async run() {
      const r = await turn('baseline', { prompt: 'Reply with exactly one word: pong' })
      check(r.streamOutcome === 'completed', 'streamOutcome 应为 completed', r.streamOutcome)
      check(answerOf(r).length > 0, '回答不能为空', r.answer)
      check(r.liveSession.registered, '注册表里应有这个会话的常驻条目', r.liveSession)
      check(r.liveSession.alive, '常驻会话的 actor 应还活着', r.liveSession)
      check(typeof r.liveSession.childPid === 'number', '应能拿到常驻子进程 pid', r.liveSession)
      check(r.liveSession.turnsServed === 1, '首轮的 turnsServed 应为 1', r.liveSession)
    },
  },

  {
    name: 'multi-turn',
    desc: '多轮延续：记得上一轮的数字、同一个 pid、turnsServed 递增',
    async run() {
      const t1 = await turn('multi-turn', {
        prompt: 'Answer with one word only. Remember the number 42.',
      })
      check(t1.streamOutcome === 'completed', '第 1 轮应正常完成', t1.streamOutcome)
      const pid1 = t1.liveSession.childPid
      check(typeof pid1 === 'number', '第 1 轮应拿到 pid', t1.liveSession)

      const t2 = await turn('multi-turn', {
        conversationId: t1.conversationId,
        prompt: 'What number did I ask you to remember? Reply with just the number.',
      })
      check(t2.streamOutcome === 'completed', '第 2 轮应正常完成', t2.streamOutcome)
      check(answerOf(t2).includes('42'), '第 2 轮没记住第 1 轮的内容 ⇒ 不是同一个活会话', t2.answer)
      check(
        t2.liveSession.childPid === pid1,
        '第 2 轮换了进程 ⇒ 常驻没生效（应复用同一个子进程）',
        { turn1Pid: pid1, turn2Pid: t2.liveSession.childPid },
      )
      check(t2.liveSession.turnsServed === 2, '同一进程服了 2 轮，turnsServed 应为 2', t2.liveSession)
      soft(t2.durationMs < t1.durationMs, '第 2 轮没有比第 1 轮快（常驻省掉的是约 3.2s 冷启动）', {
        turn1Ms: t1.durationMs,
        turn2Ms: t2.durationMs,
      })
    },
  },

  {
    name: 'cancel-resume',
    desc: '取消后同一会话继续可用：取消轮 cancelled，下一轮正常且上下文没丢、进程还是原来那个',
    async run() {
      const warm = await turn('cancel-resume', {
        prompt: 'Answer with one word only. Remember the number 77.',
      })
      check(warm.streamOutcome === 'completed', '热身轮应正常完成', warm.streamOutcome)
      const pid = warm.liveSession.childPid

      const cancelled = await turn('cancel-resume', {
        conversationId: warm.conversationId,
        prompt:
          'Write a long, detailed 800-word essay about the history of clouds in art. Start immediately.',
        // 计时从本轮 generation 登记起算（见 probe_runtime::schedule_probe_cancel）。
        cancelAfterMs: 5000,
      })
      check(
        cancelled.streamOutcome === 'cancelled',
        '被取消的轮次必须走 cancelled 出口（error 说明多弹了一个假错误气泡）',
        { streamOutcome: cancelled.streamOutcome, answerHead: answerOf(cancelled).slice(0, 200) },
      )
      check(
        cancelled.liveSession.registered && cancelled.liveSession.alive,
        '取消之后常驻会话应保留在注册表里（否则点一次停止就丢掉整个会话上下文）',
        cancelled.liveSession,
      )

      const after = await turn('cancel-resume', {
        conversationId: warm.conversationId,
        prompt: 'What number did I ask you to remember earlier? Reply with just the number.',
      })
      check(after.streamOutcome === 'completed', '取消之后的下一轮应正常完成', after.streamOutcome)
      check(answerOf(after).includes('77'), '取消把会话上下文一起丢了', after.answer)
      check(after.liveSession.childPid === pid, '取消之后换了进程 ⇒ 常驻的核心收益没了', {
        beforeCancelPid: pid,
        afterCancelPid: after.liveSession.childPid,
      })
    },
  },

  {
    name: 'context-window',
    desc: '分母来自 CLI 实报：contextWindowTokens 非空、来源标记为 cli_reported',
    async run() {
      const r = await turn('context-window', {
        prompt: 'Reply with exactly one word: ok',
        computeContextStats: true,
      })
      check(r.streamOutcome === 'completed', '本轮应正常完成', r.streamOutcome)
      check(!!r.contextState, 'computeContextStats 应回传 contextState', r)
      const cs = r.contextState
      check(
        cs.contextSource === 'external_cli',
        'contextSource 应为 external_cli（否则会话没走外部 CLI 路径）',
        cs,
      )
      check(
        typeof cs.contextWindowTokens === 'number' && cs.contextWindowTokens > 0,
        '分母必须非空且 > 0（拿不到窗口时外部路径返回 null，绝不兜底 200K）',
        cs,
      )
      check(
        cs.contextWindowTokens >= 200_000,
        'claude 的真实窗口不可能小于 200K —— 这个值像是个假分母',
        cs,
      )
      check(
        cs.tokenCountSource === 'cli_reported',
        'token 计数来源应为 cli_reported（estimated 说明分子/分母退回了估算）',
        cs,
      )
      if (r.usage && r.usage.contextWindow != null) {
        check(
          r.usage.contextWindow === cs.contextWindowTokens,
          '消息上的实报窗口与会话 contextState 的分母不一致',
          { usageContextWindow: r.usage.contextWindow, contextWindowTokens: cs.contextWindowTokens },
        )
      }
      info(`窗口 = ${cs.contextWindowTokens}，已用 = ${cs.estimatedInputTokens}，比例 = ${cs.usageRatio}`)
    },
  },

  {
    name: 'cache-in-numerator',
    desc: '分子含 cache：Anthropic 口径下 cache 与 input 不相交，total 必须大于 input+output',
    async run() {
      const t1 = await turn('cache-in-numerator', {
        prompt: 'Answer with one word only. Remember the number 13.',
      })
      check(t1.streamOutcome === 'completed', '第 1 轮应正常完成', t1.streamOutcome)
      const t2 = await turn('cache-in-numerator', {
        conversationId: t1.conversationId,
        prompt: 'What number did I ask you to remember? Reply with just the number.',
      })
      check(t2.streamOutcome === 'completed', '第 2 轮应正常完成', t2.streamOutcome)
      const u = t2.usage
      check(!!u, 'assistant 消息应带 usage', t2)
      const input = u.input ?? 0
      const output = u.output ?? 0
      const cache = (u.cachedInput ?? 0) + (u.cacheCreation ?? 0)
      check(cache > 0, 'claude 这一轮没有任何 cache token —— 这条断言无从成立，请人工复核', u)
      check(
        typeof u.total === 'number' && u.total > input + output,
        'total 没把 cache 算进去（只读 input+output 会低估一个数量级）',
        u,
      )
      soft(u.total === input + output + cache, 'total 与 input+output+cache 的加法对账不严格相等', u)
      info(`input=${input} output=${output} cache=${cache} total=${u.total}`)
    },
  },

  {
    name: 'context-usage-per-turn',
    desc: '外部 CLI 的占用一轮只更新一次：轮中不推实时，轮末权威值到位',
    async run() {
      // 连着读同一个文件三次：一轮里多次 LLM 往返 —— 以前正是这种轮次会推一串实时数字。
      const r = await turn('context-usage-per-turn', {
        prompt:
          `Use the Read tool to read ${SENTINEL_NAME} in the current working directory three ` +
          `times in a row (three separate Read calls), then reply with its contents once.`,
        computeContextStats: true,
      })
      check(r.streamOutcome === 'completed', '本轮应正常完成', r.streamOutcome)
      // 曾经这里断言「实时至少推一次」。那条通道已删：分子是单次请求快照（工具循环里每请求
      // 一变、压缩后还会掉）、分母是上一轮的粘滞值、分段是前端缩放出来的 —— 三分之二不是真值，
      // 而这个数字唯一能驱动的动作（压缩 / 换会话）只发生在两轮之间。改回来先想清楚这三点。
      const ticks = r.liveUsageTicks || []
      check(
        ticks.length === 0,
        '外部 CLI 又开始在轮中推实时占用了（分子是单次请求快照，会跳）',
        ticks,
      )
      const cs = r.contextState
      check(!!cs, '轮末应有权威上下文快照（computeContextStats 打开了）', r)
      check(
        typeof cs.estimatedInputTokens === 'number' && cs.estimatedInputTokens > 0,
        '轮末权威分子应 > 0 —— 这是用量条现在唯一的数据来源',
        cs,
      )
      check(
        typeof cs.contextWindowTokens === 'number' && cs.contextWindowTokens > 0,
        '轮末权威分母缺失 ⇒ 用量条只能显示「满度未知」',
        cs,
      )
      soft(
        cs.tokenCountSource === 'cli_reported',
        '轮末分子不是 CLI 实报口径（外部 CLI 不该退回估算）',
        cs,
      )
      info(`轮末权威值 ${cs.estimatedInputTokens} / ${cs.contextWindowTokens}（来源 ${cs.tokenCountSource}）`)
    },
  },

  {
    name: 'slash-passthrough',
    desc: '客户端斜杠命令的输出不被吞：/cost 返回真实报表而不是「命令已执行」',
    async run() {
      const r = await turn('slash-passthrough', { prompt: '/cost' })
      const answer = answerOf(r)
      check(r.streamOutcome === 'completed', '/cost 应正常完成', r.streamOutcome)
      check(
        !/命令已执行/.test(answer),
        '/cost 的报告被吞了，只剩兜底文案（system/local_command_output 与 result 兜底都没接上）',
        answer,
      )
      check(answer.length > 20, '/cost 的输出太短，不像一份报表', answer)
      check(
        /cost|token|duration|usd|\$|时长|费用/i.test(answer),
        '/cost 的输出里看不到任何费用/用量字样',
        answer,
      )
    },
  },

  {
    name: 'zero-usage-numerator',
    desc: '零用量的轮次不污染用量：/cost 之后用量条的分子仍非 0',
    async run() {
      const t1 = await turn('zero-usage-numerator', {
        prompt: 'Reply with exactly one word: ok',
        computeContextStats: true,
      })
      check(t1.streamOutcome === 'completed', '第 1 轮应正常完成', t1.streamOutcome)
      const before = t1.contextState
      check(before.estimatedInputTokens > 0, '第 1 轮之后分子就该非 0', before)

      // `/cost` 没有任何 LLM 往返 ⇒ result 的 token 分量全 0。三道防线（不产出 Usage 事件 /
      // 全零不覆盖非零 / 挑锚点时跳过全零）任一破了，分子就会掉到 0。
      const t2 = await turn('zero-usage-numerator', {
        conversationId: t1.conversationId,
        prompt: '/cost',
        computeContextStats: true,
      })
      const after = t2.contextState
      check(!!after, 'computeContextStats 应回传 contextState', t2)
      check(
        after.estimatedInputTokens > 0,
        '零用量的 /cost 轮把分子清零了（spec 第 14h 条的三道防线有一道破了）',
        { before: before.estimatedInputTokens, after: after.estimatedInputTokens },
      )
      check(
        after.contextWindowTokens > 0,
        '零用量轮之后分母也丢了（全零的 result 只该被采纳窗口那一项）',
        after,
      )
      if (t2.usage) {
        // 零用量轮**允许**留下一行全 0 的 usage，但只在它携带分母的时候（spec 第 14h 条
        // 防线 1：「窗口不为空时才发，只为携带分母」）。分子的保护不在这里，而在
        // `collect_external_session_usage` 挑锚点时跳过全零 —— 即上面那两条断言。
        const zeroed = (t2.usage.input ?? 0) === 0 && (t2.usage.total ?? 0) === 0
        check(
          !zeroed || t2.usage.contextWindow > 0,
          '零用量轮写了一份既没有 token 也没有窗口的 usage —— 这一行没有任何存在理由',
          t2.usage,
        )
      }
      info(`分子：${before.estimatedInputTokens} → ${after.estimatedInputTokens}`)
    },
  },

  {
    name: 'tool-call-shape',
    desc: '真实工具调用的形态：原生 read 工具、arguments 带 file_path',
    async run() {
      const expectedReadName = AGENT === 'dsh' ? 'read' : 'Read'
      const r = await turn('tool-call-shape', {
        prompt:
          `Use the Read tool to read the file ${SENTINEL_NAME} in the current working directory, ` +
          `then reply with its exact contents and nothing else.`,
      })
      check(r.streamOutcome === 'completed', '本轮应正常完成', r.streamOutcome)
      const calls = r.toolCalls || []
      check(calls.length > 0, '一次工具调用都没有 —— 模型没读文件，或工具事件没落到消息上', r)
      const read = calls.find((c) => c.name === expectedReadName)
      check(
        !!read,
        `没有名为 ${expectedReadName} 的工具调用（各 CLI 保留自己的原生工具名）`,
        calls.map((c) => c.name),
      )
      let args = {}
      try {
        args = JSON.parse(read.arguments || '{}')
      } catch {
        throw new AssertError(
          `${expectedReadName} 的 arguments 不是合法 JSON\n     实际：${fmt(read.arguments)}`,
        )
      }
      check(
        typeof args.file_path === 'string' && args.file_path.length > 0,
        `${expectedReadName} 的入参里没有 file_path`,
        args,
      )
      check(
        args.file_path.includes(SENTINEL_NAME),
        `${expectedReadName} 读的不是 ${SENTINEL_NAME}`,
        args.file_path,
      )
      check(read.status === 'success', `${expectedReadName} 应执行成功`, read)
      check(answerOf(r).includes(SENTINEL_TEXT), '回答里没有文件内容 ⇒ 工具结果没回到模型', r.answer)
      // 工具卡应在分段里占一个位置，且顺序在正文分段之前/之间可见。
      const kinds = r.segments || []
      check(kinds.includes('tool'), '分段里应有一个 tool 段（工具卡的顺序锚点）', r.segments)
    },
  },

  {
    name: 'config-reconnect',
    desc: '启动配置变更触发重连：换 sandbox 档位 → pid 变了 → 但上下文还在',
    async run() {
      const dsh = AGENT === 'dsh'
      const t1 = await turn('config-reconnect', {
        prompt: 'Answer with one word only. Remember the number 91.',
        externalSandbox: dsh ? 'read-only' : 'bypassPermissions',
      })
      check(t1.streamOutcome === 'completed', '第 1 轮应正常完成', t1.streamOutcome)
      const pid1 = t1.liveSession.childPid
      check(typeof pid1 === 'number', '第 1 轮应拿到 pid', t1.liveSession)

      // sandbox 是进程级配置；LaunchConfig 指纹不匹配时必须轮前换进程，再用 native id resume。
      const t2 = await turn('config-reconnect', {
        conversationId: t1.conversationId,
        prompt: 'What number did I ask you to remember? Reply with just the number.',
        externalSandbox: dsh ? 'workspace-write' : 'acceptEdits',
      })
      check(t2.streamOutcome === 'completed', '重连后的这一轮应正常完成', t2.streamOutcome)
      check(
        t2.liveSession.childPid !== pid1,
        '换了启动参数但进程没换 ⇒ 界面显示一套、会话实际跑另一套（spec 第 8/26 条）',
        { turn1Pid: pid1, turn2Pid: t2.liveSession.childPid },
      )
      check(
        t2.liveSession.turnsServed === 1,
        '重连后应是一个新进程（turnsServed 从 1 起算）',
        t2.liveSession,
      )
      check(
        answerOf(t2).includes('91'),
        '重连丢了上下文（应带 native session id resume，而不是创建新会话）',
        t2.answer,
      )
      check(
        !/上下文已重置|context.*reset/i.test(answerOf(t2)),
        '真的续上了却发了「上下文已重置」提示 —— 假提示本身就是 bug',
        t2.answer,
      )
    }
  },
]

// ---------------------------------------------------------------------------------------------
// 自检
// ---------------------------------------------------------------------------------------------

function checkAgentInstalled() {
  const bin = AGENT === 'claude' ? 'claude' : AGENT
  // Windows 上 CLI 常常是 `claude.cmd`，不经 shell 找不到。命令与参数拼成一个字符串传给
  // shell（而不是 args 数组 + shell:true）——后者在 Node 22+ 会告 DEP0190。
  const probe = spawnSync(`${bin} --version`, { encoding: 'utf8', shell: true })
  if (probe.error || probe.status !== 0) {
    return `本机没有可用的 \`${bin}\`（${probe.error?.message || `exit=${probe.status}`}）`
  }
  info(`${bin} --version → ${(probe.stdout || probe.stderr || '').trim().split('\n')[0]}`)
  return null
}

function prepareWorkdir() {
  fs.mkdirSync(WORKDIR, { recursive: true })
  // 每次跑换一个哨兵串：内容固定的话，"回答里有它" 可能只是模型在复述提示词/缓存。
  fs.writeFileSync(path.join(WORKDIR, SENTINEL_NAME), `${SENTINEL_TEXT}\n`)
}

function printSkipHints(reason) {
  log('')
  log(`${C.yellow}排查提示${C.off}`)
  info(`原因：${reason}`)
  info(`1. app 起来了吗：另开一个终端 \`npm run dev\`，等 stderr 出现 \`[chat-probe] watching\``)
  info(`2. probe 目录：${PROBE_DIR}（不存在 = app 从没在这台机器上跑过 debug 构建）`)
  info(`3. 必须是 **debug** 构建：probe 整模块 #[cfg(debug_assertions)]，release 包里没有`)
  info(`4. ${AGENT} 装了并登录了吗：\`${AGENT} --version\`，然后 \`${AGENT} -p "hi"\``)
  info(`5. 未登录时：\`${AGENT} /login\`（或该 CLI 对应的登录命令）`)
  info(`6. 会话里选的运行时是不是外部 CLI：--agent 传的是 \`${AGENT}\``)
}

// ---------------------------------------------------------------------------------------------
// main
// ---------------------------------------------------------------------------------------------

async function main() {
  const selected = filters.length
    ? SCENARIOS.filter((s) => filters.some((f) => s.name.includes(f)))
    : SCENARIOS

  if (flags.has('--list') || flags.has('--help')) {
    log('场景：')
    for (const s of SCENARIOS) log(`  ${s.name.padEnd(22)} ${s.desc}`)
    log('')
    log('用法：npm run probe:e2e -- [名字子串…] [--agent claude] [--wait <秒>] [--verbose] [--probe-dir <path>]')
    return 0
  }
  if (!selected.length) {
    log(`${C.red}没有匹配的场景：${filters.join(' ')}${C.off}（--list 看全部）`)
    return 1
  }

  log(`${C.cyan}Kivio probe e2e${C.off} — agent=${AGENT}, ${selected.length} 个场景`)
  info(`probe 目录：${PROBE_DIR}`)
  info(`工作目录：${WORKDIR}`)

  const results = []
  let skipAllReason = null

  const install = checkAgentInstalled()
  if (install) skipAllReason = install

  if (!skipAllReason) {
    prepareWorkdir()
    try {
      await waitForApp()
      info('app 已就绪（probe watcher 应答了就绪握手）')
    } catch (err) {
      skipAllReason = `app 没在跑：${err.message}`
    }
  }

  for (const scenario of selected) {
    if (skipAllReason) {
      results.push({ name: scenario.name, status: 'skip', detail: skipAllReason })
      log(`${C.yellow}SKIP${C.off} ${scenario.name} — ${skipAllReason}`)
      continue
    }
    log('')
    log(`${C.cyan}▶${C.off} ${scenario.name} — ${scenario.desc}`)
    try {
      await scenario.run()
      results.push({ name: scenario.name, status: 'pass' })
      log(`${C.green}PASS${C.off} ${scenario.name}`)
    } catch (err) {
      if (err instanceof AppDownError) {
        skipAllReason = `app 没在跑：${err.message}`
        results.push({ name: scenario.name, status: 'skip', detail: skipAllReason })
        log(`${C.yellow}SKIP${C.off} ${scenario.name} — ${skipAllReason}`)
      } else if (err instanceof EnvError) {
        // 环境问题（未登录 / 限流 / 额度）：整套后续都 skip，不 fail。一个过期的凭据
        // 不该伪装成代码回归（spec 第 15 条）。
        skipAllReason = `环境问题：${err.message}`
        results.push({ name: scenario.name, status: 'skip', detail: skipAllReason })
        log(`${C.yellow}SKIP${C.off} ${scenario.name} — 环境问题`)
        info(err.message)
      } else {
        results.push({ name: scenario.name, status: 'fail', detail: err.message })
        log(`${C.red}FAIL${C.off} ${scenario.name}`)
        log(`   ${err.message}`)
      }
    }
  }

  const pass = results.filter((r) => r.status === 'pass')
  const fail = results.filter((r) => r.status === 'fail')
  const skip = results.filter((r) => r.status === 'skip')

  log('')
  log(`${C.cyan}汇总${C.off} ${pass.length} 绿 / ${fail.length} 红 / ${skip.length} skip`)
  for (const r of fail) log(`  ${C.red}FAIL${C.off} ${r.name}`)
  for (const r of skip) log(`  ${C.yellow}SKIP${C.off} ${r.name}`)
  if (warnings.length) log(`  ${C.yellow}${warnings.length} 条 WARN（软断言，不判红）${C.off}`)
  if (skip.length && skipAllReason) printSkipHints(skipAllReason)

  return fail.length ? 1 : 0
}

main().then(
  (code) => process.exit(code),
  (err) => {
    console.error(err)
    process.exit(1)
  },
)
