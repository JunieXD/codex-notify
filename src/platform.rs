//! Per-user background service integration for the transcript watcher.

use anyhow::{Context, Result, bail};
#[cfg(target_os = "macos")]
use directories::UserDirs;
use std::fs;
use std::path::Path;
#[cfg(target_os = "macos")]
use std::path::PathBuf;
use std::process::Command;
#[cfg(target_os = "windows")]
use tempfile::NamedTempFile;

use crate::paths::AppPaths;
#[cfg(target_os = "macos")]
use crate::settings::atomic_write;

#[cfg(any(target_os = "macos", test))]
const MACOS_LABEL: &str = "com.codex-notify.watcher";
#[cfg(any(target_os = "windows", test))]
const WINDOWS_TASK_NAME: &str = "Codex Notify Watcher";

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
        Ok(Command::new("schtasks.exe")
            .args(["/Query", "/TN", WINDOWS_TASK_NAME])
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false))
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
        Ok(format!("Task Scheduler: {WINDOWS_TASK_NAME}"))
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
    fs::create_dir_all(&paths.state)
        .with_context(|| format!("could not create {}", paths.state.display()))?;
    let mut temporary = NamedTempFile::new_in(&paths.state)
        .with_context(|| format!("could not create a task file in {}", paths.state.display()))?;
    use std::io::Write;
    temporary
        .write_all(windows_task_xml(binary, &paths.root).as_bytes())
        .context("could not write Windows task definition")?;
    temporary
        .flush()
        .context("could not flush Windows task definition")?;
    // NamedTempFile keeps its handle open, which prevents schtasks.exe from
    // reading the XML on Windows. Keep the auto-deleting path alive while
    // closing the file handle before starting the child process.
    let temporary_path = temporary.into_temp_path();
    let temporary_path_string = temporary_path.display().to_string();
    let status = Command::new("schtasks.exe")
        .args([
            "/Create",
            "/TN",
            WINDOWS_TASK_NAME,
            "/XML",
            &temporary_path_string,
            "/F",
        ])
        .status()
        .context("could not create the Windows Task Scheduler task")?;
    if !status.success() {
        bail!("schtasks could not create the codex-notify watcher ({status})");
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn uninstall_windows_watcher() -> Result<()> {
    let exists = Command::new("schtasks.exe")
        .args(["/Query", "/TN", WINDOWS_TASK_NAME])
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false);
    if !exists {
        return Ok(());
    }
    let status = Command::new("schtasks.exe")
        .args(["/Delete", "/TN", WINDOWS_TASK_NAME, "/F"])
        .status()
        .context("could not remove the Windows Task Scheduler task")?;
    if !status.success() {
        bail!("schtasks could not remove the codex-notify watcher ({status})");
    }
    Ok(())
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
fn windows_task_xml(binary: &Path, app_data: &Path) -> String {
    let binary = xml_escape(&binary.display().to_string());
    let app_data = xml_escape(&app_data.display().to_string());
    format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<Task version="1.4" xmlns="http://schemas.microsoft.com/windows/2004/02/mit/task">
  <RegistrationInfo><URI>\{WINDOWS_TASK_NAME}</URI></RegistrationInfo>
  <Triggers><LogonTrigger><Enabled>true</Enabled></LogonTrigger></Triggers>
  <Principals><Principal id="Author"><RunLevel>LeastPrivilege</RunLevel></Principal></Principals>
  <Settings>
    <MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>
    <StartWhenAvailable>true</StartWhenAvailable>
    <ExecutionTimeLimit>PT0S</ExecutionTimeLimit>
    <RestartOnFailure><Interval>PT1M</Interval><Count>3</Count></RestartOnFailure>
  </Settings>
  <Actions Context="Author">
    <Exec><Command>{binary}</Command><Arguments>watch</Arguments><WorkingDirectory>{app_data}</WorkingDirectory></Exec>
  </Actions>
</Task>
"#
    )
}

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
    use super::{macos_plist, windows_task_xml};
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
    fn windows_task_restarts_a_single_watcher_at_logon() {
        let task = windows_task_xml(
            Path::new("C:\\bin\\codex-notify.exe"),
            Path::new("C:\\data"),
        );
        assert!(task.contains("<LogonTrigger>"));
        assert!(task.contains("<MultipleInstancesPolicy>IgnoreNew</MultipleInstancesPolicy>"));
        assert!(task.contains("<RestartOnFailure>"));
        assert!(task.contains("<Arguments>watch</Arguments>"));
    }
}
