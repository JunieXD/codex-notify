use anyhow::{Context, Result};
use keyring::Entry;

use crate::settings::FeishuConfig;

const SERVICE_NAME: &str = "codex-notify";

pub trait SecretStore {
    fn set_feishu_secret(&self, config: &FeishuConfig, secret: &str) -> Result<()>;
    fn get_feishu_secret(&self, config: &FeishuConfig) -> Result<String>;
    fn delete_feishu_secret(&self, config: &FeishuConfig) -> Result<()>;
}

#[derive(Debug, Default)]
pub struct KeyringSecretStore;

impl KeyringSecretStore {
    fn entry(&self, config: &FeishuConfig) -> Result<Entry> {
        Entry::new(SERVICE_NAME, &config.secret_account_name()).context("无法访问系统凭据库")
    }
}

impl SecretStore for KeyringSecretStore {
    fn set_feishu_secret(&self, config: &FeishuConfig, secret: &str) -> Result<()> {
        self.entry(config)?
            .set_password(secret)
            .context("无法将飞书 App Secret 保存到系统凭据库")
    }

    fn get_feishu_secret(&self, config: &FeishuConfig) -> Result<String> {
        self.entry(config)?
            .get_password()
            .context("系统凭据库中没有可用的飞书 App Secret")
    }

    fn delete_feishu_secret(&self, config: &FeishuConfig) -> Result<()> {
        self.entry(config)?
            .delete_credential()
            .context("无法从系统凭据库删除飞书 App Secret")
    }
}
