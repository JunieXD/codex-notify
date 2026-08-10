[CmdletBinding()]
param(
    [string]$Version = "latest",
    [string]$Repository = "JunieXD/codex-notify",
    [string]$InstallDir = (Join-Path $env:LOCALAPPDATA "codex-notify\bin"),
    [string]$DownloadBase = $env:CODEX_NOTIFY_DOWNLOAD_BASE,
    [switch]$Force
)

$ErrorActionPreference = "Stop"

function Remove-InstallTempDirectory {
    param([Parameter(Mandatory = $true)][string]$Path)

    $deadline = [DateTime]::UtcNow.AddSeconds(10)
    do {
        Remove-Item -LiteralPath $Path -Recurse -Force -ErrorAction SilentlyContinue
        if (-not (Test-Path -LiteralPath $Path)) {
            return
        }
        Start-Sleep -Milliseconds 100
    } while ([DateTime]::UtcNow -lt $deadline)

    Write-Warning "The temporary installer directory could not be removed: $Path"
}

$target = "x86_64-pc-windows-msvc"
$asset = "codex-notify-$target.zip"
$targetPath = Join-Path $InstallDir "codex-notify.exe"
$forceUpdate = $Force.IsPresent -or $env:CODEX_NOTIFY_FORCE_UPDATE -eq "1"

# Once installed, codex-notify owns the complete transaction: verify, stop the
# watcher, replace, refresh configuration, restart, and roll back on failure.
$supportsUpdate = $false
if (Test-Path -LiteralPath $targetPath -PathType Leaf) {
    try {
        & $targetPath update --help *> $null
        $supportsUpdate = $LASTEXITCODE -eq 0
    } catch {
        $supportsUpdate = $false
    }
}
if ($supportsUpdate) {
    Write-Host "Existing codex-notify installation found; starting a safe update..."
    $updateArgs = @("update", "--yes", "--repository", $Repository)
    if ($Version -ne "latest") {
        $updateArgs += @("--version", $Version)
    }
    if ($DownloadBase) {
        $updateArgs += @("--download-base", $DownloadBase)
    }
    if ($forceUpdate) {
        $updateArgs += "--force"
    }
    & $targetPath @updateArgs
    if ($LASTEXITCODE -ne 0) {
        throw "codex-notify could not complete the update. The previous installation was kept."
    }
    return
}

if ($DownloadBase) {
    $resolvedDownloadBase = $DownloadBase.TrimEnd('/')
} elseif ($Version -eq "latest") {
    $resolvedDownloadBase = "https://github.com/$Repository/releases/latest/download"
} else {
    $releaseTag = if ($Version.StartsWith('v')) { $Version } else { "v$Version" }
    $resolvedDownloadBase = "https://github.com/$Repository/releases/download/$releaseTag"
}

New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
$tempDir = Join-Path $InstallDir (".codex-notify-install-" + [guid]::NewGuid())
New-Item -ItemType Directory -Path $tempDir | Out-Null

try {
    $archivePath = Join-Path $tempDir $asset
    $checksumsPath = Join-Path $tempDir "SHA256SUMS"

    Write-Host "Downloading codex-notify for $target..."
    Invoke-WebRequest -Uri "$resolvedDownloadBase/$asset" -OutFile $archivePath
    Invoke-WebRequest -Uri "$resolvedDownloadBase/SHA256SUMS" -OutFile $checksumsPath
    if ((Get-Item -LiteralPath $archivePath).Length -gt 134217728 -or
        (Get-Item -LiteralPath $checksumsPath).Length -gt 1048576) {
        throw "The downloaded release exceeds the allowed size."
    }

    $checksumMatches = @(
        Get-Content -LiteralPath $checksumsPath | ForEach-Object {
            if ($_ -match '^([0-9a-fA-F]{64})\s+\*?(.+)$' -and $Matches[2] -eq $asset) {
                $Matches[1].ToLowerInvariant()
            }
        }
    )
    if ($checksumMatches.Count -ne 1) {
        throw "SHA256SUMS must contain exactly one valid checksum for $asset."
    }

    $expectedHash = $checksumMatches[0]
    $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $archivePath).Hash.ToLowerInvariant()
    if ($actualHash -ne $expectedHash) {
        throw "SHA-256 verification failed for $asset."
    }
    Write-Host "$asset`: checksum verified."

    $extractDir = Join-Path $tempDir "extract"
    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractDir
    $preparedPath = Join-Path $extractDir "codex-notify.exe"
    if (-not (Test-Path -LiteralPath $preparedPath -PathType Leaf)) {
        throw "The release archive does not contain codex-notify.exe."
    }

    $supportsPreparedInstall = $false
    try {
        & $preparedPath install-prepared --help *> $null
        $supportsPreparedInstall = $LASTEXITCODE -eq 0
    } catch {
        $supportsPreparedInstall = $false
    }
    if ($supportsPreparedInstall) {
        $installArgs = @("install-prepared", "--target", $targetPath)
        if ($Version -ne "latest") {
            $installArgs += @("--expected-version", $Version)
        }
        if ($forceUpdate) {
            $installArgs += "--force"
        }
        & $preparedPath @installArgs
        if ($LASTEXITCODE -ne 0) {
            throw "codex-notify could not complete the installation."
        }
    } elseif (-not (Test-Path -LiteralPath $targetPath)) {
        # Compatibility for a first install while the latest GitHub Release
        # still predates the built-in updater.
        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
        Copy-Item -LiteralPath $preparedPath -Destination $targetPath
        Write-Host "Installed codex-notify to $targetPath"
    } else {
        $installedVersion = (& $targetPath --version 2>$null | Out-String).Trim()
        $downloadedVersion = (& $preparedPath --version 2>$null | Out-String).Trim()
        if ($installedVersion -and $installedVersion -eq $downloadedVersion) {
            Write-Host "codex-notify is already up to date ($($installedVersion.Replace('codex-notify ', '')))."
        } else {
            throw "This older release cannot safely upgrade an existing installation. Install a newer codex-notify release explicitly, then retry."
        }
    }
} finally {
    Remove-InstallTempDirectory -Path $tempDir
}

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$pathEntries = @($userPath -split ";" | Where-Object { $_ })
if ($pathEntries -notcontains $InstallDir) {
    Write-Host "Add this directory to your user PATH, then open a new terminal:"
    Write-Host "  $InstallDir"
}
