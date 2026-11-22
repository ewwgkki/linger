# Builds Linger-Setup.exe. Put the audio-only build from build-ffmpeg.sh at
# src-tauri\binaries\installer\ffmpeg.exe first.
$ErrorActionPreference = 'Stop'
$root = Split-Path $PSScriptRoot -Parent
Set-Location $root
if (-not (Test-Path 'src-tauri\binaries\installer\ffmpeg.exe')) { throw 'Missing src-tauri\binaries\installer\ffmpeg.exe' }
# Keep this machine's folders and account name out of the shipped binary.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE '.cargo' }
$rustupHome = if ($env:RUSTUP_HOME) { $env:RUSTUP_HOME } else { Join-Path $env:USERPROFILE '.rustup' }
$remaps = @(
  "--remap-path-prefix=$cargoHome=/cargo",
  "--remap-path-prefix=$rustupHome=/rustup",
  "--remap-path-prefix=$root=/linger",
  "--remap-path-prefix=$env:USERPROFILE=/home"
)
$env:CARGO_ENCODED_RUSTFLAGS = $remaps -join [char]0x1f
# cargo logs to stderr, which Windows PowerShell would otherwise treat as a failure.
$ErrorActionPreference = 'Continue'
cargo tauri build --bundles nsis --config src-tauri/tauri.installer.conf.json
$buildExit = $LASTEXITCODE
$ErrorActionPreference = 'Stop'
if ($buildExit -ne 0) { throw 'Build failed' }
$latin1 = [Text.Encoding]::GetEncoding(28591)
$needles = @($env:USERPROFILE, "Users\$env:USERNAME", "Users/$env:USERNAME", $root, ($root -replace '\\', '/'))
foreach ($bin in 'src-tauri\target\release\linger.exe', 'src-tauri\binaries\installer\ffmpeg.exe') {
  $text = $latin1.GetString([IO.File]::ReadAllBytes((Join-Path $root $bin)))
  foreach ($needle in $needles) {
    $utf8 = $latin1.GetString([Text.Encoding]::UTF8.GetBytes($needle))
    $utf16 = $latin1.GetString([Text.Encoding]::Unicode.GetBytes($needle))
    if ($text.IndexOf($utf8, [StringComparison]::OrdinalIgnoreCase) -ge 0 -or $text.IndexOf($utf16, [StringComparison]::OrdinalIgnoreCase) -ge 0) {
      throw "Private build path found in $bin"
    }
  }
}
Write-Host 'Privacy scan passed'
$setup = Get-ChildItem 'src-tauri\target\release\bundle\nsis\*-setup.exe' | Sort-Object LastWriteTime | Select-Object -Last 1
Copy-Item $setup.FullName 'src-tauri\target\release\bundle\nsis\Linger-Setup.exe' -Force
Remove-Item Env:CARGO_ENCODED_RUSTFLAGS -ErrorAction SilentlyContinue
Write-Host "Built src-tauri\target\release\bundle\nsis\Linger-Setup.exe"
