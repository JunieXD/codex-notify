use anyhow::{Context, Result};
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use directories::ProjectDirs;
use directories::UserDirs;
use std::env;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone)]
pub struct AppPaths {
    pub root: PathBuf,
    pub config: PathBuf,
    pub state: PathBuf,
    pub logs: PathBuf,
    pub backups: PathBuf,
    pub codex_home: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self> {
        let root = match env::var_os("CODEX_NOTIFY_HOME") {
            Some(root) => PathBuf::from(root),
            None => {
                let root = default_application_directory();
                migrate_legacy_application_directory(&root)?;
                root
            }
        };
        let codex_home = env::var_os("CODEX_NOTIFY_CODEX_HOME")
            .or_else(|| env::var_os("CODEX_HOME"))
            .map(PathBuf::from)
            .unwrap_or_else(default_codex_home);

        Ok(Self {
            config: root.join("config.toml"),
            state: root.join("state"),
            logs: root.join("logs"),
            backups: root.join("backups"),
            root,
            codex_home,
        })
    }

    pub fn ensure_directories(&self) -> Result<()> {
        for path in [&self.root, &self.state, &self.logs, &self.backups] {
            std::fs::create_dir_all(path)
                .with_context(|| format!("无法创建目录 {}", path.display()))?;
            restrict_directory_permissions(path)?;
        }
        Ok(())
    }

    pub fn codex_config(&self) -> PathBuf {
        self.codex_home.join("config.toml")
    }

    pub fn codex_hooks(&self) -> PathBuf {
        self.codex_home.join("hooks.json")
    }

    pub fn session_index(&self) -> PathBuf {
        self.codex_home.join("session_index.jsonl")
    }

    pub fn diagnostics_log(&self) -> PathBuf {
        self.logs.join("codex-notify.log")
    }
}

fn default_application_directory() -> PathBuf {
    default_home().join(".codex-notify")
}

fn migrate_legacy_application_directory(root: &Path) -> Result<()> {
    let legacy = legacy_application_directory();
    if legacy == root || root.exists() || !legacy.exists() {
        return Ok(());
    }
    copy_application_directory(&legacy, root)
}

fn copy_application_directory(legacy: &Path, root: &Path) -> Result<()> {
    let result = copy_directory_contents(legacy, root);
    if let Err(error) = result {
        let _ = remove_copied_directory(root);
        return Err(error).with_context(|| {
            format!(
                "无法将旧版应用数据目录 {} 迁移到 {}",
                legacy.display(),
                root.display()
            )
        });
    }
    Ok(())
}

fn copy_directory_contents(source: &Path, destination: &Path) -> Result<()> {
    std::fs::create_dir(destination)
        .with_context(|| format!("无法创建目录 {}", destination.display()))?;
    for entry in
        std::fs::read_dir(source).with_context(|| format!("无法读取目录 {}", source.display()))?
    {
        let entry = entry.with_context(|| format!("无法读取目录 {}", source.display()))?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        let file_type = entry
            .file_type()
            .with_context(|| format!("无法检查 {}", source_path.display()))?;
        if file_type.is_symlink() {
            anyhow::bail!(
                "旧版应用数据中包含无法自动迁移的符号链接 {}",
                source_path.display()
            );
        }
        if file_type.is_dir() {
            copy_directory_contents(&source_path, &destination_path)?;
        } else {
            std::fs::copy(&source_path, &destination_path).with_context(|| {
                format!(
                    "无法将 {} 复制到 {}",
                    source_path.display(),
                    destination_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn remove_copied_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        return Ok(());
    }
    for entry in
        std::fs::read_dir(path).with_context(|| format!("无法读取目录 {}", path.display()))?
    {
        let entry = entry.with_context(|| format!("无法读取目录 {}", path.display()))?;
        let entry_path = entry.path();
        if entry
            .file_type()
            .with_context(|| format!("无法检查 {}", entry_path.display()))?
            .is_dir()
        {
            remove_copied_directory(&entry_path)?;
        } else {
            std::fs::remove_file(&entry_path)
                .with_context(|| format!("无法删除 {}", entry_path.display()))?;
        }
    }
    std::fs::remove_dir(path).with_context(|| format!("无法删除 {}", path.display()))
}

#[cfg(target_os = "macos")]
fn legacy_application_directory() -> PathBuf {
    default_home()
        .join("Library")
        .join("Application Support")
        .join("codex-notify")
}

#[cfg(target_os = "windows")]
fn legacy_application_directory() -> PathBuf {
    env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_home().join("AppData").join("Roaming"))
        .join("codex-notify")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn legacy_application_directory() -> PathBuf {
    ProjectDirs::from("dev", "codex-notify", "codex-notify")
        .map(|directories| directories.data_dir().to_path_buf())
        .unwrap_or_else(|| default_home().join(".codex-notify"))
}

#[cfg(unix)]
fn restrict_directory_permissions(path: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
        .with_context(|| format!("无法限制目录 {} 的访问权限", path.display()))
}

#[cfg(not(unix))]
fn restrict_directory_permissions(_path: &Path) -> Result<()> {
    Ok(())
}

fn default_codex_home() -> PathBuf {
    default_home().join(".codex")
}

fn default_home() -> PathBuf {
    UserDirs::new()
        .map(|directories| directories.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

#[cfg(test)]
mod tests {
    use super::copy_application_directory;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn legacy_application_directory_copies_to_the_hidden_home_directory() {
        let home = tempdir().expect("temporary home");
        let legacy = home.path().join("legacy-codex-notify");
        let current = home.path().join(".codex-notify");
        fs::create_dir(&legacy).expect("legacy directory");
        fs::write(legacy.join("config.toml"), "version = 1\n").expect("legacy config");

        copy_application_directory(&legacy, &current).expect("copy application directory");

        assert!(legacy.exists());
        assert_eq!(
            fs::read_to_string(current.join("config.toml")).expect("migrated config"),
            "version = 1\n"
        );
    }
}
