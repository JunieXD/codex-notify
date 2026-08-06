[CmdletBinding()]
param(
    [string]$Version = "latest",
    [string]$Repository = "JunieXD/codex-notify",
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "codex-notify\bin")
)

$ErrorActionPreference = "Stop"
$target = "x86_64-pc-windows-msvc"
$asset = "codex-notify-$target.zip"

if ($Version -eq "latest") {
    $downloadBase = "https://github.com/$Repository/releases/latest/download"
} else {
    $downloadBase = "https://github.com/$Repository/releases/download/$Version"
}

$tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("codex-notify-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tempDir | Out-Null

try {
    $archivePath = Join-Path $tempDir $asset
    $checksumsPath = Join-Path $tempDir "SHA256SUMS"

    Write-Host "Downloading codex-notify for $target..."
    Invoke-WebRequest -Uri "$downloadBase/$asset" -OutFile $archivePath
    Invoke-WebRequest -Uri "$downloadBase/SHA256SUMS" -OutFile $checksumsPath

    $checksumLine = Get-Content $checksumsPath | Where-Object {
        $_ -match ("\s" + [regex]::Escape($asset) + "$")
    } | Select-Object -First 1
    if (-not $checksumLine) {
        throw "No checksum was found for $asset."
    }

    $expectedHash = ($checksumLine -split "\s+")[0].ToLowerInvariant()
    $actualHash = (Get-FileHash -Algorithm SHA256 -Path $archivePath).Hash.ToLowerInvariant()
    if ($actualHash -ne $expectedHash) {
        throw "SHA-256 verification failed for $asset."
    }

    $extractDir = Join-Path $tempDir "extract"
    Expand-Archive -Path $archivePath -DestinationPath $extractDir
    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    Copy-Item -Force (Join-Path $extractDir "codex-notify.exe") (Join-Path $InstallDir "codex-notify.exe")
} finally {
    Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
}

Write-Host "Installed codex-notify to $(Join-Path $InstallDir 'codex-notify.exe')"
$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$pathEntries = @($userPath -split ";" | Where-Object { $_ })
if ($pathEntries -notcontains $InstallDir) {
    Write-Host "Add this directory to your user PATH, then open a new terminal:"
    Write-Host "  $InstallDir"
}
