//! Per-user background service integration for the transcript watcher.

use anyhow::{Context, Result, bail};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use directories::UserDirs;
#[cfg(target_os = "windows")]
use fs2::FileExt;
#[cfg(any(target_os = "macos", target_os = "linux", test))]
use std::fs;
#[cfg(target_os = "windows")]
use std::fs::OpenOptions;
#[cfg(target_os = "windows")]
use std::io::ErrorKind;
use std::path::Path;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use std::path::PathBuf;
#[cfg(any(target_os = "macos", target_os = "linux", target_os = "windows"))]
use std::process::Command;
#[cfg(target_os = "windows")]
use std::process::Stdio;
#[cfg(target_os = "windows")]
use winreg::RegKey;
#[cfg(target_os = "windows")]
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

use crate::paths::AppPaths;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::settings::atomic_write;

#[cfg(any(target_os = "macos", test))]
const MACOS_LABEL: &str = "com.codex-notify.watcher";
#[cfg(target_os = "windows")]
const WINDOWS_RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(target_os = "windows")]
const WINDOWS_RUN_VALUE: &str = "CodexNotifyWatcher";
#[cfg(target_os = "linux")]
const LINUX_UNIT_NAME: &str = "codex-notify-watcher.service";
#[cfg(any(target_os = "linux", test))]
const LINUX_UNIT_MARKER: &str = "# Managed by codex-notify.";

pub fn install_watcher(paths: &AppPaths, binary: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        install_macos_watcher(paths, binary)
    }
    #[cfg(target_os = "windows")]
    {
        install_windows_watcher(paths, binary)
    }
    #[cfg(target_os = "linux")]
    {
        install_linux_watcher(paths, binary)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = (paths, binary);
        bail!("后台监听仅支持 macOS、Windows 和 Linux")
    }
}

pub fn uninstall_watcher(_paths: &AppPaths) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        uninstall_macos_watcher()
    }
    #[cfg(target_os = "windows")]
    {
        uninstall_windows_watcher()
    }
    #[cfg(target_os = "linux")]
    {
        uninstall_linux_watcher()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = _paths;
        bail!("后台监听仅支持 macOS、Windows 和 Linux")
    }
}

/// Stop the managed watcher without removing its login/startup configuration.
///
/// Updates use this before replacing the executable, then call
/// [`install_watcher`] after the new binary has refreshed the integration.
pub fn stop_watcher(_paths: &AppPaths) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        stop_macos_watcher()
    }
    #[cfg(target_os = "windows")]
    {
        // Windows watcher shutdown is coordinated by the stop marker and
        // process lock in the CLI. The Run entry must remain installed.
        Ok(())
    }
    #[cfg(target_os = "linux")]
    {
        stop_linux_watcher()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = _paths;
        bail!("后台监听仅支持 macOS、Windows 和 Linux")
    }
}

pub fn is_watcher_installed(_paths: &AppPaths) -> Result<bool> {
    #[cfg(target_os = "macos")]
    {
        let path = macos_plist_path()?;
        Ok(path.is_file() && is_managed_macos_plist(&path))
    }
    #[cfg(target_os = "windows")]
    {
        Ok(read_windows_run_command()?
            .as_deref()
            .is_some_and(is_managed_windows_run_command))
    }
    #[cfg(target_os = "linux")]
    {
        let path = linux_unit_path()?;
        Ok(path.is_file() && is_managed_linux_unit(&path))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        Ok(false)
    }
}

pub fn watcher_location() -> Result<String> {
    #[cfg(target_os = "macos")]
    {
        Ok(macos_plist_path()?.display().to_string())
    }
    #[cfg(target_os = "windows")]
    {
        Ok(format!(
            "注册表：HKCU\\{WINDOWS_RUN_KEY}\\{WINDOWS_RUN_VALUE}"
        ))
    }
    #[cfg(target_os = "linux")]
    {
        Ok(linux_unit_path()?.display().to_string())
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        bail!("后台监听仅支持 macOS、Windows 和 Linux")
    }
}

#[cfg(target_os = "macos")]
fn install_macos_watcher(paths: &AppPaths, binary: &Path) -> Result<()> {
    let plist_path = macos_plist_path()?;
    let previous = fs::read(&plist_path).ok();
    if previous.is_some() && !is_managed_macos_plist(&plist_path) {
        bail!(
            "{} 是用户自行管理的 LaunchAgent，为保护现有配置，codex-notify 不会覆盖它",
            plist_path.display()
        );
    }
    let plist = macos_plist(binary, &paths.root, &paths.codex_home);
    atomic_write(&plist_path, plist.as_bytes())?;

    let uid = current_uid()?;
    let domain = format!("gui/{uid}");
    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &domain, &plist_path.display().to_string()])
        .status();
    let status = Command::new("/bin/launchctl")
        .args(["bootstrap", &domain, &plist_path.display().to_string()])
        .status()
        .context("无法运行 launchctl bootstrap")?;
    if status.success() {
        return Ok(());
    }

    match previous {
        Some(contents) => atomic_write(&plist_path, &contents)?,
        None => {
            let _ = fs::remove_file(&plist_path);
        }
    }
    bail!("launchctl 未能启动 codex-notify 后台监听（{status}）")
}

#[cfg(target_os = "macos")]
fn uninstall_macos_watcher() -> Result<()> {
    let plist_path = macos_plist_path()?;
    if !plist_path.exists() {
        return Ok(());
    }
    if !is_managed_macos_plist(&plist_path) {
        bail!(
            "{} 是用户自行管理的 LaunchAgent，为保护现有配置，codex-notify 不会删除它",
            plist_path.display()
        );
    }
    let uid = current_uid()?;
    let domain = format!("gui/{uid}");
    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &domain, &plist_path.display().to_string()])
        .status();
    fs::remove_file(&plist_path).with_context(|| format!("无法删除 {}", plist_path.display()))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn stop_macos_watcher() -> Result<()> {
    let plist_path = macos_plist_path()?;
    if !plist_path.exists() {
        return Ok(());
    }
    if !is_managed_macos_plist(&plist_path) {
        bail!(
            "{} 是用户自行管理的 LaunchAgent，codex-notify 不会停止它",
            plist_path.display()
        );
    }
    let uid = current_uid()?;
    let domain = format!("gui/{uid}");
    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &domain, &plist_path.display().to_string()])
        .status()
        .context("无法运行 launchctl bootout")?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_plist_path() -> Result<PathBuf> {
    let home = UserDirs::new()
        .map(|directories| directories.home_dir().to_path_buf())
        .context("无法确定当前用户主目录")?;
    Ok(home
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{MACOS_LABEL}.plist")))
}

#[cfg(target_os = "macos")]
fn current_uid() -> Result<String> {
    let output = Command::new("/usr/bin/id")
        .arg("-u")
        .output()
        .context("无法确定当前用户 ID")?;
    if !output.status.success() {
        bail!("无法确定当前用户 ID（{}）", output.status);
    }
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if uid.is_empty() {
        bail!("无法确定当前用户 ID");
    }
    Ok(uid)
}

#[cfg(target_os = "macos")]
fn is_managed_macos_plist(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|contents| contents.contains(&format!("<string>{MACOS_LABEL}</string>")))
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn install_linux_watcher(paths: &AppPaths, binary: &Path) -> Result<()> {
    let unit_path = linux_unit_path()?;
    let previous = match fs::read(&unit_path) {
        Ok(contents) => Some(contents),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("无法读取 {}", unit_path.display()));
        }
    };
    if previous.is_some() && !is_managed_linux_unit(&unit_path) {
        bail!(
            "{} 是用户自行管理的 systemd 服务，为保护现有配置，codex-notify 不会覆盖它",
            unit_path.display()
        );
    }

    let unit = linux_unit(binary, &paths.root, &paths.codex_home)?;
    atomic_write(&unit_path, unit.as_bytes())?;
    let install_result = (|| {
        run_user_systemctl(&["daemon-reload"])?;
        run_user_systemctl(&["enable", LINUX_UNIT_NAME])?;
        run_user_systemctl(&["restart", LINUX_UNIT_NAME])?;
        std::thread::sleep(std::time::Duration::from_millis(250));
        ensure_linux_watcher_active()
    })();
    if let Err(install_error) = install_result {
        let _ = run_user_systemctl(&["disable", "--now", LINUX_UNIT_NAME]);
        let rollback_result = (|| {
            match previous.as_deref() {
                Some(contents) => atomic_write(&unit_path, contents)?,
                None => match fs::remove_file(&unit_path) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                    Err(error) => {
                        return Err(error)
                            .with_context(|| format!("无法删除 {}", unit_path.display()));
                    }
                },
            }
            run_user_systemctl(&["daemon-reload"])?;
            if previous.is_some() {
                run_user_systemctl(&["enable", LINUX_UNIT_NAME])?;
                run_user_systemctl(&["restart", LINUX_UNIT_NAME])?;
                std::thread::sleep(std::time::Duration::from_millis(250));
                ensure_linux_watcher_active()?;
            }
            Ok(())
        })();
        if let Err(rollback_error) = rollback_result {
            bail!("{install_error:#}；恢复原有 codex-notify 后台监听也失败了：{rollback_error:#}");
        }
        return Err(install_error);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn uninstall_linux_watcher() -> Result<()> {
    let unit_path = linux_unit_path()?;
    if !unit_path.exists() {
        return Ok(());
    }
    if !is_managed_linux_unit(&unit_path) {
        bail!(
            "{} 是用户自行管理的 systemd 服务，为保护现有配置，codex-notify 不会删除它",
            unit_path.display()
        );
    }

    run_user_systemctl(&["disable", "--now", LINUX_UNIT_NAME])?;
    fs::remove_file(&unit_path).with_context(|| format!("无法删除 {}", unit_path.display()))?;
    run_user_systemctl(&["daemon-reload"])?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn stop_linux_watcher() -> Result<()> {
    let unit_path = linux_unit_path()?;
    if !unit_path.exists() {
        return Ok(());
    }
    if !is_managed_linux_unit(&unit_path) {
        bail!(
            "{} 是用户自行管理的 systemd 服务，codex-notify 不会停止它",
            unit_path.display()
        );
    }
    run_user_systemctl(&["stop", LINUX_UNIT_NAME])
}

#[cfg(target_os = "linux")]
fn run_user_systemctl(arguments: &[&str]) -> Result<()> {
    let status = Command::new("systemctl")
        .arg("--user")
        .args(arguments)
        .status()
        .context("无法运行 systemctl --user")?;
    if status.success() {
        return Ok(());
    }
    bail!(
        "systemctl --user {} 执行失败（{status}）",
        arguments.join(" ")
    )
}

#[cfg(target_os = "linux")]
fn ensure_linux_watcher_active() -> Result<()> {
    let status = Command::new("systemctl")
        .args(["--user", "is-active", "--quiet", LINUX_UNIT_NAME])
        .status()
        .context("无法检查 codex-notify systemd 用户服务")?;
    if status.success() {
        return Ok(());
    }
    bail!(
        "codex-notify systemd 用户服务未保持运行，请执行 systemctl --user status {LINUX_UNIT_NAME} 查看详情"
    )
}

#[cfg(target_os = "linux")]
fn linux_unit_path() -> Result<PathBuf> {
    let home = UserDirs::new()
        .map(|directories| directories.home_dir().to_path_buf())
        .context("无法确定当前用户主目录")?;
    Ok(home
        .join(".config")
        .join("systemd")
        .join("user")
        .join(LINUX_UNIT_NAME))
}

#[cfg(any(target_os = "linux", test))]
fn is_managed_linux_unit(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|contents| contents.starts_with(LINUX_UNIT_MARKER))
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn install_windows_watcher(paths: &AppPaths, binary: &Path) -> Result<()> {
    if let Some(existing) = read_windows_run_command()?
        && !is_managed_windows_run_command(&existing)
    {
        bail!(
            "Windows 启动项 {WINDOWS_RUN_VALUE} 由用户自行管理，为保护现有配置，codex-notify 不会覆盖它"
        );
    }
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let (run_key, _) = current_user
        .create_subkey(WINDOWS_RUN_KEY)
        .context("无法打开当前用户的 Windows 启动项注册表")?;
    run_key
        .set_value(
            WINDOWS_RUN_VALUE,
            &windows_run_command(binary, &paths.root, &paths.codex_home),
        )
        .context("无法安装 codex-notify Windows 启动项")?;

    start_windows_watcher(paths, binary)
}

#[cfg(target_os = "windows")]
fn start_windows_watcher(paths: &AppPaths, binary: &Path) -> Result<()> {
    use std::os::windows::process::CommandExt;

    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    if windows_watcher_running(paths)? {
        return Ok(());
    }
    let mut child = Command::new(binary)
        .arg("watch")
        .env("CODEX_NOTIFY_HOME", &paths.root)
        .env("CODEX_NOTIFY_CODEX_HOME", &paths.codex_home)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP)
        .spawn()
        .with_context(|| format!("无法启动后台监听 {}", binary.display()))?;
    std::thread::sleep(std::time::Duration::from_millis(250));
    if let Some(status) = child
        .try_wait()
        .context("无法检查 codex-notify Windows 后台监听")?
    {
        if windows_watcher_running(paths)? {
            return Ok(());
        }
        bail!("codex-notify Windows 后台监听启动后立即退出（{status}）");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn windows_watcher_running(paths: &AppPaths) -> Result<bool> {
    let path = paths.state.join("watcher-process.lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("无法打开 {}", path.display()))?;
    match file.try_lock_exclusive() {
        Ok(()) => {
            let _ = FileExt::unlock(&file);
            Ok(false)
        }
        Err(error) if error.kind() == ErrorKind::WouldBlock || error.raw_os_error() == Some(33) => {
            Ok(true)
        }
        Err(error) => Err(error).with_context(|| format!("无法锁定 {}", path.display())),
    }
}

#[cfg(target_os = "windows")]
fn uninstall_windows_watcher() -> Result<()> {
    let Some(existing) = read_windows_run_command()? else {
        return Ok(());
    };
    if !is_managed_windows_run_command(&existing) {
        bail!(
            "Windows 启动项 {WINDOWS_RUN_VALUE} 由用户自行管理，为保护现有配置，codex-notify 不会删除它"
        );
    }
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let run_key = current_user
        .open_subkey_with_flags(WINDOWS_RUN_KEY, KEY_READ | KEY_WRITE)
        .context("无法打开当前用户的 Windows 启动项注册表")?;
    run_key
        .delete_value(WINDOWS_RUN_VALUE)
        .context("无法删除 codex-notify Windows 启动项")
}

#[cfg(target_os = "windows")]
fn read_windows_run_command() -> Result<Option<String>> {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let run_key = match current_user.open_subkey(WINDOWS_RUN_KEY) {
        Ok(key) => key,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).context("无法打开当前用户的 Windows 启动项注册表");
        }
    };
    match run_key.get_value(WINDOWS_RUN_VALUE) {
        Ok(command) => Ok(Some(command)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("无法读取 codex-notify Windows 启动项"),
    }
}

#[cfg(any(target_os = "macos", test))]
fn macos_plist(binary: &Path, app_data: &Path, codex_home: &Path) -> String {
    let binary = xml_escape(&binary.display().to_string());
    let app_data = xml_escape(&app_data.display().to_string());
    let codex_home = xml_escape(&codex_home.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>{MACOS_LABEL}</string>
  <key>ProgramArguments</key>
  <array><string>{binary}</string><string>watch</string></array>
  <key>WorkingDirectory</key><string>{app_data}</string>
  <key>EnvironmentVariables</key>
  <dict>
    <key>CODEX_NOTIFY_HOME</key><string>{app_data}</string>
    <key>CODEX_NOTIFY_CODEX_HOME</key><string>{codex_home}</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>ProcessType</key><string>Background</string>
  <key>StandardOutPath</key><string>/dev/null</string>
  <key>StandardErrorPath</key><string>/dev/null</string>
</dict>
</plist>
"#
    )
}

#[cfg(any(target_os = "linux", test))]
fn linux_unit(binary: &Path, app_data: &Path, codex_home: &Path) -> Result<String> {
    let binary_value = binary.display().to_string();
    let app_data_value = app_data.display().to_string();
    let codex_home_value = codex_home.display().to_string();
    let binary = systemd_quote(&binary_value)?;
    let working_directory = systemd_path_value(&app_data_value)?;
    let app_environment = systemd_quote(&format!("CODEX_NOTIFY_HOME={app_data_value}"))?;
    let codex_environment = systemd_quote(&format!("CODEX_NOTIFY_CODEX_HOME={codex_home_value}"))?;
    Ok(format!(
        r#"{LINUX_UNIT_MARKER}
[Unit]
Description=codex-notify background watcher

[Service]
Type=simple
ExecStart={binary} watch
WorkingDirectory={working_directory}
Environment={app_environment}
Environment={codex_environment}
Restart=on-failure
RestartSec=5s

[Install]
WantedBy=default.target
"#
    ))
}

#[cfg(any(target_os = "linux", test))]
fn systemd_path_value(value: &str) -> Result<String> {
    if value
        .chars()
        .any(|character| matches!(character, '\n' | '\r' | '\0'))
    {
        bail!("systemd 服务配置值不能包含控制字符");
    }
    Ok(value
        .replace('\\', "\\\\")
        .replace(' ', "\\x20")
        .replace('"', "\\x22")
        .replace('%', "%%"))
}

#[cfg(any(target_os = "linux", test))]
fn systemd_quote(value: &str) -> Result<String> {
    if value
        .chars()
        .any(|character| matches!(character, '\n' | '\r' | '\0'))
    {
        bail!("systemd 服务配置值不能包含控制字符");
    }
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('%', "%%");
    Ok(format!("\"{escaped}\""))
}

#[cfg(any(target_os = "windows", test))]
fn windows_run_command(binary: &Path, app_data: &Path, codex_home: &Path) -> String {
    let binary = binary.to_string_lossy().replace('\'', "''");
    let app_data = app_data.to_string_lossy().replace('\'', "''");
    let codex_home = codex_home.to_string_lossy().replace('\'', "''");
    format!(
        "powershell.exe -NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -Command \"$env:CODEX_NOTIFY_HOME='{app_data}'; $env:CODEX_NOTIFY_CODEX_HOME='{codex_home}'; & '{binary}' watch\""
    )
}

#[cfg(any(target_os = "windows", test))]
fn is_managed_windows_run_command(command: &str) -> bool {
    const PREFIX: &str =
        "powershell.exe -NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -Command \"";
    let body = command
        .strip_prefix(PREFIX)
        .and_then(|value| value.strip_suffix("' watch\""));
    body.is_some_and(|value| {
        value.starts_with("& '")
            || (value.starts_with("$env:CODEX_NOTIFY_HOME='")
                && value.contains("'; $env:CODEX_NOTIFY_CODEX_HOME='")
                && value.contains("'; & '"))
    })
}

#[cfg(any(target_os = "macos", test))]
fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
mod tests {
    use super::{
        LINUX_UNIT_MARKER, is_managed_linux_unit, is_managed_windows_run_command, linux_unit,
        macos_plist, windows_run_command,
    };
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    #[test]
    fn macos_launch_agent_keeps_the_watcher_alive_with_explicit_paths() {
        let plist = macos_plist(
            Path::new("/Applications/codex-notify"),
            Path::new("/Users/example/Library/Application Support/codex-notify"),
            Path::new("/Users/example/.codex"),
        );
        assert!(plist.contains("<string>com.codex-notify.watcher</string>"));
        assert!(plist.contains("<key>KeepAlive</key><true/>"));
        assert!(plist.contains("<string>watch</string>"));
    }

    #[test]
    fn windows_startup_entry_runs_a_hidden_watcher_with_explicit_paths() {
        let command = windows_run_command(
            Path::new("C:\\Program Files\\codex-notify.exe"),
            Path::new("C:\\Users\\example\\AppData\\Roaming\\codex-notify"),
            Path::new("D:\\Managed Codex"),
        );
        assert_eq!(
            command,
            "powershell.exe -NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -Command \"$env:CODEX_NOTIFY_HOME='C:\\Users\\example\\AppData\\Roaming\\codex-notify'; $env:CODEX_NOTIFY_CODEX_HOME='D:\\Managed Codex'; & 'C:\\Program Files\\codex-notify.exe' watch\""
        );
        assert!(is_managed_windows_run_command(&command));
        assert!(is_managed_windows_run_command(
            "powershell.exe -NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -Command \"& 'C:\\old-codex-notify.exe' watch\""
        ));
        assert!(!is_managed_windows_run_command("other-notifier.exe"));
    }

    #[test]
    fn linux_user_service_keeps_the_watcher_alive_with_explicit_paths() {
        let unit = linux_unit(
            Path::new("/home/example/My Tools/codex-notify"),
            Path::new("/home/example/App Data/codex-notify"),
            Path::new("/home/example/Codex % profile"),
        )
        .expect("Linux user service");

        assert!(unit.starts_with(LINUX_UNIT_MARKER));
        assert!(unit.contains("ExecStart=\"/home/example/My Tools/codex-notify\" watch"));
        assert!(unit.contains("WorkingDirectory=/home/example/App\\x20Data/codex-notify"));
        assert!(
            unit.contains("Environment=\"CODEX_NOTIFY_HOME=/home/example/App Data/codex-notify\"")
        );
        assert!(
            unit.contains("Environment=\"CODEX_NOTIFY_CODEX_HOME=/home/example/Codex %% profile\"")
        );
        assert!(unit.contains("Restart=on-failure"));
        assert!(unit.contains("WantedBy=default.target"));
    }

    #[test]
    fn linux_user_service_ownership_requires_the_marker() {
        let directory = tempdir().expect("temporary directory");
        let path = directory.path().join("codex-notify-watcher.service");
        fs::write(&path, format!("{LINUX_UNIT_MARKER}\n[Service]\n")).expect("write managed unit");
        assert!(is_managed_linux_unit(&path));

        fs::write(&path, "[Service]\nExecStart=other-watcher\n").expect("write foreign unit");
        assert!(!is_managed_linux_unit(&path));
    }
}
