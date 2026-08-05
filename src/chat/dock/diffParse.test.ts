import { describe, expect, it } from 'vitest'
import { countDiffStats, intralineRanges, parseDiff } from './diffParse'

const MULTI_FILE_PATCH = `diff --git a/src/a.ts b/src/a.ts
index 1111111..2222222 100644
--- a/src/a.ts
+++ b/src/a.ts
@@ -1,3 +1,4 @@ export function a() {
 line one
-old line
+new line
+another line
 line three
diff --git a/src/b.ts b/src/b.ts
new file mode 100644
index 0000000..3333333
--- /dev/null
+++ b/src/b.ts
@@ -0,0 +1,2 @@
+first
+second
`

describe('parseDiff', () => {
  it('returns an empty array for empty input', () => {
    expect(parseDiff('')).toEqual([])
    expect(parseDiff('\n\n')).toEqual([])
  })

  it('parses a multi-file patch with hunks', () => {
    const files = parseDiff(MULTI_FILE_PATCH)
    expect(files).toHaveLength(2)

    expect(files[0].oldPath).toBe('src/a.ts')
    expect(files[0].newPath).toBe('src/a.ts')
    expect(files[0].isNew).toBe(false)
    expect(files[0].hunks).toHaveLength(1)
    expect(files[0].hunks[0].header).toContain('@@ -1,3 +1,4 @@')
    expect(files[0].hunks[0].lines.map((line) => line.type)).toEqual([
      'context',
      'del',
      'add',
      'add',
      'context',
    ])
    expect(countDiffStats(files[0])).toEqual({ adds: 2, dels: 1 })

    expect(files[1].isNew).toBe(true)
    expect(files[1].oldPath).toBe('')
    expect(files[1].newPath).toBe('src/b.ts')
    expect(countDiffStats(files[1])).toEqual({ adds: 2, dels: 0 })
  })

  it('marks deleted files via /dev/null', () => {
    const patch = `diff --git a/old.ts b/old.ts
deleted file mode 100644
index 1111111..0000000
--- a/old.ts
+++ /dev/null
@@ -1,1 +0,0 @@
-gone
`
    const files = parseDiff(patch)
    expect(files).toHaveLength(1)
    expect(files[0].isDeleted).toBe(true)
    expect(files[0].newPath).toBe('')
    expect(files[0].oldPath).toBe('old.ts')
  })

  it('marks binary files without hunks', () => {
    const patch = `diff --git a/logo.png b/logo.png
index 1111111..2222222 100644
Binary files a/logo.png and b/logo.png differ
`
    const files = parseDiff(patch)
    expect(files).toHaveLength(1)
    expect(files[0].isBinary).toBe(true)
    expect(files[0].hunks).toHaveLength(0)
  })

  it('survives a truncated tail', () => {
    const patch = `diff --git a/src/a.ts b/src/a.ts
--- a/src/a.ts
+++ b/src/a.ts
@@ -1,2 +1,2 @@
 context
-old
+new garbled \u0000 binary-ish line without prefix`
    const files = parseDiff(patch)
    expect(files).toHaveLength(1)
    // 无 +/-/' ' 前缀的截断行被跳过，合法行保留。
    expect(files[0].hunks[0].lines).toHaveLength(3)
  })

  it('handles rename patches (old/new path from ---/+++)', () => {
    const patch = `diff --git a/before.ts b/after.ts
similarity index 90%
rename from before.ts
rename to after.ts
index 1111111..2222222 100644
--- a/before.ts
+++ b/after.ts
@@ -1,1 +1,1 @@
-x
+y
`
    const files = parseDiff(patch)
    expect(files[0].oldPath).toBe('before.ts')
    expect(files[0].newPath).toBe('after.ts')
  })

  it('parses a bare patch without diff --git header (synthesized untracked)', () => {
    const patch = `--- /dev/null
+++ b/newfile.ts
@@ -0,0 +1,1 @@
+hello
`
    const files = parseDiff(patch)
    expect(files).toHaveLength(1)
    expect(files[0].newPath).toBe('newfile.ts')
    expect(files[0].isNew).toBe(true)
  })

  it('ignores "\\ No newline at end of file" markers', () => {
    const patch = `diff --git a/f.ts b/f.ts
--- a/f.ts
+++ b/f.ts
@@ -1,1 +1,1 @@
-a
\\ No newline at end of file
+b
\\ No newline at end of file
`
    const files = parseDiff(patch)
    expect(files[0].hunks[0].lines).toHaveLength(2)
  })

  it('numbers lines from the @@ header', () => {
    const files = parseDiff(MULTI_FILE_PATCH)
    const lines = files[0].hunks[0].lines
    // @@ -1,3 +1,4 @@：context / del / add / add / context
    expect(lines.map((l) => [l.oldNo ?? null, l.newNo ?? null])).toEqual([
      [1, 1],
      [2, null],
      [null, 2],
      [null, 3],
      [3, 4],
    ])
  })
})

describe('intralineRanges', () => {
  it('marks the differing middle of paired del/add lines', () => {
    const files = parseDiff(MULTI_FILE_PATCH)
    const lines = files[0].hunks[0].lines
    // "old line" → "new line"：公共后缀 " line"，中段 old/new 高亮。
    const ranges = intralineRanges(lines)
    expect(ranges.get(1)).toEqual([0, 3])
    expect(ranges.get(2)).toEqual([0, 3])
    // 无配对的第二条 add（"another line"）不标。
    expect(ranges.has(3)).toBe(false)
  })
})
