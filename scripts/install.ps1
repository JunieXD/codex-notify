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

    Write-Warning "无法删除安装临时目录：$Path"
}

function Get-EnabledSystemHttpsProxy {
    try {
        $settings = Get-ItemProperty `
            -Path "HKCU:\Software\Microsoft\Windows\CurrentVersion\Internet Settings" `
            -ErrorAction Stop
        if ([int]$settings.ProxyEnable -ne 1) {
            return $null
        }

        $server = ([string]$settings.ProxyServer).Trim()
        if (-not $server) {
            return $null
        }
        if ($server.Contains("=")) {
            $httpsEntry = @(
                $server.Split(";") | Where-Object { $_ -match '^\s*https\s*=' }
            ) | Select-Object -First 1
            if (-not $httpsEntry) {
                return $null
            }
            $server = ($httpsEntry -split "=", 2)[1].Trim()
        }
        if (-not $server) {
            return $null
        }
        if ($server -notmatch '^[a-zA-Z][a-zA-Z0-9+.-]*://') {
            $server = "http://$server"
        }
        return $server
    } catch {
        return $null
    }
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
    Write-Host "检测到已安装 codex-notify，正在安全升级……"
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

    $injectedSystemProxy = $false
    if (-not $env:HTTPS_PROXY -and -not $env:ALL_PROXY) {
        $systemProxy = Get-EnabledSystemHttpsProxy
        if ($systemProxy) {
            $env:HTTPS_PROXY = $systemProxy
            $injectedSystemProxy = $true
            Write-Host "检测到 Windows 系统代理，正在通过该代理升级……"
        }
    }
    try {
        & $targetPath @updateArgs
        if ($LASTEXITCODE -ne 0) {
            throw "codex-notify 未能完成升级，原有安装已保留。"
        }
    } finally {
        if ($injectedSystemProxy) {
            Remove-Item Env:HTTPS_PROXY -ErrorAction SilentlyContinue
        }
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

    Write-Host "正在下载适用于 $target 的 codex-notify……"
    Invoke-WebRequest -Uri "$resolvedDownloadBase/$asset" -OutFile $archivePath
    Invoke-WebRequest -Uri "$resolvedDownloadBase/SHA256SUMS" -OutFile $checksumsPath
    if ((Get-Item -LiteralPath $archivePath).Length -gt 134217728 -or
        (Get-Item -LiteralPath $checksumsPath).Length -gt 1048576) {
        throw "下载的发行版超过允许大小，已停止安装。"
    }

    $checksumMatches = @(
        Get-Content -LiteralPath $checksumsPath | ForEach-Object {
            if ($_ -match '^([0-9a-fA-F]{64})\s+\*?(.+)$' -and $Matches[2] -eq $asset) {
                $Matches[1].ToLowerInvariant()
            }
        }
    )
    if ($checksumMatches.Count -ne 1) {
        throw "SHA256SUMS 必须包含且只能包含一个 $asset 的有效校验值。"
    }

    $expectedHash = $checksumMatches[0]
    $actualHash = (Get-FileHash -Algorithm SHA256 -LiteralPath $archivePath).Hash.ToLowerInvariant()
    if ($actualHash -ne $expectedHash) {
        throw "$asset 的 SHA-256 校验失败，已停止安装。"
    }
    Write-Host "$asset：SHA-256 校验通过。"

    $extractDir = Join-Path $tempDir "extract"
    Expand-Archive -LiteralPath $archivePath -DestinationPath $extractDir
    $preparedPath = Join-Path $extractDir "codex-notify.exe"
    if (-not (Test-Path -LiteralPath $preparedPath -PathType Leaf)) {
        throw "发行版压缩包中没有 codex-notify.exe。"
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
            throw "codex-notify 未能完成安装。"
        }
    } elseif (-not (Test-Path -LiteralPath $targetPath)) {
        # Compatibility for a first install while the latest GitHub Release
        # still predates the built-in updater.
        New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
        Copy-Item -LiteralPath $preparedPath -Destination $targetPath
        Write-Host "codex-notify 已安装到 $targetPath"
    } else {
        $installedVersion = (& $targetPath --version 2>$null | Out-String).Trim()
        $downloadedVersion = (& $preparedPath --version 2>$null | Out-String).Trim()
        if ($installedVersion -and $installedVersion -eq $downloadedVersion) {
            Write-Host "codex-notify 已是最新版（$($installedVersion.Replace('codex-notify ', ''))）。"
        } else {
            throw "这个旧发行版无法安全升级现有安装。请明确安装较新的 codex-notify 发行版后重试。"
        }
    }
} finally {
    Remove-InstallTempDirectory -Path $tempDir
}

$userPath = [Environment]::GetEnvironmentVariable("Path", "User")
$pathEntries = @($userPath -split ";" | Where-Object { $_ })
if ($pathEntries -notcontains $InstallDir) {
    Write-Host "请将以下目录加入用户 PATH，然后重新打开终端："
    Write-Host "  $InstallDir"
}
