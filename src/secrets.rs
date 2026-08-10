#[cfg(target_os = "macos")]
use anyhow::bail;
use anyhow::{Context, Result};

use keyring::Entry;
#[cfg(target_os = "macos")]
use std::ffi::OsString;
#[cfg(target_os = "macos")]
use std::process::{Command, Output};

use crate::settings::FeishuConfig;

const SERVICE_NAME: &str = "codex-notify";
#[cfg(target_os = "macos")]
const MACOS_SECURITY: &str = "/usr/bin/security";

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

    /// Make an existing macOS credential trust Apple's stable `security`
    /// helper instead of the hash of one particular codex-notify binary.
    ///
    /// Older releases used the native Keychain API directly. macOS therefore
    /// attached the executable's cdhash to the item, and every self-update
    /// could block the background watcher behind a new authorization dialog.
    /// Reading once in the foreground through `/usr/bin/security` lets the
    /// user add that stable Apple-signed helper to the ACL.
    pub fn prepare_stable_access(&self, config: &FeishuConfig) -> Result<()> {
        #[cfg(target_os = "macos")]
        {
            self.get_feishu_secret(config)?;
        }
        #[cfg(not(target_os = "macos"))]
        let _ = config;
        Ok(())
    }
}

#[cfg(not(target_os = "macos"))]
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

#[cfg(target_os = "macos")]
impl SecretStore for KeyringSecretStore {
    fn set_feishu_secret(&self, config: &FeishuConfig, secret: &str) -> Result<()> {
        self.entry(config)?
            .set_password(secret)
            .context("无法将飞书 App Secret 保存到 macOS 钥匙串")
    }

    fn get_feishu_secret(&self, config: &FeishuConfig) -> Result<String> {
        let output = Command::new(MACOS_SECURITY)
            .args(macos_find_arguments(&config.secret_account_name()))
            .output()
            .context("无法启动 macOS 系统凭据工具")?;
        let output = require_macos_security_success(output, "读取飞书 App Secret")?;
        decode_macos_secret(output.stdout)
    }

    fn delete_feishu_secret(&self, config: &FeishuConfig) -> Result<()> {
        self.entry(config)?
            .delete_credential()
            .context("无法从 macOS 钥匙串删除飞书 App Secret")
    }
}

#[cfg(target_os = "macos")]
fn macos_find_arguments(account: &str) -> Vec<OsString> {
    [
        "find-generic-password",
        "-a",
        account,
        "-s",
        SERVICE_NAME,
        "-w",
    ]
    .into_iter()
    .map(OsString::from)
    .collect()
}

#[cfg(target_os = "macos")]
fn require_macos_security_success(output: Output, action: &str) -> Result<Output> {
    if output.status.success() {
        return Ok(output);
    }
    let detail = String::from_utf8_lossy(&output.stderr);
    let detail = detail.trim();
    if detail.is_empty() {
        bail!("macOS 系统凭据工具无法{action}（{}）", output.status);
    }
    bail!("macOS 系统凭据工具无法{action}：{detail}")
}

#[cfg(target_os = "macos")]
fn decode_macos_secret(mut bytes: Vec<u8>) -> Result<String> {
    if bytes.last() == Some(&b'\n') {
        bytes.pop();
    }
    if bytes.last() == Some(&b'\r') {
        bytes.pop();
    }
    let secret = String::from_utf8(bytes).context("macOS 系统凭据工具返回了无效文本")?;
    if secret.is_empty() {
        bail!("系统凭据库中的飞书 App Secret 为空");
    }
    Ok(secret)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn strings(values: Vec<OsString>) -> Vec<String> {
        values
            .into_iter()
            .map(|value| value.into_string().expect("argument should be UTF-8"))
            .collect()
    }

    #[test]
    fn macos_reads_through_the_stable_system_helper() {
        assert_eq!(
            strings(macos_find_arguments("account")),
            [
                "find-generic-password",
                "-a",
                "account",
                "-s",
                "codex-notify",
                "-w",
            ]
        );
    }

    #[test]
    fn macos_secret_decoder_removes_only_the_command_line_ending() {
        assert_eq!(
            decode_macos_secret(b" secret with spaces \r\n".to_vec()).expect("decode secret"),
            " secret with spaces "
        );
    }

    #[test]
    fn macos_secret_decoder_rejects_empty_output() {
        assert!(decode_macos_secret(b"\n".to_vec()).is_err());
    }
}
