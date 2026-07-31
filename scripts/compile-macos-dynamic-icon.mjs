import { existsSync, mkdirSync, rmSync } from 'node:fs'
import { dirname, join, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { spawnSync } from 'node:child_process'

const scriptDir = dirname(fileURLToPath(import.meta.url))
const projectRoot = resolve(scriptDir, '..')
const iconPackage = join(projectRoot, 'src-tauri/icons/AppIcon.icon')
const outputDir = join(projectRoot, 'src-tauri/generated/macos-icon')
const partialPlist = join(outputDir, 'partial-info.plist')

function run(command, args) {
  const result = spawnSync(command, args, {
    cwd: projectRoot,
    encoding: 'utf8',
    stdio: ['ignore', 'pipe', 'pipe'],
  })

  if (result.stdout) process.stdout.write(result.stdout)
  if (result.stderr) process.stderr.write(result.stderr)

  if (result.error) throw result.error
  if (result.status !== 0) {
    throw new Error(`${command} exited with status ${result.status}`)
  }
}

if (process.platform !== 'darwin') {
  console.log('Skipping Apple dynamic icon compilation on non-macOS host')
  process.exit(0)
}

if (!existsSync(join(iconPackage, 'icon.json'))) {
  throw new Error(`Missing Icon Composer document: ${iconPackage}`)
}

rmSync(outputDir, { recursive: true, force: true })
mkdirSync(outputDir, { recursive: true })

run('xcrun', [
  'actool',
  '--compile',
  outputDir,
  '--platform',
  'macosx',
  '--minimum-deployment-target',
  '14.0',
  '--app-icon',
  'AppIcon',
  '--output-partial-info-plist',
  partialPlist,
  iconPackage,
])

for (const output of ['AppIcon.icns', 'Assets.car', 'partial-info.plist']) {
  const outputPath = join(outputDir, output)
  if (!existsSync(outputPath)) {
    throw new Error(`actool did not produce ${outputPath}`)
  }
}

console.log(`Compiled Apple dynamic icon resources in ${outputDir}`)
