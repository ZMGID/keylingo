import type { PyodideInterface } from 'pyodide'
import type { ChatToolArtifact } from './types'

const PYODIDE_VERSION = '0.26.4'
const PYODIDE_PRIMARY_CDN_INDEX_URL = `https://cdn.jsdelivr.net/pyodide/v${PYODIDE_VERSION}/full/`
const PYODIDE_CDN_INDEX_URLS = [
  PYODIDE_PRIMARY_CDN_INDEX_URL,
  `https://cdn.jsdelivr.net/npm/pyodide@${PYODIDE_VERSION}/`,
  `https://unpkg.com/pyodide@${PYODIDE_VERSION}/`,
]
const LOCAL_PYODIDE_SOURCE_LABEL = 'local app resources'
const PYODIDE_PACKAGE_IMPORTS: Array<[RegExp, string]> = [
  [/(^|\n)\s*(import|from)\s+numpy\b/, 'numpy'],
  [/(^|\n)\s*(import|from)\s+matplotlib\b/, 'matplotlib'],
  [/(^|\n)\s*(import|from)\s+pandas\b/, 'pandas'],
  [/(^|\n)\s*(import|from)\s+scipy\b/, 'scipy'],
  [/(^|\n)\s*(import|from)\s+sympy\b/, 'sympy'],
  [/(^|\n)\s*(import|from)\s+sklearn\b/, 'scikit-learn'],
  [/(^|\n)\s*(import|from)\s+statsmodels\b/, 'statsmodels'],
  [/(^|\n)\s*(import|from)\s+(PIL|pillow)\b/, 'pillow'],
  [/(^|\n)\s*(import|from)\s+seaborn\b/, 'seaborn'],
  [/(^|\n)\s*(import|from)\s+pypdf\b/, 'pypdf'],
  [/(^|\n)\s*(import|from)\s+micropip\b/, 'micropip'],
  [/(^|\n)\s*(import|from)\s+et_xmlfile\b/, 'et_xmlfile'],
  [/(^|\n)\s*(import|from)\s+openpyxl\b/, 'openpyxl'],
  [/(^|\n)\s*(import|from)\s+xlrd\b/, 'xlrd'],
]
const PYTHON_SANDBOX_FAILURE_GUIDANCE =
  '不要使用 run_command/pip 安装或修改本机 Python 环境来绕过沙盒；请直接基于已有数据用文本/表格回答，除非用户明确要求修改本机环境。'
const PYTHON_ARTIFACT_EXTENSIONS = new Set([
  '.png', '.jpg', '.jpeg', '.gif', '.webp', '.svg',
  '.csv', '.tsv', '.json', '.md', '.txt', '.html', '.htm',
  '.xlsx', '.xls',
])
const PYTHON_ARTIFACT_SCAN_ROOTS = ['/home/pyodide', '/tmp']
const PYTHON_INPUT_SCAN_ROOT = '/home/pyodide/kivio_inputs'
const PYTHON_FONT_VIRTUAL_DIR = '/home/pyodide/.kivio/fonts'
const PYTHON_DEFAULT_CJK_FONT = {
  fileName: 'NotoSansCJKsc-Regular.otf',
  family: 'Noto Sans CJK SC',
}
const MAX_PYTHON_ARTIFACT_BYTES = 12 * 1024 * 1024
const MAX_PYTHON_ARTIFACTS = 16
const MAX_PYTHON_INPUT_FILE_BYTES = 100 * 1024 * 1024
const PYODIDE_IMPORT_PACKAGE_ALIASES: Record<string, string> = {
  PIL: 'pillow',
  bs4: 'beautifulsoup4',
  cv2: 'opencv-python',
  sklearn: 'scikit-learn',
  skimage: 'scikit-image',
  tavily: 'tavily-python',
  yaml: 'pyyaml',
  openpyxl: 'openpyxl',
  xlrd: 'xlrd',
  pypdf: 'pypdf',
}

type PyodideFsStat = {
  mode: number
  size?: number
  mtime?: Date | number
}
type PyodideFs = {
  readdir(path: string): string[]
  stat(path: string): PyodideFsStat
  isDir(mode: number): boolean
  isFile(mode: number): boolean
  readFile(path: string, options?: { encoding?: 'binary' }): Uint8Array | ArrayBuffer | number[] | string
  writeFile(path: string, data: Uint8Array): void
  unlink(path: string): void
  mkdirTree?: (path: string) => void
  analyzePath?: (path: string) => { exists: boolean }
}
type PythonArtifactSnapshotEntry = {
  size: number
  mtimeMs: number
}
type PythonOutputResult = {
  content: string
  artifacts: ChatToolArtifact[]
}
export type PythonInputFile = {
  name: string
  dataBase64: string
  sizeBytes: number
}
type PyodideSource = {
  label: string
  indexURL: string
}
type PyodidePackageManifest = {
  fontFiles?: Array<{
    fileName?: string
    family?: string
    sha256?: string
  }>
  pypiWheels?: Record<string, {
    fileName?: string
    pyodideDeps?: string[]
    localWheelDeps?: string[]
  }>
}
type PythonFontAsset = {
  fileName: string
  family: string
  bytes: Uint8Array
}

let localPyodidePromise: Promise<PyodideInterface> | null = null
let packagePyodidePromise: Promise<PyodideInterface> | null = null
let packagePyodideLoadedPackages = new Set<string>()
let packageManifestPromise: Promise<PyodidePackageManifest | null> | null = null
let pythonCjkFontAssetPromise: Promise<PythonFontAsset | null> | null = null
const fontConfiguredRuntimes = new WeakSet<PyodideInterface>()

function localPyodideIndexUrl(): string {
  // Pyodide 资源固定 emit 在应用根 /pyodide/（见 vite.config.ts generateBundle: `pyodide/${fileName}`）。
  // 本模块只在 Web Worker 里运行，worker 自身被 Vite 打到 /assets/ 下——绝不能拿 self.location.href
  // 当基址 + 相对的 BASE_URL('./')，那样会解析成 /assets/pyodide/ → 404 → 静默回落 CDN（dev 下
  // BASE_URL 被规范成 '/' 掩盖了这点，仅打包后暴露）。用根绝对路径解析到应用根，dev/prod 一致。
  return new URL('/pyodide/', self.location.href).toString()
}

function pyodideSources(): PyodideSource[] {
  return [
    { label: LOCAL_PYODIDE_SOURCE_LABEL, indexURL: localPyodideIndexUrl() },
    ...PYODIDE_CDN_INDEX_URLS.map((indexURL) => ({ label: indexURL, indexURL })),
  ]
}

function localPyodideAssetUrl(fileName: string): string {
  return new URL(fileName, localPyodideIndexUrl()).toString()
}

function compactErrorMessage(message: string): string {
  const normalized = message
    .replace(/\s+/g, ' ')
    .replace(/https?:\/\/\S+/g, (url) => {
      try {
        const parsed = new URL(url)
        return `${parsed.origin}${parsed.pathname}`
      } catch {
        return url
      }
    })
    .trim()
  if (normalized.length <= 700) return normalized
  return `${normalized.slice(0, 700)}...`
}

async function loadPyodideFromSource(source: PyodideSource): Promise<PyodideInterface> {
  const { loadPyodide } = await import('pyodide')
  return loadPyodide({
    indexURL: source.indexURL,
    lockFileURL: `${source.indexURL}pyodide-lock.json`,
    stdLibURL: `${source.indexURL}python_stdlib.zip`,
  })
}

async function loadPyodideRuntime(): Promise<PyodideInterface> {
  const errors: string[] = []
  for (const source of pyodideSources()) {
    try {
      return await loadPyodideFromSource(source)
    } catch (err) {
      errors.push(`${source.label}: ${compactErrorMessage(describePythonError(err))}`)
    }
  }
  throw new Error(`所有 Python 运行时来源加载失败。${errors.join('；')}`)
}

async function loadPackageManifest(): Promise<PyodidePackageManifest | null> {
  if (!packageManifestPromise) {
    packageManifestPromise = fetch(localPyodideAssetUrl('pyodide-package-manifest.json'))
      .then(async (response) => {
        if (!response.ok) return null
        return await response.json() as PyodidePackageManifest
      })
      .catch(() => null)
  }
  return packageManifestPromise
}

async function fetchPythonCjkFontAsset(): Promise<PythonFontAsset | null> {
  if (!pythonCjkFontAssetPromise) {
    pythonCjkFontAssetPromise = (async () => {
      const manifest = await loadPackageManifest()
      const fontEntry = manifest?.fontFiles?.find((entry) => entry.fileName) ?? PYTHON_DEFAULT_CJK_FONT
      const fileName = fontEntry.fileName ?? PYTHON_DEFAULT_CJK_FONT.fileName
      const family = fontEntry.family ?? PYTHON_DEFAULT_CJK_FONT.family
      const response = await fetch(localPyodideAssetUrl(fileName))
      if (!response.ok) return null
      return {
        fileName,
        family,
        bytes: new Uint8Array(await response.arrayBuffer()),
      }
    })().catch(() => null)
  }
  return pythonCjkFontAssetPromise
}

async function ensurePythonFontSupport(pyodide: PyodideInterface): Promise<void> {
  if (fontConfiguredRuntimes.has(pyodide)) return
  fontConfiguredRuntimes.add(pyodide)

  const fs = pyodide.FS as PyodideFs
  const font = await fetchPythonCjkFontAsset()
  const fontPath = font ? `${PYTHON_FONT_VIRTUAL_DIR}/${font.fileName}` : ''
  if (font) {
    try {
      fs.mkdirTree?.(PYTHON_FONT_VIRTUAL_DIR)
    } catch {
      // Directory may already exist.
    }
    fs.writeFile(fontPath, font.bytes)
  }

  await pyodide.runPythonAsync(`
import os as _kivio_font_os

KIVIO_CJK_FONT_PATH = ${pythonStringLiteral(fontPath)}
KIVIO_CJK_FONT_FAMILY = ${pythonStringLiteral(font?.family ?? '')}
if KIVIO_CJK_FONT_PATH:
    _kivio_font_os.environ["KIVIO_CJK_FONT_PATH"] = KIVIO_CJK_FONT_PATH
    _kivio_font_os.environ["KIVIO_CJK_FONT_FAMILY"] = KIVIO_CJK_FONT_FAMILY

def _kivio_configure_matplotlib_fonts():
    if not KIVIO_CJK_FONT_PATH:
        return None
    try:
        import matplotlib as _kivio_matplotlib
        from matplotlib import font_manager as _kivio_font_manager
        _kivio_font_manager.fontManager.addfont(KIVIO_CJK_FONT_PATH)
        _kivio_font_name = _kivio_font_manager.FontProperties(fname=KIVIO_CJK_FONT_PATH).get_name()
        _kivio_font_candidates = [
            _kivio_font_name,
            KIVIO_CJK_FONT_FAMILY,
            "Noto Sans CJK SC",
            "Noto Sans CJK JP",
            "Noto Sans CJK KR",
            "DejaVu Sans",
        ]
        _kivio_matplotlib.rcParams["font.family"] = [
            _kivio_name for _kivio_name in _kivio_font_candidates if _kivio_name
        ]
        _kivio_matplotlib.rcParams["axes.unicode_minus"] = False
        return _kivio_font_name
    except Exception:
        return None

def kivio_cjk_font(size=24):
    if not KIVIO_CJK_FONT_PATH:
        raise RuntimeError("KIVIO_CJK_FONT_PATH is unavailable in this Python sandbox.")
    from PIL import ImageFont as _kivio_image_font
    return _kivio_image_font.truetype(KIVIO_CJK_FONT_PATH, size)
`.trim())
}

async function installLocalWheelPackage(
  pyodide: PyodideInterface,
  packageName: string,
  manifest: PyodidePackageManifest | null,
  installing = new Set<string>(),
): Promise<boolean> {
  const wheel = manifest?.pypiWheels?.[packageName]
  if (!wheel?.fileName) return false
  if (installing.has(packageName)) {
    throw new Error(`Circular Python sandbox wheel dependency: ${[...installing, packageName].join(' -> ')}`)
  }
  installing.add(packageName)

  try {
    const deps = wheel.pyodideDeps?.filter((dep) => dep !== packageName) ?? []
    if (deps.length > 0) {
      await pyodide.loadPackage(deps)
    }
    for (const dep of wheel.localWheelDeps ?? []) {
      await installLocalWheelPackage(pyodide, dep, manifest, installing)
    }
    if (!deps.includes('micropip')) {
      await pyodide.loadPackage('micropip')
    }

    const wheelUrl = localPyodideAssetUrl(wheel.fileName)
    const installWheelCode = `
import micropip as _kivio_micropip
await _kivio_micropip.install(${pythonStringLiteral(wheelUrl)}, deps=False)
`.trim()
    await pyodide.runPythonAsync(installWheelCode)
  } finally {
    installing.delete(packageName)
  }
  return true
}

async function installPackagesOnRuntime(
  pyodide: PyodideInterface,
  packages: string[],
  manifest: PyodidePackageManifest | null,
): Promise<void> {
  if (packages.length === 0) return
  const pyodidePackages: string[] = []
  for (const packageName of packages) {
    if (await installLocalWheelPackage(pyodide, packageName, manifest)) {
      continue
    }
    pyodidePackages.push(packageName)
  }
  if (pyodidePackages.length > 0) {
    await pyodide.loadPackage(pyodidePackages)
  }
}

async function installMicropipPackage(pyodide: PyodideInterface, packageName: string): Promise<void> {
  await pyodide.loadPackage('micropip')
  await pyodide.runPythonAsync(`
import micropip as _kivio_micropip
await _kivio_micropip.install(${pythonStringLiteral(packageName)})
`.trim())
}

async function loadRuntimeWithPackages(packages: string[]): Promise<PyodideInterface> {
  const manifest = packages.length > 0 ? await loadPackageManifest() : null
  const errors: string[] = []
  for (const source of pyodideSources()) {
    try {
      const pyodide = await loadPyodideFromSource(source)
      await installPackagesOnRuntime(pyodide, packages, manifest)
      return pyodide
    } catch (err) {
      errors.push(`${source.label}: ${compactErrorMessage(describePythonError(err))}`)
    }
  }
  throw new Error(`所有 Python 包来源加载失败。${errors.join('；')}`)
}

async function getPyodideWithPackages(packages: string[]): Promise<PyodideInterface> {
  if (!packagePyodidePromise) {
    packagePyodideLoadedPackages = new Set()
    packagePyodidePromise = loadRuntimeWithPackages(packages)
      .then((pyodide) => {
        packages.forEach((packageName) => packagePyodideLoadedPackages.add(packageName))
        return pyodide
      })
      .catch((err) => {
        packagePyodidePromise = null
        packagePyodideLoadedPackages = new Set()
        throw err
      })
    return packagePyodidePromise
  }

  const pyodide = await packagePyodidePromise
  const missingPackages = packages.filter((packageName) => !packagePyodideLoadedPackages.has(packageName))
  if (missingPackages.length === 0) return pyodide

  try {
    await installPackagesOnRuntime(pyodide, missingPackages, await loadPackageManifest())
    missingPackages.forEach((packageName) => packagePyodideLoadedPackages.add(packageName))
    return pyodide
  } catch {
    packagePyodidePromise = null
    packagePyodideLoadedPackages = new Set()
    return await getPyodideWithPackages(packages)
  }
}

function getPyodide(): Promise<PyodideInterface> {
  if (!localPyodidePromise) {
    localPyodidePromise = loadPyodideRuntime().catch((err) => {
      localPyodidePromise = null
      throw err
    })
  }
  return localPyodidePromise
}

export type PythonRunOutcome = {
  content: string
  isError: boolean
  artifacts: ChatToolArtifact[]
}

type PythonExecutionSuccess = {
  content: string
  artifacts: ChatToolArtifact[]
}

function describePythonError(err: unknown): string {
  if (err instanceof Error) {
    return compactErrorMessage(err.message || err.stack || err.name || String(err))
  }
  if (typeof err === 'string') return err
  try {
    return compactErrorMessage(JSON.stringify(err))
  } catch {
    return compactErrorMessage(String(err))
  }
}

function pythonStringLiteral(value: string): string {
  return JSON.stringify(value)
}

export function sanitizePythonInputName(name: string): string {
  const base = name.split(/[/\\]/).pop()?.trim() ?? ''
  const sanitized = base
    .replace(/[^\p{L}\p{N}._ -]/gu, '_')
    .replace(/^[. _-]+|[. _-]+$/g, '')
  return sanitized || 'input'
}

export function wrapPythonUserCode(code: string): string {
  return `
import traceback as _kivio_traceback
import warnings as _kivio_warnings

for _kivio_warning_category in (
    DeprecationWarning,
    PendingDeprecationWarning,
    FutureWarning,
    ResourceWarning,
):
    _kivio_warnings.filterwarnings("ignore", category=_kivio_warning_category)

try:
    exec(${pythonStringLiteral(code)}, globals(), globals())
except BaseException:
    _kivio_traceback.print_exc()
    raise
`.trim()
}

function stripPythonOutputLabel(message: string): string {
  return message
    .replace(/\bstderr:\s*/i, '')
    .replace(/\bstdout:\s*/i, '')
    .trim()
}

function cleanPythonExceptionSnippet(message: string): string {
  const normalized = message.replace(/\s+/g, ' ').trim()
  const stackBoundary = normalized.search(
    /\s+(?=Traceback \(most recent call last\):|File\s+"|File\s+'|await CodeRunner\(|coroutine =|new_error@|[0-9]+@wasm-function|\^+)/,
  )
  const clipped = stackBoundary >= 0 ? normalized.slice(0, stackBoundary) : normalized
  return compactErrorMessage(clipped)
}

function summarizePythonTraceback(message: string): string {
  const cleaned = stripPythonOutputLabel(message)
  const stackNoise = /(pyodide\.asm\.js|wasm-function|new_error@|_pyodide)/i
  const exceptionName = /^[A-Za-z_][\w.]*(Error|Exception|Warning|Interrupt|Exit|Fault|Found|Denied|Timeout)\b/
  const lines = cleaned
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean)
  const exceptionLine = [...lines]
    .reverse()
    .find((line) => exceptionName.test(line) && !stackNoise.test(line) && !line.startsWith('PythonError: Traceback'))
  if (exceptionLine) return cleanPythonExceptionSnippet(exceptionLine)

  const inlineExceptions = [
    ...cleaned.matchAll(
      /\b([A-Za-z_][\w.]*(?:Error|Exception|Warning|Interrupt|Exit|Fault|Found|Denied|Timeout)\b(?::\s*[^。\r\n]+)?)/g,
    ),
  ]
    .map((match) => cleanPythonExceptionSnippet(match[1] || ''))
    .filter((value) => value && !stackNoise.test(value) && !value.startsWith('PythonError: Traceback'))
  const inlineException = inlineExceptions.reverse()[0]
  return compactErrorMessage(inlineException || cleaned)
}

function detectPyodidePackages(code: string): string[] {
  const packages = PYODIDE_PACKAGE_IMPORTS
    .filter(([pattern]) => pattern.test(code))
    .map(([, packageName]) => packageName)
  return [...new Set(packages)]
}

function packageNameForImport(moduleName: string): string | null {
  const rootName = moduleName.split('.')[0]?.trim()
  if (!rootName) return null
  return PYODIDE_IMPORT_PACKAGE_ALIASES[rootName] ?? rootName
}

function missingModulePackageName(message: string): string | null {
  const patterns = [
    /ModuleNotFoundError:\s*No module named ['"]([^'"]+)['"]/i,
    /ImportError:\s*No module named ['"]([^'"]+)['"]/i,
    /No known package with name ['"]([^'"]+)['"]/i,
  ]
  for (const pattern of patterns) {
    const match = message.match(pattern)
    const packageName = match?.[1] ? packageNameForImport(match[1]) : null
    if (packageName) return packageName
  }
  return null
}

async function installSandboxPackage(pyodide: PyodideInterface, packageName: string): Promise<void> {
  try {
    await installPackagesOnRuntime(pyodide, [packageName], await loadPackageManifest())
  } catch {
    await installMicropipPackage(pyodide, packageName)
  }
}

function codeUsesMatplotlib(code: string): boolean {
  return /(^|\n)\s*(import|from)\s+matplotlib\b/.test(code)
}

function preparePythonCode(code: string): string {
  if (!codeUsesMatplotlib(code)) return code
  return `
import os
os.environ["MPLBACKEND"] = "Agg"
import matplotlib
matplotlib.use("Agg", force=True)
try:
    _kivio_configure_matplotlib_fonts()
except NameError:
    pass
import matplotlib.pyplot as _kivio_matplotlib_pyplot
_kivio_matplotlib_pyplot.ioff()

${code}
`.trim()
}

async function formatPythonOutput(pyodide: PyodideInterface): Promise<PythonOutputResult> {
  const stdout = String(await pyodide.runPythonAsync('_stdout.getvalue()'))
  const stderr = String(await pyodide.runPythonAsync('_stderr.getvalue()'))
  const stdoutResult = captureInlineImageArtifacts(stdout, 'python-output')
  const stderrResult = captureInlineImageArtifacts(stderr, 'python-error-output')
  let content = ''
  if (stdoutResult.text.trim()) {
    content += `stdout:\n${stdoutResult.text}`
    if (!stdoutResult.text.endsWith('\n')) content += '\n'
  }
  if (stderrResult.text.trim()) {
    content += `stderr:\n${stderrResult.text}`
    if (!stderrResult.text.endsWith('\n')) content += '\n'
  }
  if (!content.trim()) {
    content = '(no output)\n'
  }
  return {
    content,
    artifacts: [...stdoutResult.artifacts, ...stderrResult.artifacts],
  }
}

function shouldRetryMatplotlibExecution(code: string, message: string): boolean {
  if (!codeUsesMatplotlib(code)) return false
  const lower = message.toLowerCase()
  return (
    lower.includes('pyodide.asm.js') ||
    lower.includes('wasm-function') ||
    lower.includes('matplotlib') ||
    lower.includes('backend')
  )
}

async function warmMatplotlib(pyodide: PyodideInterface): Promise<void> {
  await pyodide.runPythonAsync(`
import os
os.environ["MPLBACKEND"] = "Agg"
import matplotlib
matplotlib.use("Agg", force=True)
_kivio_configure_matplotlib_fonts()
import matplotlib.pyplot as _kivio_matplotlib_warmup_pyplot
_kivio_matplotlib_warmup_pyplot.figure()
_kivio_matplotlib_warmup_pyplot.close("all")
`.trim())
}

async function executePython(
  pyodide: PyodideInterface,
  code: string,
  timeoutMs: number,
): Promise<PythonExecutionSuccess> {
  const fs = pyodide.FS as PyodideFs
  const initialCwd = await getPythonCwd(pyodide)
  const initialRoots = collectScanRoots(fs, initialCwd)
  const beforeArtifacts = scanArtifactFiles(fs, initialRoots)
  await pyodide.runPythonAsync(`
import sys
from io import StringIO
_stdout = StringIO()
_stderr = StringIO()
sys.stdout = _stdout
sys.stderr = _stderr
`)

  try {
    await Promise.race([
      pyodide.runPythonAsync(wrapPythonUserCode(code)),
      new Promise<never>((_, reject) => {
        self.setTimeout(
          () => reject(new Error(`Python execution timed out after ${timeoutMs}ms`)),
          timeoutMs,
        )
      }),
    ])
  } catch (err) {
    const output = await formatPythonOutput(pyodide)
    const message = describePythonError(err)
    const content = output.content.trim() && output.content.trim() !== '(no output)'
      ? output.content
      : message
    throw Object.assign(new Error(content), { artifacts: output.artifacts })
  }
  const output = await formatPythonOutput(pyodide)
  const finalCwd = await getPythonCwd(pyodide)
  const scanRoots = collectScanRoots(fs, finalCwd)
  const artifacts = appendUniqueArtifacts(
    output.artifacts,
    collectNewFileArtifacts(
      pyodide,
      beforeArtifacts,
      [...new Set([...initialRoots, ...scanRoots])],
      finalCwd ?? initialCwd,
    ),
  )
  return { content: output.content, artifacts }
}

function pathBasename(path: string): string {
  return path.split('/').filter(Boolean).pop() || path
}

function normalizeVirtualPath(path: string): string {
  const normalized = path.replace(/\/+/g, '/')
  if (normalized === '/') return normalized
  return normalized.replace(/\/$/, '')
}

function joinVirtualPath(parent: string, child: string): string {
  return normalizeVirtualPath(`${parent.replace(/\/$/, '')}/${child}`)
}

function artifactMimeType(path: string): string {
  const lower = path.toLowerCase()
  if (lower.endsWith('.png')) return 'image/png'
  if (lower.endsWith('.jpg') || lower.endsWith('.jpeg')) return 'image/jpeg'
  if (lower.endsWith('.gif')) return 'image/gif'
  if (lower.endsWith('.webp')) return 'image/webp'
  if (lower.endsWith('.svg')) return 'image/svg+xml'
  if (lower.endsWith('.csv')) return 'text/csv'
  if (lower.endsWith('.tsv')) return 'text/tab-separated-values'
  if (lower.endsWith('.json')) return 'application/json'
  if (lower.endsWith('.md')) return 'text/markdown'
  if (lower.endsWith('.txt')) return 'text/plain'
  if (lower.endsWith('.html') || lower.endsWith('.htm')) return 'text/html'
  if (lower.endsWith('.xlsx')) {
    return 'application/vnd.openxmlformats-officedocument.spreadsheetml.sheet'
  }
  if (lower.endsWith('.xls')) return 'application/vnd.ms-excel'
  return 'application/octet-stream'
}

function imageExtensionForMimeType(mimeType: string): string {
  if (mimeType === 'image/png') return 'png'
  if (mimeType === 'image/jpeg') return 'jpg'
  if (mimeType === 'image/gif') return 'gif'
  if (mimeType === 'image/webp') return 'webp'
  if (mimeType === 'image/svg+xml') return 'svg'
  return 'img'
}

function inferImageMimeType(bytes: Uint8Array): string | null {
  if (
    bytes.length >= 8 &&
    bytes[0] === 0x89 &&
    bytes[1] === 0x50 &&
    bytes[2] === 0x4e &&
    bytes[3] === 0x47 &&
    bytes[4] === 0x0d &&
    bytes[5] === 0x0a &&
    bytes[6] === 0x1a &&
    bytes[7] === 0x0a
  ) {
    return 'image/png'
  }
  if (bytes.length >= 3 && bytes[0] === 0xff && bytes[1] === 0xd8 && bytes[2] === 0xff) {
    return 'image/jpeg'
  }
  if (
    bytes.length >= 6 &&
    bytes[0] === 0x47 &&
    bytes[1] === 0x49 &&
    bytes[2] === 0x46 &&
    bytes[3] === 0x38
  ) {
    return 'image/gif'
  }
  if (
    bytes.length >= 12 &&
    bytes[0] === 0x52 &&
    bytes[1] === 0x49 &&
    bytes[2] === 0x46 &&
    bytes[3] === 0x46 &&
    bytes[8] === 0x57 &&
    bytes[9] === 0x45 &&
    bytes[10] === 0x42 &&
    bytes[11] === 0x50
  ) {
    return 'image/webp'
  }
  const start = new TextDecoder('utf-8', { fatal: false }).decode(bytes.slice(0, 256)).trimStart()
  if (start.startsWith('<svg') || start.startsWith('<?xml')) {
    return 'image/svg+xml'
  }
  return null
}

function fileExtension(path: string): string {
  const name = pathBasename(path)
  const index = name.lastIndexOf('.')
  return index >= 0 ? name.slice(index).toLowerCase() : ''
}

function statMtimeMs(stat: PyodideFsStat): number {
  if (stat.mtime instanceof Date) return stat.mtime.getTime()
  if (typeof stat.mtime === 'number') return stat.mtime
  return 0
}

function statSize(stat: PyodideFsStat): number {
  return typeof stat.size === 'number' && Number.isFinite(stat.size) ? stat.size : 0
}

function pathExists(fs: PyodideFs, path: string): boolean {
  try {
    if (fs.analyzePath) return fs.analyzePath(path).exists
    fs.stat(path)
    return true
  } catch {
    return false
  }
}

async function getPythonCwd(pyodide: PyodideInterface): Promise<string | null> {
  try {
    return normalizeVirtualPath(String(await pyodide.runPythonAsync('import os\nos.getcwd()')))
  } catch {
    return null
  }
}

function collectScanRoots(fs: PyodideFs, cwd: string | null): string[] {
  const roots = [cwd, ...PYTHON_ARTIFACT_SCAN_ROOTS]
    .filter((path): path is string => Boolean(path))
    .map(normalizeVirtualPath)
  return [...new Set(roots)].filter((path) => pathExists(fs, path))
}

function shouldScanArtifactPath(path: string): boolean {
  const normalized = normalizeVirtualPath(path)
  if (normalized === PYTHON_INPUT_SCAN_ROOT || normalized.startsWith(`${PYTHON_INPUT_SCAN_ROOT}/`)) {
    return false
  }
  return PYTHON_ARTIFACT_EXTENSIONS.has(fileExtension(path))
}

function scanArtifactFiles(
  fs: PyodideFs,
  roots: string[],
): Map<string, PythonArtifactSnapshotEntry> {
  const files = new Map<string, PythonArtifactSnapshotEntry>()
  const visited = new Set<string>()
  const walk = (dir: string, depth: number) => {
    if (depth > 8 || visited.has(dir)) return
    visited.add(dir)
    let names: string[]
    try {
      names = fs.readdir(dir)
    } catch {
      return
    }
    for (const name of names) {
      if (name === '.' || name === '..') continue
      const path = joinVirtualPath(dir, name)
      let stat: PyodideFsStat
      try {
        stat = fs.stat(path)
      } catch {
        continue
      }
      if (fs.isDir(stat.mode)) {
        walk(path, depth + 1)
        continue
      }
      if (!fs.isFile(stat.mode) || !shouldScanArtifactPath(path)) {
        continue
      }
      files.set(path, {
        size: statSize(stat),
        mtimeMs: statMtimeMs(stat),
      })
    }
  }

  for (const root of roots) {
    walk(root, 0)
  }
  return files
}

function bytesToBase64(bytes: Uint8Array): string {
  let binary = ''
  const chunkSize = 0x8000
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    const chunk = bytes.subarray(offset, offset + chunkSize)
    binary += String.fromCharCode(...chunk)
  }
  return self.btoa(binary)
}

function base64ToBytes(value: string, maxBytes = MAX_PYTHON_ARTIFACT_BYTES): Uint8Array | null {
  const compact = value.replace(/\s+/g, '')
  if (!compact || compact.length > Math.ceil(maxBytes * 4 / 3) + 8) {
    return null
  }
  try {
    const binary = self.atob(compact)
    const bytes = new Uint8Array(binary.length)
    for (let index = 0; index < binary.length; index += 1) {
      bytes[index] = binary.charCodeAt(index)
    }
    return bytes
  } catch {
    return null
  }
}

async function mountPythonInputFiles(
  pyodide: PyodideInterface,
  files: PythonInputFile[] = [],
): Promise<string[]> {
  const fs = pyodide.FS as PyodideFs
  const inputDir = '/home/pyodide/kivio_inputs'
  try {
    fs.mkdirTree?.(inputDir)
  } catch {
    // Directory may already exist.
  }
  try {
    for (const entry of fs.readdir(inputDir)) {
      if (entry === '.' || entry === '..') continue
      const path = joinVirtualPath(inputDir, entry)
      const stat = fs.stat(path)
      if (fs.isFile(stat.mode)) {
        fs.unlink(path)
      }
    }
  } catch {
    // If cleanup fails, writing fresh numbered inputs below will still reset KIVIO_INPUT_FILES.
  }

  const mountedPaths: string[] = []
  files.forEach((file, index) => {
    const bytes = base64ToBytes(file.dataBase64, MAX_PYTHON_INPUT_FILE_BYTES)
    if (!bytes) {
      throw new Error(`Invalid run_python input file payload: ${file.name}`)
    }
    const safeName = sanitizePythonInputName(file.name)
    const virtualPath = `${inputDir}/${index + 1}-${safeName}`
    fs.writeFile(virtualPath, bytes)
    mountedPaths.push(virtualPath)
  })

  await pyodide.runPythonAsync(`
KIVIO_INPUT_FILES = ${pythonStringLiteral(JSON.stringify(mountedPaths))}
import json as _kivio_json
KIVIO_INPUT_FILES = _kivio_json.loads(KIVIO_INPUT_FILES)
`.trim())
  return mountedPaths
}

function toUint8Array(content: Uint8Array | ArrayBuffer | number[] | string): Uint8Array {
  if (content instanceof Uint8Array) return content
  if (content instanceof ArrayBuffer) return new Uint8Array(content)
  if (Array.isArray(content)) return Uint8Array.from(content)
  return new TextEncoder().encode(content)
}

function artifactNameForPath(path: string, cwd: string | null): string {
  const normalized = normalizeVirtualPath(path)
  const normalizedCwd = cwd ? normalizeVirtualPath(cwd) : ''
  if (normalizedCwd && normalized.startsWith(`${normalizedCwd}/`)) {
    return normalized.slice(normalizedCwd.length + 1)
  }
  return pathBasename(normalized)
}

function imageArtifactNotice(artifact: ChatToolArtifact): string {
  const size = artifact.sizeBytes ?? artifact.size_bytes
  const sizeText = typeof size === 'number' ? `, ${size} bytes` : ''
  return `[image artifact captured: ${artifact.name}${sizeText}. Do not repeat base64 or data URLs in the final answer; Kivio renders this image automatically.]`
}

function createInlineImageArtifact(
  baseName: string,
  mimeType: string,
  bytes: Uint8Array,
  index: number,
): ChatToolArtifact {
  const suffix = index > 1 ? `-${index}` : ''
  return {
    name: `${baseName}${suffix}.${imageExtensionForMimeType(mimeType)}`,
    mimeType,
    dataUrl: `data:${mimeType};base64,${bytesToBase64(bytes)}`,
    sizeBytes: bytes.byteLength,
  }
}

function captureInlineImageArtifacts(text: string, baseName: string): { text: string; artifacts: ChatToolArtifact[] } {
  const artifacts: ChatToolArtifact[] = []
  let nextIndex = 1
  let sanitized = text.replace(
    /data:(image\/(?:png|jpe?g|gif|webp|svg\+xml));base64,([a-z0-9+/=]{128,})/gi,
    (_match, rawMimeType: string, rawBase64: string) => {
      const bytes = base64ToBytes(rawBase64)
      if (!bytes) return _match
      const mimeType = rawMimeType.toLowerCase().replace('image/jpg', 'image/jpeg')
      const inferred = inferImageMimeType(bytes)
      if (inferred && inferred !== mimeType) return _match
      const artifact = createInlineImageArtifact(baseName, mimeType, bytes, nextIndex)
      nextIndex += 1
      artifacts.push(artifact)
      return imageArtifactNotice(artifact)
    },
  )

  sanitized = sanitized
    .split('\n')
    .map((line) => {
      const trimmed = line.trim()
      if (trimmed.length < 128 || !/^[a-z0-9+/=]+$/i.test(trimmed)) return line
      const bytes = base64ToBytes(trimmed)
      if (!bytes) return line
      const mimeType = inferImageMimeType(bytes)
      if (!mimeType) return line
      const artifact = createInlineImageArtifact(baseName, mimeType, bytes, nextIndex)
      nextIndex += 1
      artifacts.push(artifact)
      const prefix = line.slice(0, line.length - line.trimStart().length)
      return `${prefix}${imageArtifactNotice(artifact)}`
    })
    .join('\n')

  return { text: sanitized, artifacts }
}

function appendUniqueArtifacts(
  artifacts: ChatToolArtifact[],
  nextArtifacts: ChatToolArtifact[],
): ChatToolArtifact[] {
  const seen = new Set(
    artifacts.map((artifact) => artifact.dataUrl ?? artifact.data_url ?? artifact.name),
  )
  const combined = [...artifacts]
  for (const artifact of nextArtifacts) {
    const key = artifact.dataUrl ?? artifact.data_url ?? artifact.name
    if (!key || seen.has(key)) continue
    seen.add(key)
    combined.push(artifact)
  }
  return combined.slice(0, MAX_PYTHON_ARTIFACTS)
}

function collectNewFileArtifacts(
  pyodide: PyodideInterface,
  before: Map<string, PythonArtifactSnapshotEntry>,
  roots: string[],
  cwd: string | null,
): ChatToolArtifact[] {
  const fs = pyodide.FS as PyodideFs
  const after = scanArtifactFiles(fs, roots)
  const changedPaths = [...after.entries()]
    .filter(([path, stat]) => {
      const previous = before.get(path)
      return !previous || previous.size !== stat.size || previous.mtimeMs !== stat.mtimeMs
    })
    .sort((a, b) => b[1].mtimeMs - a[1].mtimeMs)
    .slice(0, MAX_PYTHON_ARTIFACTS)

  const artifacts: ChatToolArtifact[] = []
  for (const [path, stat] of changedPaths) {
    if (stat.size > MAX_PYTHON_ARTIFACT_BYTES) continue
    try {
      const bytes = toUint8Array(fs.readFile(path, { encoding: 'binary' }))
      const mimeType = artifactMimeType(path)
      artifacts.push({
        name: artifactNameForPath(path, cwd),
        mimeType,
        dataUrl: `data:${mimeType};base64,${bytesToBase64(bytes)}`,
        sizeBytes: bytes.byteLength,
      })
    } catch {
      // The visible Python output remains useful even if a generated artifact cannot be read.
    }
  }
  return artifacts
}

function prefixMountedFiles(content: string, mountedFiles: string[]): string {
  if (mountedFiles.length === 0) return content
  return `mounted files:\n${mountedFiles.join('\n')}\n${content}`
}

export async function runPythonInSandbox(
  code: string,
  timeoutMs: number,
  files: PythonInputFile[] = [],
): Promise<PythonRunOutcome> {
  const packages = detectPyodidePackages(code)
  const preparedCode = preparePythonCode(code)
  let pyodide: PyodideInterface
  try {
    pyodide = packages.length > 0
      ? await getPyodideWithPackages(packages)
      : await getPyodide()
  } catch (err) {
    return {
      content: `Python 沙盒当前不可用：${describePythonError(err)}。${PYTHON_SANDBOX_FAILURE_GUIDANCE}`,
      isError: true,
      artifacts: [],
    }
  }

  let mountedFiles: string[] = []
  try {
    await ensurePythonFontSupport(pyodide)
    mountedFiles = await mountPythonInputFiles(pyodide, files)
    if (codeUsesMatplotlib(code)) {
      await warmMatplotlib(pyodide)
    }
    const output = await executePython(pyodide, preparedCode, timeoutMs)
    output.content = prefixMountedFiles(output.content, mountedFiles)
    return { content: output.content, isError: false, artifacts: output.artifacts }
  } catch (err) {
    const message = describePythonError(err)
    if (shouldRetryMatplotlibExecution(code, message)) {
      try {
        await warmMatplotlib(pyodide)
        const retryOutput = await executePython(pyodide, preparedCode, timeoutMs)
        return {
          content: prefixMountedFiles(retryOutput.content, mountedFiles),
          isError: false,
          artifacts: retryOutput.artifacts,
        }
      } catch (retryErr) {
        const retryMessage = summarizePythonTraceback(describePythonError(retryErr))
        return {
          content: `Python 执行失败（已自动重试一次 matplotlib 初始化）：${retryMessage}。建议优先使用 Pillow / numpy 直接生成图片，除非确实需要 matplotlib 图表能力。`,
          isError: true,
          artifacts: [],
        }
      }
    }
    const lower = message.toLowerCase()
    if (lower.includes('timed out')) {
      return { content: `Python 执行超时：${message}`, isError: true, artifacts: [] }
    }
    const missingPackage = missingModulePackageName(message)
    if (missingPackage) {
      try {
        await installSandboxPackage(pyodide, missingPackage)
        const retryOutput = await executePython(pyodide, preparedCode, timeoutMs)
        return {
          content: prefixMountedFiles(retryOutput.content, mountedFiles),
          isError: false,
          artifacts: retryOutput.artifacts,
        }
      } catch (installOrRetryErr) {
        const retryMessage = summarizePythonTraceback(describePythonError(installOrRetryErr))
        return {
          content: `Python 执行失败：已尝试在沙盒内安装 ${missingPackage}，但仍未成功：${retryMessage}`,
          isError: true,
          artifacts: [],
        }
      }
    }
    const summary = summarizePythonTraceback(message)
    if (message.includes('SyntaxError') || lower.includes('syntaxerror')) {
      return { content: `Python 语法错误：${summary}`, isError: true, artifacts: [] }
    }
    return { content: `Python 执行失败：${summary}`, isError: true, artifacts: [] }
  }
}
