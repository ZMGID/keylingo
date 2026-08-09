#!/usr/bin/env node

import fs from 'node:fs'

const reportPath = process.argv[2]
if (!reportPath) {
  console.error('usage: npm run probe:chat-performance -- report.json')
  process.exit(2)
}

let report
try {
  report = JSON.parse(fs.readFileSync(reportPath, 'utf8'))
} catch (error) {
  console.error(`cannot read report: ${reportPath}`)
  console.error(error instanceof Error ? error.message : String(error))
  process.exit(2)
}

const samples = Array.isArray(report.samples) ? report.samples : []
const longTasks = Array.isArray(report.longTasks) ? report.longTasks : []
const buckets = Array.isArray(report.buckets) ? report.buckets : []
const summary = {
  maxMountedRows: Math.max(0, ...samples.map((sample) => Number(sample.mountedRows) || 0)),
  maxDomNodes: Math.max(0, ...samples.map((sample) => Number(sample.domNodes) || 0)),
  maxSampleDurationMs: Math.max(0, ...samples.map((sample) => Number(sample.durationMs) || 0)),
  maxLongTaskMs: Math.max(0, ...longTasks.map((task) => Number(task.durationMs) || 0)),
  totalProfilerCommits: buckets.reduce((sum, bucket) => sum + (Number(bucket.commits) || 0), 0),
  totalProfilerActualMs: buckets.reduce((sum, bucket) => sum + (Number(bucket.actualMs) || 0), 0),
}

// These are guardrails for the rendering architecture, not product SLAs.
// Tighten them only after collecting a baseline on the target machine.
const budget = {
  maxMountedRows: 64,
  maxDomNodes: 12_000,
  maxLongTaskMs: 250,
}
const violations = []
if (summary.maxMountedRows > budget.maxMountedRows) {
  violations.push(`mountedRows ${summary.maxMountedRows} > ${budget.maxMountedRows}`)
}
if (summary.maxDomNodes > budget.maxDomNodes) {
  violations.push(`domNodes ${summary.maxDomNodes} > ${budget.maxDomNodes}`)
}
if (summary.maxLongTaskMs > budget.maxLongTaskMs) {
  violations.push(`longTaskMs ${summary.maxLongTaskMs} > ${budget.maxLongTaskMs}`)
}

console.log(JSON.stringify({ reportPath, budget, summary, violations }, null, 2))
if (violations.length > 0) process.exit(1)
