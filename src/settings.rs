use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
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
    pub codex_config_path: String,
    pub codex_hooks_path: String,
    pub prompt_hook_marker: String,
}

impl InstallationConfig {
    pub fn validate(&self) -> Result<()> {
        if self.managed_notify.is_empty() {
            bail!("managed Codex notify command is missing");
        }
        if self.prompt_hook_marker.trim().is_empty() {
            bail!("managed prompt Hook marker is missing");
        }
        Ok(())
    }
}

pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let parent = path
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
    if path.exists() {
        fs::remove_file(path).with_context(|| format!("could not replace {}", path.display()))?;
    }
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("could not save {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{AppConfig, FeishuConfig, InstallationConfig, ReceiverIdType};
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
                codex_config_path: "/tmp/config.toml".to_owned(),
                codex_hooks_path: "/tmp/hooks.json".to_owned(),
                prompt_hook_marker: "codex-notify: record task context".to_owned(),
            },
        );

        config.save(&paths).expect("save config");
        let contents = std::fs::read_to_string(&paths.config).expect("read config");
        assert!(!contents.contains("app_secret"));
        assert_eq!(AppConfig::load(&paths).expect("load config"), Some(config));
    }
}
