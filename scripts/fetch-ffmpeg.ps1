<#
Downloads the pinned ffmpeg build and installs ffmpeg.exe / ffprobe.exe into
src-tauri/binaries/ using Tauri's sidecar naming (<name>-<target-triple>.exe).

Build: gyan.dev "full" release 9.0.2 (GPL; includes libx264, libx265, libmp3lame,
libvpx, libopus). The binaries ship as separate executables, never linked.
The SHA256 below was computed by hand on the downloaded asset; the script fails on mismatch.

Usage (from the repo root):
  powershell.exe -ExecutionPolicy Bypass -File scripts/fetch-ffmpeg.ps1
#>
$ErrorActionPreference = 'Stop'

$Version = '9.0.2'
$Url     = "https://github.com/GyanD/codexffmpeg/releases/download/$Version/ffmpeg-$Version-full_build.zip"
$Sha256  = '759d0a9831c436a0eb331ad36f236c06cb04aaa0005da46571f0e9d3d9206f6b'
$Triple  = 'x86_64-pc-windows-msvc'

$Root    = Split-Path -Parent $PSScriptRoot
$OutDir  = Join-Path $Root 'src-tauri\binaries'
$Targets = @{
    'ffmpeg.exe'  = Join-Path $OutDir "ffmpeg-$Triple.exe"
    'ffprobe.exe' = Join-Path $OutDir "ffprobe-$Triple.exe"
}

$Missing = $Targets.Values | Where-Object { -not (Test-Path $_) }
if (-not $Missing) {
    Write-Host "ffmpeg $Version binaries already present in $OutDir"
    exit 0
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
$Zip = Join-Path ([IO.Path]::GetTempPath()) "hasami-ffmpeg-$Version.zip"

try {
    Write-Host "Downloading $Url"
    $ProgressPreference = 'SilentlyContinue'
    Invoke-WebRequest -Uri $Url -OutFile $Zip -UseBasicParsing

    $Actual = (Get-FileHash -Path $Zip -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($Actual -ne $Sha256) {
        throw "SHA256 mismatch for ffmpeg $Version. Expected $Sha256, got $Actual."
    }
    Write-Host "SHA256 verified."

    Add-Type -AssemblyName System.IO.Compression.FileSystem
    $Archive = [IO.Compression.ZipFile]::OpenRead($Zip)
    try {
        foreach ($Name in $Targets.Keys) {
            $Entry = $Archive.Entries | Where-Object { $_.FullName -match "^[^/]+/bin/$([regex]::Escape($Name))$" } | Select-Object -First 1
            if (-not $Entry) { throw "$Name not found in the ffmpeg archive." }
            [IO.Compression.ZipFileExtensions]::ExtractToFile($Entry, $Targets[$Name], $true)
            Write-Host "Installed $($Targets[$Name])"
        }
    }
    finally {
        $Archive.Dispose()
    }
}
finally {
    Remove-Item -Force -ErrorAction SilentlyContinue $Zip
}
