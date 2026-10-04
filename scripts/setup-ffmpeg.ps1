<#
.SYNOPSIS
  Downloads a current Windows ffmpeg/ffprobe build and installs it into
  src-tauri/binaries/ using Tauri sidecar naming.

.DESCRIPTION
  Klippit uses FFmpeg/ffprobe for editing, export, and recorder finalization.
  Live screen capture is native and does not depend on FFmpeg capture filters.
  This script:
    1. detects the Rust target triple
    2. downloads gyan.dev's release-essentials build
    3. extracts ffmpeg.exe and ffprobe.exe
    4. copies them into src-tauri/binaries/
    5. verifies both sidecars can run

  It is safe to run again later to refresh the bundled FFmpeg build.
#>

$ErrorActionPreference = 'Stop'

$repoRoot    = Split-Path -Parent $PSScriptRoot
$binariesDir = Join-Path $repoRoot 'src-tauri\binaries'
$tempDir     = Join-Path $env:TEMP 'klippit-ffmpeg-setup'
$zipPath     = Join-Path $tempDir 'ffmpeg-release-essentials.zip'
$ffmpegUrl   = 'https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip'

Write-Host "Klippit FFmpeg setup" -ForegroundColor Cyan

# 1. Detect target triple.
$rustcOutput = & rustc -Vv 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Error "rustc was not found or failed to run. Install Rust first, then run this script again."
    exit 1
}

$hostLine = $rustcOutput | Where-Object { $_ -match '^host:\s*(.+)$' }
if (-not $hostLine) {
    Write-Error "Could not parse the host line from rustc -Vv output."
    exit 1
}

$triple = ($hostLine -replace '^host:\s*', '').Trim()
Write-Host "Target triple: $triple"

# 2. Download.
if (Test-Path $tempDir) {
    Remove-Item $tempDir -Recurse -Force
}
New-Item -ItemType Directory -Path $tempDir | Out-Null

Write-Host "Downloading FFmpeg from gyan.dev..."
Invoke-WebRequest -Uri $ffmpegUrl -OutFile $zipPath

# 3. Extract.
Write-Host "Extracting..."
$extractDir = Join-Path $tempDir 'extracted'
Expand-Archive -Path $zipPath -DestinationPath $extractDir

$ffmpegExe  = Get-ChildItem -Path $extractDir -Recurse -Filter 'ffmpeg.exe'  | Select-Object -First 1
$ffprobeExe = Get-ChildItem -Path $extractDir -Recurse -Filter 'ffprobe.exe' | Select-Object -First 1

if (-not $ffmpegExe -or -not $ffprobeExe) {
    Write-Error "Could not find ffmpeg.exe or ffprobe.exe in the downloaded archive."
    exit 1
}

# 4. Copy and rename into place.
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

# 5. Verify the sidecars run.
Write-Host "Verifying FFmpeg and ffprobe..."
& $ffmpegDest -hide_banner -version | Select-Object -First 1
if ($LASTEXITCODE -ne 0) {
    Write-Error "The installed ffmpeg sidecar failed to run."
    exit 1
}
& $ffprobeDest -hide_banner -version | Select-Object -First 1
if ($LASTEXITCODE -ne 0) {
    Write-Error "The installed ffprobe sidecar failed to run."
    exit 1
}
Write-Host "FFmpeg + ffprobe OK." -ForegroundColor Green

# 6. Clean up.
Remove-Item $tempDir -Recurse -Force

Write-Host "Done. You can now run cargo tauri dev or cargo tauri build." -ForegroundColor Green
