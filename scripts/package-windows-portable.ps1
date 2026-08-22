# Pack a Windows portable zip that mirrors the NSIS install layout:
# exe + sidecar + bundled skills, unzip-and-run, no Start Menu / uninstaller.
# Output: src-tauri/target/release/bundle/portable/Kivio.Desktop_${Version}_x64-portable.zip
#
# Requires a finished `tauri build --bundles nsis` (kivio.exe in target/release).

[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)]
  [string]$Version
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repoRoot = Resolve-Path (Join-Path $PSScriptRoot '..')
$releaseDir = Join-Path $repoRoot 'src-tauri\target\release'
$exe = Join-Path $releaseDir 'kivio.exe'
$skillsSrc = Join-Path $repoRoot 'src-tauri\resources\skills'
$sidecarSrc = Join-Path $repoRoot 'src-tauri\binaries\kivio-ocr-helper-x86_64-pc-windows-msvc.exe'

if (-not (Test-Path -LiteralPath $exe)) {
  throw "kivio.exe not found at $exe. Run tauri build first."
}
if (-not (Test-Path -LiteralPath (Join-Path $skillsSrc 'pdf\SKILL.md'))) {
  throw "Bundled skills missing under $skillsSrc."
}

$stageRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("kivio-portable-" + [guid]::NewGuid().ToString('n'))
$appDir = Join-Path $stageRoot 'Kivio Desktop'
New-Item -ItemType Directory -Path $appDir | Out-Null

Copy-Item -LiteralPath $exe -Destination (Join-Path $appDir 'Kivio Desktop.exe')
Copy-Item -LiteralPath $skillsSrc -Destination (Join-Path $appDir 'skills') -Recurse

if (Test-Path -LiteralPath $sidecarSrc) {
  Copy-Item -LiteralPath $sidecarSrc -Destination (Join-Path $appDir 'kivio-ocr-helper.exe')
}

Get-ChildItem -LiteralPath $releaseDir -File -Filter '*.dll' -ErrorAction SilentlyContinue |
  ForEach-Object { Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $appDir $_.Name) }

$readme = @"
Kivio Desktop 便携版 / Portable

解压到任意目录，双击「Kivio Desktop.exe」即可。不写注册表、不进开始菜单。
需要已安装 Microsoft Edge WebView2（Windows 10/11 通常已有）。
设置和对话仍保存在本机用户目录，和安装版共用。

Unzip anywhere and run Kivio Desktop.exe. No installer, no Start Menu.
Requires the Edge WebView2 runtime (already on most Windows 10/11 PCs).
Settings and chats stay in your user folder and are shared with the installed app.
"@
Set-Content -LiteralPath (Join-Path $appDir '使用说明.txt') -Value $readme -Encoding utf8

$outDir = Join-Path $repoRoot 'src-tauri\target\release\bundle\portable'
New-Item -ItemType Directory -Path $outDir -Force | Out-Null
$zipName = "Kivio.Desktop_${Version}_x64-portable.zip"
$zipPath = Join-Path $outDir $zipName
if (Test-Path -LiteralPath $zipPath) {
  Remove-Item -LiteralPath $zipPath -Force
}

Compress-Archive -Path $appDir -DestinationPath $zipPath -CompressionLevel Optimal
Remove-Item -LiteralPath $stageRoot -Recurse -Force

if (-not (Test-Path -LiteralPath $zipPath) -or (Get-Item -LiteralPath $zipPath).Length -lt 1MB) {
  throw "Portable zip missing or too small: $zipPath"
}

Write-Host "Wrote $zipPath"
