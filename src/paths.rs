use anyhow::{Context, Result};
#[cfg(not(any(target_os = "macos", target_os = "windows")))]
use directories::ProjectDirs;
use directories::UserDirs;
use std::env;
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
        let root = env::var_os("CODEX_NOTIFY_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(default_application_directory);
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

#[cfg(target_os = "macos")]
fn default_application_directory() -> PathBuf {
    default_home()
        .join("Library")
        .join("Application Support")
        .join("codex-notify")
}

#[cfg(target_os = "windows")]
fn default_application_directory() -> PathBuf {
    env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|| default_home().join("AppData").join("Roaming"))
        .join("codex-notify")
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn default_application_directory() -> PathBuf {
    ProjectDirs::from("dev", "codex-notify", "codex-notify")
        .map(|directories| directories.data_dir().to_path_buf())
        .unwrap_or_else(|| default_home().join(".codex-notify"))
}

fn default_codex_home() -> PathBuf {
    default_home().join(".codex")
}

fn default_home() -> PathBuf {
    UserDirs::new()
        .map(|directories| directories.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}
