import { describe, expect, it } from 'vitest'
import { initialReplacePackProgressState, reduceReplacePackProgress } from './replacePackProgress'

const progress = {
  pack: 'replace_translation' as const,
  componentId: 'rapidocr-high',
  fileName: 'high/det.onnx',
  downloadedBytes: 50,
  fileTotalBytes: 100,
  overallDownloadedBytes: 70,
  overallTotalBytes: 200,
  attempt: 1,
  state: 'downloading' as const,
}

describe('replace pack progress reducer', () => {
  it('starts clean and records progress', () => {
    const started = reduceReplacePackProgress(initialReplacePackProgressState, { type: 'start' })
    expect(started).toEqual({ downloadState: 'downloading', error: '', progress: null })
    expect(reduceReplacePackProgress(started, { type: 'progress', progress }).progress).toEqual(progress)
  })

  it('keeps the failing file and error for retry UI', () => {
    const failed = reduceReplacePackProgress(initialReplacePackProgressState, {
      type: 'progress',
      progress: { ...progress, state: 'failed', error: 'checksum mismatch' },
    })
    expect(failed.downloadState).toBe('failed')
    expect(failed.error).toBe('checksum mismatch')
    expect(failed.progress?.fileName).toBe(progress.fileName)
  })

  it('stays downloading on a mid-pack per-file completed event', () => {
    const started = reduceReplacePackProgress(initialReplacePackProgressState, { type: 'start' })
    const midway = reduceReplacePackProgress(started, {
      type: 'progress',
      progress: { ...progress, state: 'completed', downloadedBytes: 100, overallDownloadedBytes: 100 },
    })
    expect(midway.downloadState).toBe('downloading')
  })

  it('resolves to idle when the final completed event covers all bytes', () => {
    const started = reduceReplacePackProgress(initialReplacePackProgressState, { type: 'start' })
    const done = reduceReplacePackProgress(started, {
      type: 'progress',
      progress: {
        ...progress,
        state: 'completed',
        downloadedBytes: 100,
        overallDownloadedBytes: 200,
      },
    })
    expect(done.downloadState).toBe('idle')
    expect(done.error).toBe('')
  })
})
