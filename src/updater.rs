//! Download, verify, and stage codex-notify release binaries.

use anyhow::{Context, Result, anyhow, bail};
use flate2::read::GzDecoder;
use reqwest::blocking::{Client, Response};
use reqwest::{NoProxy, Proxy, Url};
use semver::Version;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::env;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use tempfile::{TempDir, TempPath};
#[cfg(windows)]
use winreg::RegKey;
#[cfg(windows)]
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ};

pub const DEFAULT_REPOSITORY: &str = "JunieXD/codex-notify";
const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;
const MAX_CHECKSUM_BYTES: u64 = 1024 * 1024;
#[cfg(windows)]
const WINDOWS_INTERNET_SETTINGS_KEY: &str =
    r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

#[derive(Debug, Clone, Default)]
pub struct NetworkOptions {
    proxy: Option<String>,
    no_proxy: bool,
}

impl NetworkOptions {
    pub fn new(proxy: Option<String>, no_proxy: bool) -> Self {
        Self { proxy, no_proxy }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NetworkRoute {
    ExplicitProxy,
    EnvironmentProxy,
    #[cfg_attr(not(windows), allow(dead_code))]
    WindowsSystemProxy,
    ProxyDisabled,
    Direct,
}

#[derive(Debug)]
struct NetworkClient {
    client: Client,
    route: NetworkRoute,
}

#[cfg(windows)]
#[derive(Debug, Clone, PartialEq, Eq)]
struct SystemProxySettings {
    address: String,
    bypass: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub tag: String,
    pub version: Version,
    pub asset_name: String,
    download_base: String,
}

#[derive(Debug)]
pub struct PreparedRelease {
    pub info: ReleaseInfo,
    pub executable: PathBuf,
    _directory: TempDir,
}

#[derive(Debug)]
pub struct ExecutableBackup {
    path: TempPath,
}

impl ExecutableBackup {
    pub fn path(&self) -> &Path {
        self.path.as_ref()
    }
}

#[derive(Debug, Deserialize)]
struct LatestRelease {
    tag_name: String,
}

pub fn current_version() -> Result<Version> {
    Version::parse(env!("CARGO_PKG_VERSION")).context("当前程序版本不是有效的语义化版本")
}

pub fn parse_version(value: &str) -> Result<Version> {
    normalized_tag(value).map(|(_, version)| version)
}

pub fn executable_version(executable: &Path) -> Result<Version> {
    let output = Command::new(executable)
        .arg("--version")
        .output()
        .with_context(|| format!("无法运行 {} --version", executable.display()))?;
    if !output.status.success() {
        bail!(
            "{} --version 执行失败（{}）",
            executable.display(),
            output.status
        );
    }
    let reported =
        String::from_utf8(output.stdout).context("已安装程序返回的版本信息不是有效 UTF-8")?;
    let version = reported
        .trim()
        .strip_prefix("codex-notify ")
        .with_context(|| format!("{} 返回了无法识别的版本信息", executable.display()))?;
    Version::parse(version)
        .with_context(|| format!("{} 返回了无效的语义化版本“{version}”", executable.display()))
}

pub fn resolve_release(
    repository: &str,
    requested_version: Option<&str>,
    download_base_override: Option<&str>,
    network: &NetworkOptions,
) -> Result<ReleaseInfo> {
    validate_repository(repository)?;
    let client = http_client(network)?;
    let tag = match requested_version {
        Some(version) => normalized_tag(version)?.0,
        None => latest_release_tag(&client, repository)?,
    };
    let (tag, version) = normalized_tag(&tag)?;
    let download_base = download_base_override
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.trim_end_matches('/').to_owned())
        .unwrap_or_else(|| format!("https://github.com/{repository}/releases/download/{tag}"));
    Ok(ReleaseInfo {
        tag,
        version,
        asset_name: current_asset_name()?,
        download_base,
    })
}

pub fn update_needed(current: &Version, target: &Version, force: bool) -> Result<bool> {
    if target < current && !force {
        bail!("目标版本 v{target} 早于当前版本 v{current}；如需降级，请明确添加 --force");
    }
    Ok(force || target != current)
}

pub fn prepare_release(
    info: ReleaseInfo,
    staging_parent: &Path,
    network: &NetworkOptions,
) -> Result<PreparedRelease> {
    let client = http_client(network)?;
    let archive_url = format!("{}/{}", info.download_base, info.asset_name);
    let checksum_url = format!("{}/SHA256SUMS", info.download_base);
    let archive = download_limited(&client, &archive_url, MAX_ARCHIVE_BYTES)
        .with_context(|| format!("无法下载 {}", info.asset_name))?;
    let checksums = download_limited(&client, &checksum_url, MAX_CHECKSUM_BYTES)
        .context("无法下载 SHA256SUMS")?;
    let checksums = std::str::from_utf8(&checksums).context("SHA256SUMS 不是有效的 UTF-8 文件")?;
    verify_checksum(&archive, checksums, &info.asset_name)?;

    let directory = tempfile::Builder::new()
        .prefix("codex-notify-update-")
        .tempdir_in(staging_parent)
        .with_context(|| format!("无法在 {} 中创建升级临时目录", staging_parent.display()))?;
    let archive_path = directory.path().join(&info.asset_name);
    write_file(&archive_path, &archive)?;
    let executable_name = release_executable_name();
    let executable = directory.path().join(executable_name);
    extract_release_executable(&archive_path, &executable, executable_name)?;
    set_executable_permissions(&executable)?;
    verify_executable_version(&executable, &info.version)?;

    Ok(PreparedRelease {
        info,
        executable,
        _directory: directory,
    })
}

impl PreparedRelease {
    pub fn backup_current_executable(&self, current_executable: &Path) -> Result<ExecutableBackup> {
        let parent = current_executable
            .parent()
            .context("无法确定已安装程序所在目录")?;
        let backup = tempfile::Builder::new()
            .prefix(".codex-notify-backup-")
            .suffix(if cfg!(windows) { ".exe" } else { "" })
            .tempfile_in(parent)
            .with_context(|| format!("无法在 {} 中创建程序备份", parent.display()))?;
        fs::copy(current_executable, backup.path())
            .with_context(|| format!("无法备份已安装程序 {}", current_executable.display()))?;
        backup
            .as_file()
            .sync_all()
            .with_context(|| format!("无法保存 {}", backup.path().display()))?;
        Ok(ExecutableBackup {
            path: backup.into_temp_path(),
        })
    }
}

pub fn replace_current_executable(new_executable: &Path) -> Result<()> {
    self_replace::self_replace(new_executable)
        .with_context(|| format!("无法使用 {} 替换当前程序", new_executable.display()))
}

/// Atomically install an executable that is not the currently running process.
///
/// The caller is responsible for keeping a backup when replacing an existing
/// executable. On Windows the destination must not be running.
pub fn install_executable(source: &Path, destination: &Path) -> Result<()> {
    let parent = destination.parent().context("无法确定程序安装目录")?;
    fs::create_dir_all(parent).with_context(|| format!("无法创建目录 {}", parent.display()))?;
    let temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("无法在 {} 中创建升级文件", parent.display()))?;
    fs::copy(source, temporary.path()).with_context(|| {
        format!(
            "无法暂存程序 {} 以安装到 {}",
            source.display(),
            destination.display()
        )
    })?;
    set_executable_permissions(temporary.path())?;
    temporary
        .as_file()
        .sync_all()
        .with_context(|| format!("无法保存 {}", temporary.path().display()))?;

    #[cfg(windows)]
    if destination.exists() {
        remove_executable(destination)?;
    }

    temporary
        .persist(destination)
        .map_err(|error| error.error)
        .with_context(|| format!("无法安装 {}", destination.display()))?;
    Ok(())
}

#[cfg(windows)]
pub fn remove_executable(destination: &Path) -> Result<()> {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        match fs::remove_file(destination) {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error)
                if (error.kind() == std::io::ErrorKind::PermissionDenied
                    || matches!(error.raw_os_error(), Some(5 | 32 | 33)))
                    && std::time::Instant::now() < deadline =>
            {
                std::thread::sleep(Duration::from_millis(100));
            }
            Err(error) => {
                return Err(error).with_context(|| format!("无法替换 {}", destination.display()));
            }
        }
    }
}

#[cfg(not(windows))]
pub fn remove_executable(destination: &Path) -> Result<()> {
    match fs::remove_file(destination) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("无法删除 {}", destination.display())),
    }
}

fn validate_repository(repository: &str) -> Result<()> {
    let Some((owner, name)) = repository.split_once('/') else {
        bail!("仓库地址必须使用 owner/name 格式");
    };
    if owner.is_empty()
        || name.is_empty()
        || name.contains('/')
        || !owner.chars().all(repository_character)
        || !name.chars().all(repository_character)
    {
        bail!("仓库地址必须使用 owner/name 格式");
    }
    Ok(())
}

fn repository_character(character: char) -> bool {
    character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
}

fn normalized_tag(value: &str) -> Result<(String, Version)> {
    let value = value.trim();
    let version_text = value.strip_prefix('v').unwrap_or(value);
    let version =
        Version::parse(version_text).with_context(|| format!("“{value}”不是有效的语义化版本"))?;
    Ok((format!("v{version}"), version))
}

fn latest_release_tag(client: &NetworkClient, repository: &str) -> Result<String> {
    let url = format!("https://api.github.com/repos/{repository}/releases/latest");
    let release = client
        .get(&url)
        .context("无法查询最新 GitHub Release")?
        .error_for_status()
        .context("GitHub 没有返回最新发行版")?
        .json::<LatestRelease>()
        .context("无法解析最新 GitHub Release")?;
    Ok(release.tag_name)
}

fn http_client(network: &NetworkOptions) -> Result<NetworkClient> {
    if network.proxy.is_some() && network.no_proxy {
        bail!("--proxy 与 --no-proxy 不能同时使用");
    }

    let mut builder = Client::builder()
        .user_agent(format!("codex-notify/{}", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .timeout(Duration::from_secs(120));
    let route;

    if network.no_proxy {
        builder = builder.no_proxy();
        route = NetworkRoute::ProxyDisabled;
    } else if let Some(address) = network.proxy.as_deref() {
        builder = builder.proxy(proxy_from_address(address, None)?);
        route = NetworkRoute::ExplicitProxy;
    } else if proxy_environment_configured() {
        route = NetworkRoute::EnvironmentProxy;
    } else {
        #[cfg(windows)]
        {
            if let Some(settings) = windows_system_proxy() {
                builder = builder.proxy(proxy_from_address(
                    &settings.address,
                    settings.bypass.as_deref(),
                )?);
                route = NetworkRoute::WindowsSystemProxy;
            } else {
                route = NetworkRoute::Direct;
            }
        }
        #[cfg(not(windows))]
        {
            route = NetworkRoute::Direct;
        }
    }

    let client = builder.build().context("无法创建升级网络客户端")?;
    Ok(NetworkClient { client, route })
}

impl NetworkClient {
    fn get(&self, url: &str) -> Result<Response> {
        self.client
            .get(url)
            .send()
            .map_err(|error| friendly_request_error(error, self.route))
    }
}

fn proxy_environment_configured() -> bool {
    ["HTTPS_PROXY", "https_proxy", "ALL_PROXY", "all_proxy"]
        .into_iter()
        .any(|name| env::var_os(name).is_some_and(|value| !value.is_empty()))
}

fn proxy_from_address(address: &str, bypass: Option<&str>) -> Result<Proxy> {
    let address = normalized_proxy_address(address)
        .context("代理地址不能为空；示例：--proxy http://127.0.0.1:7890")?;
    let parsed =
        Url::parse(&address).context("代理地址无效；示例：--proxy http://127.0.0.1:7890")?;
    if !matches!(parsed.scheme(), "http" | "https") {
        bail!("升级代理仅支持 http:// 或 https:// 地址");
    }
    let mut proxy =
        Proxy::all(parsed).context("代理地址无效；示例：--proxy http://127.0.0.1:7890")?;
    if let Some(no_proxy) = combined_no_proxy(bypass) {
        proxy = proxy.no_proxy(Some(no_proxy));
    }
    Ok(proxy)
}

fn normalized_proxy_address(address: &str) -> Option<String> {
    let address = address.trim();
    if address.is_empty() {
        return None;
    }
    Some(if address.contains("://") {
        address.to_owned()
    } else {
        format!("http://{address}")
    })
}

fn normalized_windows_bypass(bypass: Option<&str>) -> Option<String> {
    let entries = bypass?
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty() && !entry.eq_ignore_ascii_case("<local>"))
        .map(|entry| entry.strip_prefix("*.").unwrap_or(entry))
        .collect::<Vec<_>>();
    (!entries.is_empty()).then(|| entries.join(","))
}

fn combined_no_proxy(windows_bypass: Option<&str>) -> Option<NoProxy> {
    let environment = env::var("NO_PROXY")
        .or_else(|_| env::var("no_proxy"))
        .ok()
        .filter(|value| !value.trim().is_empty());
    let windows = normalized_windows_bypass(windows_bypass);
    let combined = [environment, windows]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(",");
    NoProxy::from_string(&combined)
}

#[cfg(any(windows, test))]
fn windows_https_proxy(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    if !value.contains('=') {
        return Some(value.to_owned());
    }
    value.split(';').find_map(|entry| {
        let (scheme, address) = entry.split_once('=')?;
        scheme
            .trim()
            .eq_ignore_ascii_case("https")
            .then(|| address.trim().to_owned())
            .filter(|address| !address.is_empty())
    })
}

#[cfg(windows)]
fn windows_system_proxy() -> Option<SystemProxySettings> {
    let settings = RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey_with_flags(WINDOWS_INTERNET_SETTINGS_KEY, KEY_READ)
        .ok()?;
    let enabled = settings.get_value::<u32, _>("ProxyEnable").ok()?;
    if enabled == 0 {
        return None;
    }
    let configured = settings.get_value::<String, _>("ProxyServer").ok()?;
    let address = windows_https_proxy(&configured)?;
    let bypass = settings
        .get_value::<String, _>("ProxyOverride")
        .ok()
        .filter(|value| !value.trim().is_empty());
    Some(SystemProxySettings { address, bypass })
}

fn friendly_request_error(error: reqwest::Error, route: NetworkRoute) -> anyhow::Error {
    let message = request_failure_message(route, error.is_timeout());
    anyhow!("{message}（{error}）")
}

fn request_failure_message(route: NetworkRoute, timed_out: bool) -> &'static str {
    match (route, timed_out) {
        (NetworkRoute::ExplicitProxy, true)
        | (NetworkRoute::EnvironmentProxy, true)
        | (NetworkRoute::WindowsSystemProxy, true) => {
            "通过代理连接 GitHub 超时；请确认代理软件正在运行、代理地址和端口正确，然后重试"
        }
        (NetworkRoute::ExplicitProxy, false)
        | (NetworkRoute::EnvironmentProxy, false)
        | (NetworkRoute::WindowsSystemProxy, false) => {
            "无法通过代理连接 GitHub；请确认代理软件正在运行、代理地址和端口正确，然后重试"
        }
        (NetworkRoute::ProxyDisabled, true) => {
            "直连 GitHub 超时；当前已使用 --no-proxy 禁用代理，请检查网络后重试"
        }
        (NetworkRoute::ProxyDisabled, false) => {
            "无法直连 GitHub；当前已使用 --no-proxy 禁用代理，请检查网络后重试"
        }
        (NetworkRoute::Direct, true) => {
            "连接 GitHub 超时；如果正在使用代理软件，请开启系统代理或 TUN 模式，也可以使用 --proxy <URL> 重试"
        }
        (NetworkRoute::Direct, false) => {
            "无法连接 GitHub；如果正在使用代理软件，请开启系统代理或 TUN 模式，也可以使用 --proxy <URL> 重试"
        }
    }
}

fn download_limited(client: &NetworkClient, url: &str, maximum: u64) -> Result<Vec<u8>> {
    let response = client
        .get(url)?
        .error_for_status()
        .with_context(|| format!("下载失败：{url}"))?;
    read_limited(response, maximum)
}

fn read_limited(response: Response, maximum: u64) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > maximum)
    {
        bail!("下载内容超过允许大小");
    }
    let mut bytes = Vec::new();
    response
        .take(maximum.saturating_add(1))
        .read_to_end(&mut bytes)
        .context("无法读取下载内容")?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > maximum {
        bail!("下载内容超过允许大小");
    }
    Ok(bytes)
}

fn verify_checksum(archive: &[u8], checksums: &str, asset_name: &str) -> Result<()> {
    let expected = checksum_for_asset(checksums, asset_name)?;
    let actual = format!("{:x}", Sha256::digest(archive));
    if actual != expected {
        bail!("{asset_name} 的 SHA-256 校验失败，为保护安装安全，已停止升级");
    }
    Ok(())
}

fn checksum_for_asset(checksums: &str, asset_name: &str) -> Result<String> {
    let mut found = None;
    for line in checksums.lines() {
        let mut fields = line.split_whitespace();
        let Some(hash) = fields.next() else {
            continue;
        };
        let Some(name) = fields.next() else {
            continue;
        };
        if fields.next().is_some() || name.trim_start_matches('*') != asset_name {
            continue;
        }
        let normalized = hash.to_ascii_lowercase();
        if normalized.len() != 64
            || !normalized
                .chars()
                .all(|character| character.is_ascii_hexdigit())
        {
            bail!("SHA256SUMS 中 {asset_name} 的校验值无效");
        }
        if found.replace(normalized).is_some() {
            bail!("SHA256SUMS 中包含多个 {asset_name} 校验值");
        }
    }
    found.with_context(|| format!("SHA256SUMS 中没有 {asset_name} 的校验值"))
}

fn write_file(path: &Path, contents: &[u8]) -> Result<()> {
    let mut file =
        File::create(path).with_context(|| format!("无法创建升级文件 {}", path.display()))?;
    file.write_all(contents)
        .with_context(|| format!("无法写入升级文件 {}", path.display()))?;
    file.sync_all()
        .with_context(|| format!("无法保存升级文件 {}", path.display()))
}

fn extract_release_executable(
    archive_path: &Path,
    destination: &Path,
    executable_name: &str,
) -> Result<()> {
    if archive_path.extension().and_then(|value| value.to_str()) == Some("zip") {
        extract_zip_executable(archive_path, destination, executable_name)
    } else {
        extract_tar_executable(archive_path, destination, executable_name)
    }
}

fn extract_tar_executable(
    archive_path: &Path,
    destination: &Path,
    executable_name: &str,
) -> Result<()> {
    let file =
        File::open(archive_path).with_context(|| format!("无法打开 {}", archive_path.display()))?;
    let mut archive = tar::Archive::new(GzDecoder::new(file));
    for entry in archive.entries().context("无法读取发行版压缩包")? {
        let mut entry = entry.context("无法读取发行版压缩包中的文件")?;
        if !entry.header().entry_type().is_file()
            || entry
                .path()
                .context("发行版压缩包中的文件路径无效")?
                .as_ref()
                != Path::new(executable_name)
        {
            continue;
        }
        let mut output = File::create(destination)
            .with_context(|| format!("无法创建 {}", destination.display()))?;
        std::io::copy(&mut entry, &mut output)
            .with_context(|| format!("无法解压 {executable_name}"))?;
        output
            .sync_all()
            .with_context(|| format!("无法保存 {}", destination.display()))?;
        return Ok(());
    }
    bail!("发行版压缩包中没有 {executable_name}")
}

fn extract_zip_executable(
    archive_path: &Path,
    destination: &Path,
    executable_name: &str,
) -> Result<()> {
    let file =
        File::open(archive_path).with_context(|| format!("无法打开 {}", archive_path.display()))?;
    let mut archive = zip::ZipArchive::new(file).context("无法读取发行版 ZIP")?;
    let mut entry = archive
        .by_name(executable_name)
        .with_context(|| format!("发行版 ZIP 中没有 {executable_name}"))?;
    if !entry.is_file() {
        bail!("发行版 ZIP 中的 {executable_name} 不是文件");
    }
    let mut output =
        File::create(destination).with_context(|| format!("无法创建 {}", destination.display()))?;
    std::io::copy(&mut entry, &mut output)
        .with_context(|| format!("无法解压 {executable_name}"))?;
    output
        .sync_all()
        .with_context(|| format!("无法保存 {}", destination.display()))
}

#[cfg(unix)]
fn set_executable_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut permissions = fs::metadata(path)
        .with_context(|| format!("无法检查 {}", path.display()))?
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(path, permissions)
        .with_context(|| format!("无法为 {} 添加可执行权限", path.display()))
}

#[cfg(not(unix))]
fn set_executable_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn verify_executable_version(executable: &Path, expected: &Version) -> Result<()> {
    let reported = executable_version(executable)
        .with_context(|| format!("无法验证暂存程序 {}", executable.display()))?;
    if &reported != expected {
        bail!("暂存程序报告版本 v{reported}，预期应为 v{expected}");
    }
    Ok(())
}

fn current_asset_name() -> Result<String> {
    asset_name_for(std::env::consts::OS, std::env::consts::ARCH)
        .map(ToOwned::to_owned)
        .context("当前平台暂不支持 codex-notify 自动升级")
}

fn asset_name_for(os: &str, architecture: &str) -> Option<&'static str> {
    match (os, architecture) {
        ("macos", "aarch64") => Some("codex-notify-aarch64-apple-darwin.tar.gz"),
        ("macos", "x86_64") => Some("codex-notify-x86_64-apple-darwin.tar.gz"),
        ("linux", "aarch64") => Some("codex-notify-aarch64-unknown-linux-gnu.tar.gz"),
        ("linux", "x86_64") => Some("codex-notify-x86_64-unknown-linux-gnu.tar.gz"),
        ("windows", "x86_64") => Some("codex-notify-x86_64-pc-windows-msvc.zip"),
        _ => None,
    }
}

fn release_executable_name() -> &'static str {
    if cfg!(windows) {
        "codex-notify.exe"
    } else {
        "codex-notify"
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NetworkRoute, asset_name_for, checksum_for_asset, install_executable,
        normalized_proxy_address, normalized_tag, normalized_windows_bypass,
        request_failure_message, update_needed, windows_https_proxy,
    };
    use semver::Version;

    #[test]
    fn release_targets_cover_every_supported_platform() {
        assert_eq!(
            asset_name_for("macos", "aarch64"),
            Some("codex-notify-aarch64-apple-darwin.tar.gz")
        );
        assert_eq!(
            asset_name_for("macos", "x86_64"),
            Some("codex-notify-x86_64-apple-darwin.tar.gz")
        );
        assert_eq!(
            asset_name_for("linux", "aarch64"),
            Some("codex-notify-aarch64-unknown-linux-gnu.tar.gz")
        );
        assert_eq!(
            asset_name_for("linux", "x86_64"),
            Some("codex-notify-x86_64-unknown-linux-gnu.tar.gz")
        );
        assert_eq!(
            asset_name_for("windows", "x86_64"),
            Some("codex-notify-x86_64-pc-windows-msvc.zip")
        );
        assert_eq!(asset_name_for("freebsd", "x86_64"), None);
    }

    #[test]
    fn release_versions_accept_tags_and_plain_semver() {
        assert_eq!(normalized_tag("v1.2.3").expect("tag").0, "v1.2.3");
        assert_eq!(normalized_tag("1.2.3").expect("version").0, "v1.2.3");
        assert!(normalized_tag("latest").is_err());
    }

    #[test]
    fn update_decision_prevents_accidental_downgrades() {
        let current = Version::new(2, 0, 0);
        assert!(!update_needed(&current, &current, false).expect("same"));
        assert!(update_needed(&current, &Version::new(2, 1, 0), false).expect("upgrade"));
        assert!(update_needed(&current, &Version::new(1, 9, 0), false).is_err());
        assert!(update_needed(&current, &Version::new(1, 9, 0), true).expect("forced"));
    }

    #[test]
    fn windows_proxy_parser_selects_the_https_route() {
        assert_eq!(
            windows_https_proxy("10.211.55.2:7897").as_deref(),
            Some("10.211.55.2:7897")
        );
        assert_eq!(
            windows_https_proxy("http=127.0.0.1:8080;https=127.0.0.1:8443").as_deref(),
            Some("127.0.0.1:8443")
        );
        assert_eq!(windows_https_proxy("http=127.0.0.1:8080"), None);
        assert_eq!(windows_https_proxy("   "), None);
    }

    #[test]
    fn proxy_addresses_and_windows_bypass_entries_are_normalized() {
        assert_eq!(
            normalized_proxy_address("127.0.0.1:7890").as_deref(),
            Some("http://127.0.0.1:7890")
        );
        assert_eq!(
            normalized_proxy_address("https://proxy.example:443").as_deref(),
            Some("https://proxy.example:443")
        );
        assert_eq!(normalized_proxy_address("  "), None);
        assert_eq!(
            normalized_windows_bypass(Some("<local>;localhost;*.example.com")).as_deref(),
            Some("localhost,example.com")
        );
    }

    #[test]
    fn network_failures_explain_the_relevant_recovery() {
        let direct = request_failure_message(NetworkRoute::Direct, true);
        assert!(direct.contains("连接 GitHub 超时"));
        assert!(direct.contains("代理软件"));
        assert!(direct.contains("--proxy <URL>"));

        let proxied = request_failure_message(NetworkRoute::WindowsSystemProxy, true);
        assert!(proxied.contains("通过代理连接 GitHub 超时"));
        assert!(proxied.contains("代理地址和端口"));

        let disabled = request_failure_message(NetworkRoute::ProxyDisabled, true);
        assert!(disabled.contains("--no-proxy"));
    }

    #[test]
    fn checksum_parser_requires_one_exact_asset_match() {
        let hash = "a".repeat(64);
        let checksums = format!("{hash}  first.tar.gz\n{hash} *second.zip\n");
        assert_eq!(
            checksum_for_asset(&checksums, "second.zip").expect("checksum"),
            hash
        );
        assert!(checksum_for_asset(&checksums, "missing.zip").is_err());
        let duplicate = format!("{hash}  same.zip\n{hash}  same.zip\n");
        assert!(checksum_for_asset(&duplicate, "same.zip").is_err());
    }

    #[test]
    fn executable_install_replaces_the_destination_without_partial_contents() {
        let directory = tempfile::tempdir().expect("temporary directory");
        let source = directory.path().join("source");
        let destination = directory.path().join("destination");
        std::fs::write(&source, b"new executable").expect("write source");
        std::fs::write(&destination, b"old executable").expect("write destination");

        install_executable(&source, &destination).expect("install executable");

        assert_eq!(
            std::fs::read(&destination).expect("read destination"),
            b"new executable"
        );
    }
}
