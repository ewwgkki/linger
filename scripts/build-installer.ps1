# Builds Linger-Setup.exe. Put the audio-only build from build-ffmpeg.sh at
# src-tauri\binaries\installer\ffmpeg.exe first.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
if (-not (Test-Path 'src-tauri\binaries\installer\ffmpeg.exe')) { throw 'Missing src-tauri\binaries\installer\ffmpeg.exe' }
# cargo logs to stderr, which Windows PowerShell would otherwise treat as a failure.
$ErrorActionPreference = 'Continue'
cargo tauri build --bundles nsis --config src-tauri/tauri.installer.conf.json
$buildExit = $LASTEXITCODE
$ErrorActionPreference = 'Stop'
if ($buildExit -ne 0) { throw 'Build failed' }
$setup = Get-ChildItem 'src-tauri\target\release\bundle\nsis\*-setup.exe' | Sort-Object LastWriteTime | Select-Object -Last 1
Copy-Item $setup.FullName 'src-tauri\target\release\bundle\nsis\Linger-Setup.exe' -Force
Write-Host "Built src-tauri\target\release\bundle\nsis\Linger-Setup.exe"
