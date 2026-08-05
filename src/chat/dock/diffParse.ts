// 轻量 unified diff 解析器（不引第三方依赖）。
// 容错优先：截断的尾部、非标准行一律跳过而不是抛错 —— 后端 patch 有 512KB 截断。

export type DiffLineType = 'add' | 'del' | 'context'

export type DiffLine = {
  type: DiffLineType
  content: string
  /** 旧文件行号（add 行没有）。来自 @@ 头计数；头不可解析时缺省。 */
  oldNo?: number
  /** 新文件行号（del 行没有）。 */
  newNo?: number
}

export type DiffHunk = {
  /** 完整 @@ 头（含尾部函数上下文）。 */
  header: string
  lines: DiffLine[]
}

export type DiffFile = {
  oldPath: string
  newPath: string
  hunks: DiffHunk[]
  isBinary: boolean
  isNew: boolean
  isDeleted: boolean
}

/** 去掉 git 路径的 a/ b/ 前缀与引号。 */
function stripGitPathPrefix(path: string): string {
  let result = path.trim()
  if (result.startsWith('"') && result.endsWith('"') && result.length >= 2) {
    result = result.slice(1, -1)
  }
  if (result.startsWith('a/') || result.startsWith('b/')) result = result.slice(2)
  return result
}

/** `--- a/path\t(timestamp)` 这类行取路径部分。 */
function pathFromMarkerLine(line: string): string {
  const rest = line.slice(4)
  // 制表符后跟时间戳的情况只取路径段。
  const tabIndex = rest.indexOf('\t')
  return stripGitPathPrefix(tabIndex >= 0 ? rest.slice(0, tabIndex) : rest)
}

export function parseDiff(patch: string): DiffFile[] {
  const files: DiffFile[] = []
  if (!patch) return files

  let currentFile: DiffFile | null = null
  let currentHunk: DiffHunk | null = null
  // @@ 头里的行号计数器（头不可解析时为 null，行不带行号）。
  let oldNo: number | null = null
  let newNo: number | null = null

  // 去掉结尾换行产生的空串行，避免被当成 context 行收进最后一个 hunk。
  const lines = patch.split('\n')
  while (lines.length > 0 && lines[lines.length - 1] === '') lines.pop()

  const ensureFile = (): DiffFile => {
    if (!currentFile) {
      // 没有 `diff --git` 头的裸 patch（自合成的 untracked patch 等）兜底出一个匿名文件。
      currentFile = { oldPath: '', newPath: '', hunks: [], isBinary: false, isNew: false, isDeleted: false }
      files.push(currentFile)
    }
    return currentFile
  }

  for (const line of lines) {
    if (line.startsWith('diff --git ')) {
      currentFile = { oldPath: '', newPath: '', hunks: [], isBinary: false, isNew: false, isDeleted: false }
      files.push(currentFile)
      currentHunk = null
      // `diff --git a/x b/y` 先粗略填一对路径，---/+++ 行再精确覆盖。
      const match = /^diff --git ("?a\/.+?"?) ("?b\/.+"?)$/.exec(line)
      if (match) {
        currentFile.oldPath = stripGitPathPrefix(match[1])
        currentFile.newPath = stripGitPathPrefix(match[2])
      }
      continue
    }
    if (!currentFile) {
      // 文件头之前的噪声（截断开头等）忽略；裸 patch（无 diff --git 头）兜底出一个匿名文件。
      if (line.startsWith('@@') || line.startsWith('---') || line.startsWith('+++')) {
        currentFile = ensureFile()
      } else {
        continue
      }
    }
    const file = currentFile

    if (line.startsWith('new file mode')) {
      file.isNew = true
      continue
    }
    if (line.startsWith('deleted file mode')) {
      file.isDeleted = true
      continue
    }
    if (line.startsWith('Binary files ') || line.startsWith('GIT binary patch')) {
      file.isBinary = true
      continue
    }
    if (line.startsWith('--- ')) {
      const path = pathFromMarkerLine(line)
      file.oldPath = path === '/dev/null' ? '' : path
      if (path === '/dev/null') file.isNew = true
      continue
    }
    if (line.startsWith('+++ ')) {
      const path = pathFromMarkerLine(line)
      file.newPath = path === '/dev/null' ? '' : path
      if (path === '/dev/null') file.isDeleted = true
      continue
    }
    if (line.startsWith('@@')) {
      currentHunk = { header: line, lines: [] }
      file.hunks.push(currentHunk)
      const nums = /^@@ -(\d+)(?:,\d+)? \+(\d+)(?:,\d+)? @@/.exec(line)
      oldNo = nums ? Number(nums[1]) : null
      newNo = nums ? Number(nums[2]) : null
      continue
    }
    if (line.startsWith('\\')) {
      // "\ No newline at end of file"
      continue
    }
    if (!currentHunk) continue
    const first = line[0]
    if (first === '+') {
      currentHunk.lines.push({ type: 'add', content: line.slice(1), newNo: newNo ?? undefined })
      if (newNo != null) newNo += 1
    } else if (first === '-') {
      currentHunk.lines.push({ type: 'del', content: line.slice(1), oldNo: oldNo ?? undefined })
      if (oldNo != null) oldNo += 1
    } else if (first === ' ' || line === '') {
      currentHunk.lines.push({
        type: 'context',
        content: line.slice(1),
        oldNo: oldNo ?? undefined,
        newNo: newNo ?? undefined,
      })
      if (oldNo != null) oldNo += 1
      if (newNo != null) newNo += 1
    }
    // 其他行（截断尾部垃圾）跳过。
  }

  // 丢弃完全没有内容的空文件条目（例如仅有 diff --git 头、其余被截掉）。
  return files.filter((file) => file.hunks.length > 0 || file.isBinary || file.isNew || file.isDeleted)
}

/** 从 hunks 数出 +adds/−dels（stat 解析不可靠时的主口径）。 */
export function countDiffStats(file: DiffFile): { adds: number; dels: number } {
  let adds = 0
  let dels = 0
  for (const hunk of file.hunks) {
    for (const line of hunk.lines) {
      if (line.type === 'add') adds += 1
      else if (line.type === 'del') dels += 1
    }
  }
  return { adds, dels }
}

/** 词级差异：相邻的 del 段和 add 段按位成对，找公共前后缀，中段返回高亮区间。
 *  key = 行在 lines 里的下标，值 = [start, end)。整行都不同（无公共前后缀）不标——
 *  整行底色已经表达了。ponytail: 纯前后缀比较，无 LCS；乱序改动标不出词级差异，够用。 */
export function intralineRanges(lines: DiffLine[]): Map<number, [number, number]> {
  const ranges = new Map<number, [number, number]>()
  let i = 0
  while (i < lines.length) {
    if (lines[i].type !== 'del') {
      i += 1
      continue
    }
    const delStart = i
    while (i < lines.length && lines[i].type === 'del') i += 1
    const addStart = i
    while (i < lines.length && lines[i].type === 'add') i += 1
    const pairs = Math.min(addStart - delStart, i - addStart)
    for (let k = 0; k < pairs; k += 1) {
      const a = lines[delStart + k].content
      const b = lines[addStart + k].content
      if (!a || !b || a === b) continue
      const max = Math.min(a.length, b.length)
      let prefix = 0
      while (prefix < max && a[prefix] === b[prefix]) prefix += 1
      let suffix = 0
      while (suffix < max - prefix && a[a.length - 1 - suffix] === b[b.length - 1 - suffix]) suffix += 1
      if (prefix === 0 && suffix === 0) continue
      ranges.set(delStart + k, [prefix, a.length - suffix])
      ranges.set(addStart + k, [prefix, b.length - suffix])
    }
  }
  return ranges
}
