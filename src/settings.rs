use anyhow::{Context, Result, bail};
use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};
use tempfile::NamedTempFile;
use toml_edit::de::from_str;
use toml_edit::ser::to_string_pretty;

use crate::paths::AppPaths;

pub const CONFIG_VERSION: u32 = 2;
pub const DEFAULT_FEISHU_PROVIDER_ID: &str = "feishu";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AppConfig {
    pub version: u32,
    #[serde(default)]
    pub notifications: NotificationConfig,
    pub providers: BTreeMap<String, ProviderConfig>,
    pub installation: InstallationConfig,
}

impl AppConfig {
    pub fn new(feishu: FeishuConfig, installation: InstallationConfig) -> Self {
        let providers = BTreeMap::from([(
            DEFAULT_FEISHU_PROVIDER_ID.to_owned(),
            ProviderConfig::Feishu {
                enabled: true,
                config: feishu,
            },
        )]);
        Self {
            version: CONFIG_VERSION,
            notifications: NotificationConfig::default(),
            providers,
            installation,
        }
    }

    pub fn load(paths: &AppPaths) -> Result<Option<Self>> {
        match Self::load_stored(paths)? {
            Some(StoredConfig::Current(config)) => Ok(Some(config)),
            Some(StoredConfig::Legacy(_)) => {
                bail!("检测到旧版 codex-notify 配置，需要先迁移 App Secret")
            }
            None => Ok(None),
        }
    }

    pub(crate) fn load_stored(paths: &AppPaths) -> Result<Option<StoredConfig>> {
        let contents = match fs::read_to_string(&paths.config) {
            Ok(contents) => contents,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => {
                return Err(error).with_context(|| format!("无法读取 {}", paths.config.display()));
            }
        };
        let version = from_str::<ConfigVersion>(&contents)
            .with_context(|| format!("无法解析配置文件 {}", paths.config.display()))?
            .version;
        match version {
            CONFIG_VERSION => {
                let config: Self = from_str(&contents)
                    .with_context(|| format!("无法解析配置文件 {}", paths.config.display()))?;
                config.validate()?;
                Ok(Some(StoredConfig::Current(config)))
            }
            1 => {
                let config: LegacyAppConfig = from_str(&contents)
                    .with_context(|| format!("无法解析旧版配置文件 {}", paths.config.display()))?;
                config.validate()?;
                Ok(Some(StoredConfig::Legacy(config)))
            }
            version => bail!(
                "不支持 codex-notify 配置版本 {}，当前支持的版本是 {}",
                version,
                CONFIG_VERSION
            ),
        }
    }

    pub fn save(&self, paths: &AppPaths) -> Result<()> {
        self.validate()?;
        paths.ensure_directories()?;
        let contents = to_string_pretty(self).context("无法生成 codex-notify 配置内容")?;
        atomic_write(&paths.config, contents.as_bytes())
    }

    pub fn validate(&self) -> Result<()> {
        if self.version != CONFIG_VERSION {
            bail!(
                "不支持 codex-notify 配置版本 {}，当前支持的版本是 {}",
                self.version,
                CONFIG_VERSION
            );
        }
        if self.providers.is_empty() {
            bail!("至少需要配置一个通知平台");
        }
        let mut enabled = 0usize;
        for (id, provider) in &self.providers {
            if id.trim().is_empty() {
                bail!("通知平台实例 ID 不能为空");
            }
            provider.validate()?;
            if provider.enabled() {
                enabled += 1;
            }
        }
        if enabled == 0 {
            bail!("至少需要启用一个通知平台");
        }
        self.installation.validate()
    }

    pub fn feishu(&self) -> Result<&FeishuConfig> {
        self.providers
            .values()
            .find_map(ProviderConfig::enabled_feishu)
            .context("没有启用的飞书通知配置")
    }

    pub fn replace_feishu(&mut self, config: FeishuConfig) {
        let id = self
            .providers
            .iter()
            .find_map(|(id, provider)| provider.is_feishu().then(|| id.clone()))
            .unwrap_or_else(|| DEFAULT_FEISHU_PROVIDER_ID.to_owned());
        self.providers.insert(
            id,
            ProviderConfig::Feishu {
                enabled: true,
                config,
            },
        );
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct NotificationConfig {
    /// Subagent/side-thread transcripts are excluded unless explicitly enabled.
    #[serde(default)]
    pub include_subagents: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ProviderConfig {
    Feishu {
        #[serde(default = "default_provider_enabled")]
        enabled: bool,
        #[serde(flatten)]
        config: FeishuConfig,
    },
}

impl ProviderConfig {
    fn validate(&self) -> Result<()> {
        match self {
            Self::Feishu { config, .. } => config.validate(),
        }
    }

    fn enabled(&self) -> bool {
        match self {
            Self::Feishu { enabled, .. } => *enabled,
        }
    }

    fn enabled_feishu(&self) -> Option<&FeishuConfig> {
        match self {
            Self::Feishu {
                enabled: true,
                config,
            } => Some(config),
            Self::Feishu { enabled: false, .. } => None,
        }
    }

    fn is_feishu(&self) -> bool {
        matches!(self, Self::Feishu { .. })
    }
}

fn default_provider_enabled() -> bool {
    true
}

#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeishuConfig {
    pub app_id: String,
    pub app_secret: String,
    pub receiver_id_type: ReceiverIdType,
    pub receiver_id: String,
}

impl fmt::Debug for FeishuConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("FeishuConfig")
            .field("app_id", &self.app_id)
            .field("app_secret", &"[REDACTED]")
            .field("receiver_id_type", &self.receiver_id_type)
            .field("receiver_id", &self.receiver_id)
            .finish()
    }
}

impl FeishuConfig {
    pub fn validate(&self) -> Result<()> {
        if self.app_id.trim().is_empty() {
            bail!("飞书 App ID 不能为空");
        }
        if self.app_secret.trim().is_empty() {
            bail!("飞书 App Secret 不能为空");
        }
        if self.receiver_id.trim().is_empty() {
            bail!("飞书接收者 ID 不能为空");
        }
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
struct ConfigVersion {
    version: u32,
}

#[derive(Debug)]
pub(crate) enum StoredConfig {
    Current(AppConfig),
    Legacy(LegacyAppConfig),
}

impl StoredConfig {
    pub fn installation(&self) -> &InstallationConfig {
        match self {
            Self::Current(config) => &config.installation,
            Self::Legacy(config) => &config.installation,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LegacyAppConfig {
    version: u32,
    pub feishu: LegacyFeishuConfig,
    pub installation: InstallationConfig,
}

impl LegacyAppConfig {
    fn validate(&self) -> Result<()> {
        if self.version != 1 {
            bail!("旧版配置版本必须是 1");
        }
        self.feishu.validate()?;
        self.installation.validate()
    }

    pub fn into_current(self, app_secret: String) -> AppConfig {
        AppConfig::new(
            FeishuConfig {
                app_id: self.feishu.app_id,
                app_secret,
                receiver_id_type: self.feishu.receiver_id_type,
                receiver_id: self.feishu.receiver_id,
            },
            self.installation,
        )
    }
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct LegacyFeishuConfig {
    pub app_id: String,
    pub receiver_id_type: ReceiverIdType,
    pub receiver_id: String,
}

impl LegacyFeishuConfig {
    fn validate(&self) -> Result<()> {
        if self.app_id.trim().is_empty() {
            bail!("飞书 App ID 不能为空");
        }
        if self.receiver_id.trim().is_empty() {
            bail!("飞书接收者 ID 不能为空");
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
            bail!("缺少由 codex-notify 管理的 Codex notify 命令");
        }
        if self.prompt_hook_marker.trim().is_empty() {
            bail!("缺少由 codex-notify 管理的 UserPromptSubmit Hook 标记");
        }
        if self.stop_hook_marker.trim().is_empty() {
            bail!("缺少由 codex-notify 管理的 Stop Hook 标记");
        }
        Ok(())
    }
}

fn default_stop_hook_marker() -> String {
    "codex-notify: record interruption fallback".to_owned()
}

pub fn atomic_write(path: &Path, contents: &[u8]) -> Result<()> {
    let destination = resolved_write_path(path)?;
    let parent = destination.parent().context("无法确定配置文件所在目录")?;
    fs::create_dir_all(parent).with_context(|| format!("无法创建目录 {}", parent.display()))?;

    let mut temporary = NamedTempFile::new_in(parent)
        .with_context(|| format!("无法在 {} 中创建临时文件", parent.display()))?;
    use std::io::Write;
    temporary
        .write_all(contents)
        .with_context(|| format!("无法写入 {}", path.display()))?;
    temporary
        .flush()
        .with_context(|| format!("无法保存 {} 的内容", path.display()))?;

    #[cfg(windows)]
    if destination.exists() {
        fs::remove_file(&destination)
            .with_context(|| format!("无法替换 {}", destination.display()))?;
    }
    temporary
        .persist(&destination)
        .map_err(|error| error.error)
        .with_context(|| format!("无法保存 {}", destination.display()))?;
    Ok(())
}

/// Resolve the file an atomic replacement should target without replacing a
/// configuration manager's symbolic link with a regular file.
pub fn resolved_write_path(path: &Path) -> Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            fs::canonicalize(path).with_context(|| format!("无法解析符号链接 {}", path.display()))
        }
        Ok(_) => Ok(path.to_path_buf()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(path.to_path_buf()),
        Err(error) => Err(error).with_context(|| format!("无法检查 {}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::atomic_write;
    use super::{AppConfig, FeishuConfig, InstallationConfig, ReceiverIdType};
    use crate::paths::AppPaths;
    use tempfile::tempdir;

    #[test]
    fn config_round_trip_keeps_provider_credentials_in_the_toml_file() {
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
                app_secret: "secret_test_123".to_owned(),
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
        assert!(contents.contains("[providers.feishu]"));
        assert!(contents.contains("type = \"feishu\""));
        assert!(contents.contains("app_secret = \"secret_test_123\""));
        assert!(contents.contains("[notifications]"));
        assert!(contents.contains("include_subagents = false"));
        assert!(!format!("{config:?}").contains("secret_test_123"));
        assert_eq!(AppConfig::load(&paths).expect("load config"), Some(config));
    }

    #[test]
    fn existing_version_two_config_defaults_to_excluding_subagents() {
        let config: AppConfig = toml_edit::de::from_str(
            r#"
version = 2

[providers.feishu]
type = "feishu"
enabled = true
app_id = "cli_existing"
app_secret = "secret_existing"
receiver_id_type = "email"
receiver_id = "owner@example.com"

[installation]
managed_notify = ["codex-notify", "notify"]
codex_config_path = "/tmp/config.toml"
codex_hooks_path = "/tmp/hooks.json"
prompt_hook_marker = "codex-notify: record task context"
"#,
        )
        .expect("parse existing version two config");

        assert!(!config.notifications.include_subagents);
    }

    #[cfg(unix)]
    #[test]
    fn saved_config_is_readable_only_by_the_current_user() {
        use std::os::unix::fs::PermissionsExt;

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
                app_id: "cli_test_permissions".to_owned(),
                app_secret: "secret_permissions".to_owned(),
                receiver_id_type: ReceiverIdType::Email,
                receiver_id: "test@example.com".to_owned(),
            },
            InstallationConfig {
                previous_notify: None,
                managed_notify: vec!["codex-notify".to_owned(), "notify".to_owned()],
                managed_binary_paths: Vec::new(),
                managed_config_paths: Vec::new(),
                codex_config_path: "/tmp/config.toml".to_owned(),
                codex_hooks_path: "/tmp/hooks.json".to_owned(),
                prompt_hook_marker: "codex-notify: record task context".to_owned(),
                stop_hook_marker: "codex-notify: record interruption fallback".to_owned(),
                created_codex_config: false,
                created_codex_hooks: false,
            },
        );

        config.save(&paths).expect("save config");

        assert_eq!(
            std::fs::metadata(&paths.config)
                .expect("config metadata")
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::metadata(&paths.root)
                .expect("root metadata")
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
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
