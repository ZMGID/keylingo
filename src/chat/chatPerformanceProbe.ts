import { useEffect, useRef } from 'react'
import type { ProfilerOnRenderCallback } from 'react'

type ProbeBucket = {
  commits: number
  renders: number
  actualMs: number
  baseMs: number
  maxActualMs: number
  lastPhase: string
  lastDetail: string
}

export type ChatPerfWindowSample = {
  name: string
  durationMs: number
  mountedRows: number
  domNodes: number
  detail?: string
}

const windowSamples: ChatPerfWindowSample[] = []
const buckets = new Map<string, ProbeBucket>()
let flushTimer: number | null = null
let enabledCache: boolean | null = null

function probeEnabled(): boolean {
  if (enabledCache !== null) return enabledCache
  if (!import.meta.env.DEV) {
    enabledCache = false
    return false
  }
  const globalFlag = (globalThis as { __KIVIO_CHAT_PERF__?: boolean }).__KIVIO_CHAT_PERF__
  if (globalFlag !== undefined) {
    enabledCache = globalFlag
    return globalFlag
  }
  try {
    enabledCache = window.localStorage.getItem('kivio.debug.chatPerf') !== '0'
  } catch {
    enabledCache = true
  }
  return enabledCache
}

function formatDetail(detail: unknown): string {
  if (detail == null) return ''
  if (typeof detail === 'string') return detail
  try {
    return JSON.stringify(detail)
  } catch {
    return String(detail)
  }
}

function scheduleFlush() {
  if (flushTimer !== null || typeof window === 'undefined') return
  flushTimer = window.setTimeout(() => {
    flushTimer = null
    if (buckets.size === 0) return
    const rows = [...buckets.entries()].map(([name, bucket]) => ({
      name,
      commits: bucket.commits,
      renders: bucket.renders,
      actualMs: Number(bucket.actualMs.toFixed(1)),
      baseMs: Number(bucket.baseMs.toFixed(1)),
      maxActualMs: Number(bucket.maxActualMs.toFixed(1)),
      phase: bucket.lastPhase,
      detail: bucket.lastDetail,
    }))
    buckets.clear()
    console.info('[kivio:perf] chat window', rows, windowSamples.splice(0, windowSamples.length))
  }, 500)
}

export function recordChatPerfSample(sample: ChatPerfWindowSample): void {
  if (!probeEnabled()) return
  windowSamples.push({
    ...sample,
    durationMs: Number(Math.max(0, sample.durationMs).toFixed(1)),
  })
  scheduleFlush()
}

export function measureChatSurface(
  name: string,
  root: Element | null,
  detail?: string,
): () => void {
  const startedAt = typeof performance === 'undefined' ? 0 : performance.now()
  return () => {
    if (startedAt === 0) return
    recordChatPerfSample({
      name,
      durationMs: performance.now() - startedAt,
      mountedRows: root?.querySelectorAll('[data-chat-message-list-item]').length ?? 0,
      domNodes: root?.querySelectorAll('*').length ?? 0,
      detail,
    })
  }
}

export function resetChatPerfProbeForTests(): void {
  buckets.clear()
  windowSamples.length = 0
  flushTimer = null
  enabledCache = null
}

function bucketFor(name: string): ProbeBucket {
  const existing = buckets.get(name)
  if (existing) return existing
  const bucket: ProbeBucket = {
    commits: 0,
    renders: 0,
    actualMs: 0,
    baseMs: 0,
    maxActualMs: 0,
    lastPhase: '',
    lastDetail: '',
  }
  buckets.set(name, bucket)
  return bucket
}

function recordRender(name: string, detail?: unknown) {
  if (!probeEnabled()) return
  const bucket = bucketFor(name)
  bucket.renders += 1
  bucket.lastDetail = formatDetail(detail)
  scheduleFlush()
}

export const onChatPerfProfiler: ProfilerOnRenderCallback = (
  id,
  phase,
  actualDuration,
  baseDuration,
) => {
  if (!probeEnabled()) return
  const bucket = bucketFor(id)
  bucket.commits += 1
  bucket.actualMs += actualDuration
  bucket.baseMs += baseDuration
  bucket.maxActualMs = Math.max(bucket.maxActualMs, actualDuration)
  bucket.lastPhase = phase
  scheduleFlush()
}

export function useChatPerfRenderProbe(name: string, detail?: unknown) {
  const renderCount = useRef(0)
  renderCount.current += 1
  recordRender(name, detail)
}

export function useChatPerfLongTaskProbe() {
  useEffect(() => {
    if (!probeEnabled() || typeof PerformanceObserver === 'undefined') return
    let observer: PerformanceObserver | null = null
    try {
      observer = new PerformanceObserver((list) => {
        for (const entry of list.getEntries()) {
          console.warn('[kivio:perf] long task', {
            durationMs: Number(entry.duration.toFixed(1)),
            startTime: Number(entry.startTime.toFixed(1)),
          })
        }
      })
      observer.observe({ entryTypes: ['longtask'] })
    } catch {
      // Safari/WebKit may not expose the longtask entry type.
    }
    return () => observer?.disconnect()
  }, [])
}
