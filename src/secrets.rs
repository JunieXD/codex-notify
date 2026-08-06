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
        Entry::new(SERVICE_NAME, &config.secret_account_name())
            .context("could not access the operating system credential store")
    }
}

impl SecretStore for KeyringSecretStore {
    fn set_feishu_secret(&self, config: &FeishuConfig, secret: &str) -> Result<()> {
        self.entry(config)?.set_password(secret).context(
            "could not save the Feishu App Secret in the operating system credential store",
        )
    }

    fn get_feishu_secret(&self, config: &FeishuConfig) -> Result<String> {
        self.entry(config)?
            .get_password()
            .context("Feishu App Secret is not available in the operating system credential store")
    }

    fn delete_feishu_secret(&self, config: &FeishuConfig) -> Result<()> {
        self.entry(config)?.delete_credential().context(
            "could not remove the Feishu App Secret from the operating system credential store",
        )
    }
}
