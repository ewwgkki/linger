# Downloads a GPL win64 FFmpeg build (BtbN/FFmpeg-Builds) for development and extracts only
# ffmpeg.exe to src-tauri/binaries/ffmpeg-x86_64-pc-windows-msvc.exe (gitignored).
#   powershell -File scripts/fetch-ffmpeg.ps1 [-Force]
param([switch]$Force)
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'

$root = Split-Path -Parent $PSScriptRoot
$binDir = Join-Path $root 'src-tauri\binaries'
$dest = Join-Path $binDir 'ffmpeg-x86_64-pc-windows-msvc.exe'
if ((Test-Path $dest) -and -not $Force) {
    Write-Host "already present: $dest (use -Force to re-download)"
    exit 0
}

$url = 'https://github.com/BtbN/FFmpeg-Builds/releases/download/latest/ffmpeg-master-latest-win64-gpl.zip'
$tmp = Join-Path ([IO.Path]::GetTempPath()) ("linger-ffmpeg-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tmp | Out-Null
try {
    $zip = Join-Path $tmp 'ffmpeg.zip'
    Write-Host "downloading $url"
    [Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
    Invoke-WebRequest -Uri $url -OutFile $zip -UseBasicParsing

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $archive = [IO.Compression.ZipFile]::OpenRead($zip)
    try {
        $entry = $archive.Entries | Where-Object { $_.FullName -match '(^|/)bin/ffmpeg\.exe$' } | Select-Object -First 1
        if (-not $entry) { throw 'ffmpeg.exe not found in the archive' }
        New-Item -ItemType Directory -Force -Path $binDir | Out-Null
        $part = "$dest.part"
        [IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $part, $true)
        Move-Item -Force $part $dest
    } finally {
        $archive.Dispose()
    }
    & $dest -hide_banner -version | Select-Object -First 1
    Write-Host "installed: $dest"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
