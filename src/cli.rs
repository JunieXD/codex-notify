use anyhow::{Context, Result, anyhow, bail};
use clap::{Args, Parser, Subcommand};
use dialoguer::{Confirm, Input, Password};
use serde_json::json;
use std::fs;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;

use crate::codex::{
    CompletionEvent, PromptHookEvent, RestoreNotifyResult, backup_file, completion_notification,
    has_prompt_hook, install_integration, read_notify_command, record_prompt_context,
    remove_completion_state, remove_prompt_hook, restore_notify_command, rollback_integration,
    run_previous_notifier,
};
use crate::diagnostics;
use crate::feishu::FeishuClient;
use crate::model::Notification;
use crate::paths::AppPaths;
use crate::secrets::{KeyringSecretStore, SecretStore};
use crate::settings::{AppConfig, FeishuConfig, ReceiverIdType};

#[derive(Debug, Parser)]
#[command(
    name = "codex-notify",
    version,
    about = "Local Feishu notifications for Codex."
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// Configure Feishu and reversible Codex integration.
    Init(InitArgs),
    /// Send a Feishu test card using the current configuration.
    Test,
    /// Show local installation status without revealing secrets.
    Status(JsonOutput),
    /// Diagnose the local Codex and Feishu configuration.
    Doctor(JsonOutput),
    /// Restore the previous Codex notifier and remove codex-notify integration.
    Uninstall(UninstallArgs),
    /// M2 transcript error watcher.
    Watch,
    #[command(hide = true)]
    Notify {
        /// The single JSON argument supplied by Codex notify.
        event_json: String,
    },
    #[command(name = "prompt-hook", hide = true)]
    PromptHook,
}

#[derive(Debug, Args)]
struct InitArgs {
    /// Feishu App ID. Prompts when omitted.
    #[arg(long)]
    app_id: Option<String>,
    /// Feishu App Secret. Passing this in a shell can expose it in history.
    #[arg(long)]
    app_secret: Option<String>,
    /// Feishu receiver identifier type.
    #[arg(long, value_enum)]
    receiver_id_type: Option<ReceiverIdType>,
    /// Feishu receiver identifier.
    #[arg(long)]
    receiver_id: Option<String>,
    /// Binary path that Codex should invoke. Defaults to the running executable.
    #[arg(long)]
    binary: Option<PathBuf>,
    /// Do not prompt for confirmation.
    #[arg(short, long)]
    yes: bool,
    /// Do not send a test Feishu card after installation.
    #[arg(long)]
    skip_test: bool,
}

#[derive(Debug, Args)]
struct JsonOutput {
    /// Print machine-readable JSON.
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct UninstallArgs {
    /// Do not prompt for confirmation.
    #[arg(short, long)]
    yes: bool,
}

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Init(arguments) => init(arguments),
        Commands::Test => send_test(),
        Commands::Status(arguments) => status(arguments.json),
        Commands::Doctor(arguments) => doctor(arguments.json),
        Commands::Uninstall(arguments) => uninstall(arguments),
        Commands::Watch => Err(anyhow!(
            "watch is planned for M2; completion notifications are available now"
        )),
        Commands::Notify { event_json } => notify(event_json),
        Commands::PromptHook => prompt_hook(),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("codex-notify: {error:#}");
            ExitCode::from(1)
        }
    }
}

fn init(arguments: InitArgs) -> Result<()> {
    let paths = AppPaths::discover()?;
    let existing = AppConfig::load(&paths)?;
    let app_id = input_value(
        arguments.app_id,
        existing
            .as_ref()
            .map(|config| config.feishu.app_id.as_str()),
        "Feishu App ID",
    )?;
    let app_secret = secret_value(arguments.app_secret)?;
    let receiver_id_type = receiver_type_value(
        arguments.receiver_id_type,
        existing
            .as_ref()
            .map(|config| config.feishu.receiver_id_type),
    )?;
    let receiver_id = input_value(
        arguments.receiver_id,
        existing
            .as_ref()
            .map(|config| config.feishu.receiver_id.as_str()),
        "Feishu receiver ID",
    )?;
    let binary = resolve_binary(arguments.binary)?;
    let feishu = FeishuConfig {
        app_id,
        receiver_id_type,
        receiver_id,
    };

    println!("codex-notify will update:");
    println!("  - {}", paths.codex_config().display());
    println!("  - {}", paths.codex_hooks().display());
    println!("  - {}", paths.config.display());
    println!("The existing Codex notify command will be preserved and invoked first.");
    if !arguments.yes
        && !Confirm::new()
            .with_prompt("Continue?")
            .default(false)
            .interact()
            .context("could not read confirmation")?
    {
        println!("Canceled without changing configuration.");
        return Ok(());
    }

    let secrets = KeyringSecretStore;
    let previous_secret = existing
        .as_ref()
        .filter(|old| old.feishu.app_id == feishu.app_id)
        .and_then(|old| secrets.get_feishu_secret(&old.feishu).ok());
    secrets.set_feishu_secret(&feishu, &app_secret)?;
    let setup = match install_integration(
        &paths,
        &binary,
        existing.as_ref().map(|config| &config.installation),
    ) {
        Ok(setup) => setup,
        Err(error) => {
            restore_feishu_secret(&secrets, &feishu, previous_secret.as_deref());
            return Err(error);
        }
    };
    let config = AppConfig::new(feishu.clone(), setup.installation.clone());
    if let Err(error) = config.save(&paths) {
        let _ = rollback_integration(&paths, &setup);
        restore_feishu_secret(&secrets, &feishu, previous_secret.as_deref());
        return Err(error);
    }

    if let Some(old_config) = existing
        .as_ref()
        .filter(|old| old.feishu.app_id != feishu.app_id)
    {
        let _ = secrets.delete_feishu_secret(&old_config.feishu);
    }

    if config
        .installation
        .previous_notify
        .as_ref()
        .is_some_and(|command| looks_like_feishu_notifier(command))
    {
        eprintln!(
            "Warning: the preserved notifier appears to send Feishu messages too; it may create duplicate messages until it is migrated."
        );
    }
    println!("Codex integration is installed.");
    if let Some(path) = setup.config_backup {
        println!("Config backup: {}", path.display());
    }
    if let Some(path) = setup.hooks_backup {
        println!("Hooks backup: {}", path.display());
    }
    println!("Open /hooks in Codex and trust the new UserPromptSubmit hook before use.");

    let should_test = !arguments.skip_test
        && (arguments.yes
            || Confirm::new()
                .with_prompt("Send a Feishu test card now?")
                .default(true)
                .interact()
                .context("could not read test confirmation")?);
    if should_test {
        match send_test_for(&config) {
            Ok(()) => println!("Feishu test card sent."),
            Err(error) => {
                eprintln!(
                    "Integration is installed, but the test card failed: {error:#}\nRun codex-notify doctor after checking Feishu app permissions."
                );
            }
        }
    }
    Ok(())
}

fn send_test() -> Result<()> {
    let paths = AppPaths::discover()?;
    let config = configured(&paths)?;
    send_test_for(&config)?;
    println!("Feishu test card sent.");
    Ok(())
}

fn send_test_for(config: &AppConfig) -> Result<()> {
    let secret = KeyringSecretStore.get_feishu_secret(&config.feishu)?;
    let notification = Notification::completed(
        "Codex notification test",
        "Send a Feishu test notification",
        "The Feishu provider, credential store, and card renderer are working.",
        Some(Duration::from_secs(2)),
        "test-notification",
    );
    FeishuClient::new()?.send(&config.feishu, &secret, &notification)?;
    Ok(())
}

fn status(json_output: bool) -> Result<()> {
    let paths = AppPaths::discover()?;
    let config = AppConfig::load(&paths)?;
    let data = inspection(&paths, config.as_ref())?;
    print_inspection(&data, json_output, false);
    Ok(())
}

fn doctor(json_output: bool) -> Result<()> {
    let paths = AppPaths::discover()?;
    let config = AppConfig::load(&paths)?;
    let data = inspection(&paths, config.as_ref())?;
    print_inspection(&data, json_output, true);
    Ok(())
}

fn inspection(paths: &AppPaths, config: Option<&AppConfig>) -> Result<serde_json::Value> {
    let secret_status = config
        .map(|config| {
            if KeyringSecretStore.get_feishu_secret(&config.feishu).is_ok() {
                "present"
            } else {
                "unavailable"
            }
        })
        .unwrap_or("not_configured");
    let active_notify = read_notify_command(&paths.codex_config())?;
    let notifier_status = config
        .map(|config| {
            active_notify.as_deref() == Some(config.installation.managed_notify.as_slice())
        })
        .unwrap_or(false);
    let hook_status = has_prompt_hook(&paths.codex_hooks())?;

    Ok(json!({
        "configured": config.is_some(),
        "app_data_directory": paths.root,
        "codex_home": paths.codex_home,
        "credential_store": secret_status,
        "codex_notify_installed": notifier_status,
        "prompt_hook_installed": hook_status,
        "feishu_receiver_id_type": config.map(|config| config.feishu.receiver_id_type.as_api_value()),
    }))
}

fn print_inspection(data: &serde_json::Value, json_output: bool, include_guidance: bool) {
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(data).expect("inspection must be serializable")
        );
        return;
    }

    let configured = data["configured"].as_bool().unwrap_or(false);
    let secret = data["credential_store"].as_str().unwrap_or("unavailable");
    let notifier = data["codex_notify_installed"].as_bool().unwrap_or(false);
    let hook = data["prompt_hook_installed"].as_bool().unwrap_or(false);
    println!("Configured: {}", yes_no(configured));
    println!("Credential store: {secret}");
    println!("Codex notify dispatcher: {}", yes_no(notifier));
    println!("UserPromptSubmit hook: {}", yes_no(hook));
    if include_guidance && hook {
        println!("Hook trust: inspect and trust it with /hooks in Codex.");
    }
}

fn uninstall(arguments: UninstallArgs) -> Result<()> {
    let paths = AppPaths::discover()?;
    let config = configured(&paths)?;
    if !arguments.yes
        && !Confirm::new()
            .with_prompt("Restore the previous Codex notifier and remove codex-notify?")
            .default(false)
            .interact()
            .context("could not read confirmation")?
    {
        println!("Canceled without changing configuration.");
        return Ok(());
    }

    let config_path = PathBuf::from(&config.installation.codex_config_path);
    let hooks_path = PathBuf::from(&config.installation.codex_hooks_path);
    let _ = backup_file(&paths, &config_path, "uninstall-config")?;
    let _ = backup_file(&paths, &hooks_path, "uninstall-hooks")?;
    match restore_notify_command(&config_path, &config.installation)? {
        RestoreNotifyResult::Restored => {}
        RestoreNotifyResult::NotOwned => {
            bail!(
                "the active Codex notify command is no longer owned by codex-notify; no files were removed"
            );
        }
    }
    let _ = remove_prompt_hook(&hooks_path)?;
    let _ = KeyringSecretStore.delete_feishu_secret(&config.feishu);
    let _ = fs::remove_file(&paths.config);
    if paths.state.exists() {
        fs::remove_dir_all(&paths.state)
            .with_context(|| format!("could not remove {}", paths.state.display()))?;
    }
    println!("codex-notify integration was removed and the previous notifier was restored.");
    Ok(())
}

fn notify(event_json: String) -> Result<()> {
    let paths = AppPaths::discover()?;
    let Some(config) = AppConfig::load(&paths)? else {
        diagnostics::record(
            &paths,
            "notify skipped because codex-notify is not configured",
        );
        return Ok(());
    };

    if let Some(previous) = config.installation.previous_notify.as_ref() {
        if run_previous_notifier(previous, &event_json).is_err() {
            diagnostics::record(&paths, "previous Codex notifier failed");
        }
    }

    let event: CompletionEvent = match serde_json::from_str(&event_json) {
        Ok(event) => event,
        Err(_) => {
            diagnostics::record(&paths, "Codex notify received invalid event JSON");
            return Ok(());
        }
    };
    if !event.is_completion() || event.is_internal() {
        return Ok(());
    }

    let result: Result<()> = (|| {
        let notification = completion_notification(&paths, &event)?;
        let secret = KeyringSecretStore.get_feishu_secret(&config.feishu)?;
        FeishuClient::new()?.send(&config.feishu, &secret, &notification)?;
        Ok(())
    })();
    let _ = remove_completion_state(&paths, &event);
    if result.is_err() {
        diagnostics::record(&paths, "Feishu completion notification failed");
    }
    Ok(())
}

fn prompt_hook() -> Result<()> {
    let paths = AppPaths::discover()?;
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("could not read Codex Hook input")?;
    match serde_json::from_str::<PromptHookEvent>(&input) {
        Ok(event) => {
            if record_prompt_context(&paths, &event).is_err() {
                diagnostics::record(&paths, "could not record Codex task context");
            }
        }
        Err(_) => diagnostics::record(&paths, "Codex UserPromptSubmit Hook received invalid JSON"),
    }
    println!("{{}}");
    Ok(())
}

fn configured(paths: &AppPaths) -> Result<AppConfig> {
    AppConfig::load(paths)?.context("codex-notify is not configured; run codex-notify init")
}

fn input_value(value: Option<String>, default: Option<&str>, label: &str) -> Result<String> {
    let value = match value {
        Some(value) => value,
        None => {
            let mut input = Input::<String>::new().with_prompt(label);
            if let Some(default) = default.filter(|value| !value.trim().is_empty()) {
                input = input.default(default.to_owned());
            }
            input.interact_text().context("could not read input")?
        }
    };
    if value.trim().is_empty() {
        bail!("{label} must not be empty");
    }
    Ok(value.trim().to_owned())
}

fn secret_value(value: Option<String>) -> Result<String> {
    let value = match value {
        Some(value) => value,
        None => Password::new()
            .with_prompt("Feishu App Secret")
            .interact()
            .context("could not read App Secret")?,
    };
    if value.trim().is_empty() {
        bail!("Feishu App Secret must not be empty");
    }
    Ok(value)
}

fn restore_feishu_secret(
    secrets: &KeyringSecretStore,
    config: &FeishuConfig,
    previous_secret: Option<&str>,
) {
    match previous_secret {
        Some(secret) => {
            let _ = secrets.set_feishu_secret(config, secret);
        }
        None => {
            let _ = secrets.delete_feishu_secret(config);
        }
    }
}

fn receiver_type_value(
    value: Option<ReceiverIdType>,
    default: Option<ReceiverIdType>,
) -> Result<ReceiverIdType> {
    if let Some(value) = value {
        return Ok(value);
    }

    let default = default.unwrap_or(ReceiverIdType::OpenId);
    let input = Input::<String>::new()
        .with_prompt("Feishu receiver ID type (open_id, user_id, email, chat_id)")
        .default(default.as_api_value().to_owned())
        .interact_text()
        .context("could not read receiver ID type")?;
    match input.trim() {
        "open_id" => Ok(ReceiverIdType::OpenId),
        "user_id" => Ok(ReceiverIdType::UserId),
        "email" => Ok(ReceiverIdType::Email),
        "chat_id" => Ok(ReceiverIdType::ChatId),
        _ => bail!("receiver ID type must be open_id, user_id, email, or chat_id"),
    }
}

fn resolve_binary(value: Option<PathBuf>) -> Result<PathBuf> {
    let path = match value {
        Some(path) => path,
        None => std::env::current_exe().context("could not determine the running binary path")?,
    };
    fs::canonicalize(&path)
        .with_context(|| format!("could not resolve binary path {}", path.display()))
}

fn looks_like_feishu_notifier(command: &[String]) -> bool {
    command.iter().any(|argument| {
        let normalized = argument.to_ascii_lowercase();
        normalized.contains("feishu")
            || normalized.contains("lark")
            || normalized.contains("notify_dispatch")
    })
}

fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
