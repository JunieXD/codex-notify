use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use toml_edit::de::from_str;
use toml_edit::ser::to_string_pretty;

use crate::paths::AppPaths;

pub const CONFIG_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppConfig {
    pub version: u32,
    pub feishu: FeishuConfig,
    pub installation: InstallationConfig,
}

impl AppConfig {
    pub fn new(feishu: FeishuConfig, installation: InstallationConfig) -> Self {
        Self {
            version: CONFIG_VERSION,
            feishu,
            installation,
        }
    }

    pub fn load(paths: &AppPaths) -> Result<Option<Self>> {
        let contents = match fs::read_to_string(&paths.config) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("could not read {}", paths.config.display()));
            }
        };
        let config: Self = from_str(&contents)
            .with_context(|| format!("could not parse {}", paths.config.display()))?;
        config.validate()?;
        Ok(Some(config))
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        self.validate()?;
        paths.ensure_directories()?;
        let contents = to_string_pretty(self).context("could not serialize codex-notify config")?;
        atomic_write(&paths.config, contents.as_bytes())
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != CONFIG_VERSION {
            bail!(
                "unsupported codex-notify config version {}; expected {}",
                self.version,
                CONFIG_VERSION
            );
        }
        self.feishu.validate()?;
        self.installation.validate()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeishuConfig {
    pub app_id: String,
    pub receiver_id_type: ReceiverIdType,
    pub receiver_id: String,
}

impl FeishuConfig {
    pub fn validate(&self) -> Result<()> {
        if self.app_id.trim().is_empty() {
            bail!("Feishu App ID must not be empty");
        }
        if self.receiver_id.trim().is_empty() {
            bail!("Feishu receiver ID must not be empty");
        }
        Ok(())
    }

    pub fn secret_account_name(&self) -> String {
        format!("feishu-app-secret:{}", self.app_id.trim())
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, ValueEnum, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReceiverIdType {
    #[value(name = "open_id")]
    OpenId,
    #[value(name = "user_id")]
    UserId,
    #[value(name = "email")]
    Email,
    #[value(name = "chat_id")]
    ChatId,
}

impl ReceiverIdType {
    pub fn as_api_value(self) -> &'static str {
        match self {
            Self::OpenId => "open_id",
            Self::UserId => "user_id",
            Self::Email => "email",
            Self::ChatId => "chat_id",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstallationConfig {
    pub previous_notify: Option<Vec<String>>,
    pub managed_notify: Vec<String>,
    #[serde(default)]
    pub managed_binary_paths: Vec<String>,
    #[serde(default)]
    pub managed_config_paths: Vec<String>,
    pub codex_config_path: String,
    pub codex_hooks_path: String,
    pub prompt_hook_marker: String,
    #[serde(default = "default_stop_hook_marker")]
    pub stop_hook_marker: String,
    #[serde(default)]
    pub created_codex_config: bool,
    #[serde(default)]
    pub created_codex_hooks: bool,
}

impl InstallationConfig {
    pub fn validate(&self) -> Result<()> {
        if self.managed_notify.len() < 2 {
            bail!("managed Codex notify command is missing");
        }
        if self.prompt_hook_marker.trim().is_empty() {
            bail!("managed prompt Hook marker is missing");
        }
        if self.stop_hook_marker.trim().is_empty() {
            bail!("managed Stop Hook marker is missing");
        }
        Ok(())
    }
}

fn default_stop_hook_marker() -> String {
    "codex-notify: record interruption fallback".to_owned()
}

pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let destination = resolved_write_path(path)?;
    let parent = destination
        .parent()
        .context("configuration path does not have a parent directory")?;
    fs::create_dir_all(parent).with_context(|| format!("could not create {}", parent.display()))?;

    let mut temporary = NamedTempFile::new_in(parent)
        .with_context(|| format!("could not create temporary file in {}", parent.display()))?;
    use std::io::Write;
    temporary
        .write_all(contents)
        .with_context(|| format!("could not write {}", path.display()))?;
    temporary
        .flush()
        .with_context(|| format!("could not flush {}", path.display()))?;

    #[cfg(windows)]
    if destination.exists() {
        fs::remove_file(&destination)
            .with_context(|| format!("could not replace {}", destination.display()))?;
    }
    temporary
        .persist(&destination)
        .map_err(|error| error.error)
        .with_context(|| format!("could not save {}", destination.display()))?;
    Ok(())
}

/// Resolve the file an atomic replacement should target without replacing a
/// configuration manager's symbolic link with a regular file.
pub fn resolved_write_path(path: &Path) -> Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => fs::canonicalize(path)
            .with_context(|| format!("could not resolve symbolic link {}", path.display())),
        Ok(_) => Ok(path.to_path_buf()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(error) => Err(error).with_context(|| format!("could not inspect {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, FeishuConfig, InstallationConfig, ReceiverIdType, atomic_write};
    use crate::paths::AppPaths;
    use tempfile::tempdir;

    #[test]
    fn config_round_trip_does_not_contain_the_app_secret() {
        let app_home = tempdir().expect("temporary app home");
        let codex_home = tempdir().expect("temporary Codex home");
        let paths = AppPaths {
            root: app_home.path().to_path_buf(),
            config: app_home.path().join("config.toml"),
            state: app_home.path().join("state"),
            logs: app_home.path().join("logs"),
            backups: app_home.path().join("backups"),
            codex_home: codex_home.path().to_path_buf(),
        };
        let config = AppConfig::new(
            FeishuConfig {
                app_id: "cli_test_123".to_owned(),
                receiver_id_type: ReceiverIdType::OpenId,
                receiver_id: "ou_test".to_owned(),
            },
            InstallationConfig {
                previous_notify: Some(vec!["existing-notifier".to_owned()]),
                managed_notify: vec!["codex-notify".to_owned(), "notify".to_owned()],
                managed_binary_paths: vec!["codex-notify".to_owned()],
                managed_config_paths: vec!["/tmp/config.toml".to_owned()],
                codex_config_path: "/tmp/config.toml".to_owned(),
                codex_hooks_path: "/tmp/hooks.json".to_owned(),
                prompt_hook_marker: "codex-notify: record task context".to_owned(),
                stop_hook_marker: "codex-notify: record interruption fallback".to_owned(),
                created_codex_config: false,
                created_codex_hooks: false,
            },
        );

        config.save(&paths).expect("save config");
        let contents = std::fs::read_to_string(&paths.config).expect("read config");
        assert!(!contents.contains("app_secret"));
        assert_eq!(AppConfig::load(&paths).expect("load config"), Some(config));
    }

    #[test]
    fn m1_installation_config_defaults_the_new_stop_hook_marker() {
        let installation: InstallationConfig = toml_edit::de::from_str(
            r#"
previous_notify = ["old-notifier"]
managed_notify = ["codex-notify", "notify"]
codex_config_path = "/tmp/config.toml"
codex_hooks_path = "/tmp/hooks.json"
prompt_hook_marker = "codex-notify: record task context"
"#,
        )
        .expect("parse M1 installation config");

        assert_eq!(
            installation.stop_hook_marker,
            "codex-notify: record interruption fallback"
        );
        assert!(!installation.created_codex_config);
        assert!(!installation.created_codex_hooks);
        assert!(installation.managed_binary_paths.is_empty());
        assert!(installation.managed_config_paths.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn atomic_write_preserves_a_configuration_symlink() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().expect("temporary directory");
        let target = directory.path().join("profile-a.toml");
        let link = directory.path().join("config.toml");
        std::fs::write(&target, "model = \"before\"\n").expect("write target");
        symlink(&target, &link).expect("create symlink");

        atomic_write(&link, b"model = \"after\"\n").expect("atomic write");

        assert!(
            std::fs::symlink_metadata(&link)
                .expect("link metadata")
                .file_type()
                .is_symlink()
        );
        assert_eq!(
            std::fs::read_to_string(&target).expect("read target"),
            "model = \"after\"\n"
        );
    }
}
