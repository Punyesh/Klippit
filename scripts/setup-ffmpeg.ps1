<#
.SYNOPSIS
  Downloads the latest Windows ffmpeg/ffprobe build and installs them into
  src-tauri/binaries/ with the exact naming Tauri's sidecar mechanism
  requires (name-<target-triple>.exe).

.DESCRIPTION
  Mirrors the manual steps in the README's "Bundling ffmpeg" section:
    1. detect this machine's Rust target triple (rustc -Vv)
    2. download gyan.dev's "release essentials" build (a permalink that
       always points at the current release — no version number to track)
    3. unzip it
    4. copy+rename ffmpeg.exe/ffprobe.exe into src-tauri/binaries/
    5. clean up the temp download

  Safe to re-run any time — e.g. to pick up a newer ffmpeg release later.
  Run from anywhere; it locates the repo root relative to this script's
  own location, not the current working directory.

.NOTES
  STATUS: written but not run in this environment (no Windows/PowerShell
  here to test against). Sanity-checked by eye for syntax and the gyan.dev
  URL/archive-layout assumptions below, not by an actual execution — if
  gyan.dev changes their folder layout, only the $ffmpegExe/$ffprobeExe
  search patterns below should need adjusting.
#>

$ErrorActionPreference = 'Stop'

$repoRoot   = Split-Path -Parent $PSScriptRoot
$binariesDir = Join-Path $repoRoot 'src-tauri\binaries'
$tempDir     = Join-Path $env:TEMP 'klippit-ffmpeg-setup'
$zipPath     = Join-Path $tempDir 'ffmpeg-release-essentials.zip'
$ffmpegUrl   = 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip'

Write-Host "Klippit ffmpeg setup" -ForegroundColor Cyan

# ---------- 1. detect target triple ----------
$rustcOutput = & rustc -Vv 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Error "rustc not found or failed to run. Install Rust first (see README) before running this script."
    exit 1
}
$hostLine = $rustcOutput | Where-Object { $_ -match '^host:\s*(.+)$' }
if (-not $hostLine) {
    Write-Error "Couldn't parse a 'host:' line out of 'rustc -Vv' output. Run 'rustc -Vv' yourself and check what it prints."
    exit 1
}
$triple = ($hostLine -replace '^host:\s*', '').Trim()
Write-Host "Target triple: $triple"

# ---------- 2. download ----------
if (Test-Path $tempDir) { Remove-Item $tempDir -Recurse -Force }
New-Item -ItemType Directory -Path $tempDir | Out-Null

Write-Host "Downloading ffmpeg (gyan.dev release-essentials build)..."
Invoke-WebRequest -Uri $ffmpegUrl -OutFile $zipPath

# ---------- 3. extract ----------
Write-Host "Extracting..."
$extractDir = Join-Path $tempDir 'extracted'
Expand-Archive -Path $zipPath -DestinationPath $extractDir

# The archive's top-level folder name changes with every ffmpeg version
# (e.g. ffmpeg-7.1.1-essentials_build), so search for bin\ffmpeg.exe
# rather than hardcoding a version-specific path.
$ffmpegExe  = Get-ChildItem -Path $extractDir -Recurse -Filter 'ffmpeg.exe'  | Select-Object -First 1
$ffprobeExe = Get-ChildItem -Path $extractDir -Recurse -Filter 'ffprobe.exe' | Select-Object -First 1

if (-not $ffmpegExe -or -not $ffprobeExe) {
    Write-Error "Couldn't find ffmpeg.exe/ffprobe.exe inside the downloaded archive. gyan.dev may have changed their folder layout — check $extractDir by hand."
    exit 1
}

# ---------- 4. copy + rename into place ----------
if (-not (Test-Path $binariesDir)) {
    New-Item -ItemType Directory -Path $binariesDir | Out-Null
}

$ffmpegDest  = Join-Path $binariesDir "ffmpeg-$triple.exe"
$ffprobeDest = Join-Path $binariesDir "ffprobe-$triple.exe"

Copy-Item $ffmpegExe.FullName  $ffmpegDest  -Force
Copy-Item $ffprobeExe.FullName $ffprobeDest -Force

Write-Host "Installed:" -ForegroundColor Green
Write-Host "  $ffmpegDest"
Write-Host "  $ffprobeDest"

# ---------- 5. clean up ----------
Remove-Item $tempDir -Recurse -Force

Write-Host "Done. You can now run 'cargo tauri dev' or 'cargo tauri build'." -ForegroundColor Green
