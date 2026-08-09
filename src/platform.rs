//! Per-user background service integration for the transcript watcher.

use anyhow::{Context, Result, bail};
#[cfg(target_os = "macos")]
use directories::UserDirs;
#[cfg(target_os = "macos")]
use std::fs;
#[cfg(target_os = "windows")]
use std::io::ErrorKind;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::path::PathBuf;
#[cfg(target_os = "macos")]
use std::process::Command;
#[cfg(target_os = "windows")]
use winreg::RegKey;
#[cfg(target_os = "windows")]
use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_WRITE};

use crate::paths::AppPaths;
#[cfg(target_os = "macos")]
use crate::settings::atomic_write;

#[cfg(any(target_os = "macos", test))]
const MACOS_LABEL: &str = "com.codex-notify.watcher";
#[cfg(target_os = "windows")]
const WINDOWS_RUN_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";
#[cfg(target_os = "windows")]
const WINDOWS_RUN_VALUE: &str = "CodexNotifyWatcher";

pub fn install_watcher(paths: &AppPaths, binary: &Path) -> Result<()> {
    #[cfg(target_os = "macos")]
    {
        install_macos_watcher(paths, binary)
    }
    #[cfg(target_os = "windows")]
    {
        install_windows_watcher(paths, binary)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = (paths, binary);
        bail!("the background watcher is supported on macOS and Windows only")
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
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        let _ = _paths;
        bail!("the background watcher is supported on macOS and Windows only")
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
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
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
            "Registry: HKCU\\{WINDOWS_RUN_KEY}\\{WINDOWS_RUN_VALUE}"
        ))
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        bail!("the background watcher is supported on macOS and Windows only")
    }
}

#[cfg(target_os = "macos")]
fn install_macos_watcher(paths: &AppPaths, binary: &Path) -> Result<()> {
    let plist_path = macos_plist_path()?;
    let previous = fs::read(&plist_path).ok();
    if previous.is_some() && !is_managed_macos_plist(&plist_path) {
        bail!(
            "refusing to overwrite user-managed LaunchAgent {}",
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
        .context("could not run launchctl bootstrap")?;
    if status.success() {
        return Ok(());
    }

    match previous {
        Some(contents) => atomic_write(&plist_path, &contents)?,
        None => {
            let _ = fs::remove_file(&plist_path);
        }
    }
    bail!("launchctl could not start the codex-notify watcher ({status})")
}

#[cfg(target_os = "macos")]
fn uninstall_macos_watcher() -> Result<()> {
    let plist_path = macos_plist_path()?;
    if !plist_path.exists() {
        return Ok(());
    }
    if !is_managed_macos_plist(&plist_path) {
        bail!(
            "refusing to remove user-managed LaunchAgent {}",
            plist_path.display()
        );
    }
    let uid = current_uid()?;
    let domain = format!("gui/{uid}");
    let _ = Command::new("/bin/launchctl")
        .args(["bootout", &domain, &plist_path.display().to_string()])
        .status();
    fs::remove_file(&plist_path)
        .with_context(|| format!("could not remove {}", plist_path.display()))?;
    Ok(())
}

#[cfg(target_os = "macos")]
fn macos_plist_path() -> Result<PathBuf> {
    let home = UserDirs::new()
        .map(|directories| directories.home_dir().to_path_buf())
        .context("could not determine the current user home directory")?;
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
        .context("could not determine the current user ID")?;
    if !output.status.success() {
        bail!(
            "could not determine the current user ID ({})",
            output.status
        );
    }
    let uid = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    if uid.is_empty() {
        bail!("could not determine the current user ID");
    }
    Ok(uid)
}

#[cfg(target_os = "macos")]
fn is_managed_macos_plist(path: &Path) -> bool {
    fs::read_to_string(path)
        .map(|contents| contents.contains(&format!("<string>{MACOS_LABEL}</string>")))
        .unwrap_or(false)
}

#[cfg(target_os = "windows")]
fn install_windows_watcher(paths: &AppPaths, binary: &Path) -> Result<()> {
    if let Some(existing) = read_windows_run_command()?
        && !is_managed_windows_run_command(&existing)
    {
        bail!("refusing to overwrite user-managed Windows startup entry {WINDOWS_RUN_VALUE}");
    }
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let (run_key, _) = current_user
        .create_subkey(WINDOWS_RUN_KEY)
        .context("could not open the current user's Windows startup registry key")?;
    run_key
        .set_value(
            WINDOWS_RUN_VALUE,
            &windows_run_command(binary, &paths.root, &paths.codex_home),
        )
        .context("could not install the codex-notify Windows startup entry")
}

#[cfg(target_os = "windows")]
fn uninstall_windows_watcher() -> Result<()> {
    let Some(existing) = read_windows_run_command()? else {
        return Ok(());
    };
    if !is_managed_windows_run_command(&existing) {
        bail!("refusing to remove user-managed Windows startup entry {WINDOWS_RUN_VALUE}");
    }
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let run_key = current_user
        .open_subkey_with_flags(WINDOWS_RUN_KEY, KEY_READ | KEY_WRITE)
        .context("could not open the current user's Windows startup registry key")?;
    run_key
        .delete_value(WINDOWS_RUN_VALUE)
        .context("could not remove the codex-notify Windows startup entry")
}

#[cfg(target_os = "windows")]
fn read_windows_run_command() -> Result<Option<String>> {
    let current_user = RegKey::predef(HKEY_CURRENT_USER);
    let run_key = match current_user.open_subkey(WINDOWS_RUN_KEY) {
        Ok(key) => key,
        Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error)
                .context("could not open the current user's Windows startup registry key");
        }
    };
    match run_key.get_value(WINDOWS_RUN_VALUE) {
        Ok(command) => Ok(Some(command)),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).context("could not read the codex-notify Windows startup entry"),
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
    use super::{is_managed_windows_run_command, macos_plist, windows_run_command};
    use std::path::Path;

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
}
