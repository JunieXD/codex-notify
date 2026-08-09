use anyhow::{Context, Result, bail};
use clap::{Args, Parser, Subcommand};
use dialoguer::{Confirm, Input, Password};
use fs2::FileExt;
use serde_json::json;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::ExitCode;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use crate::codex::{
    CompletionEvent, NotifyReconcileResult, PromptHookEvent, RestoreNotifyResult, StopHookEvent,
    backup_file, completion_notification, has_prompt_hook, has_stop_hook, install_integration,
    notify_integration_placement, reconcile_notify_integration, record_prompt_context,
    remove_completion_state, remove_empty_created_codex_files, remove_prompt_hook,
    remove_stop_hook, restore_notify_command, rollback_integration, run_previous_notifier,
};
use crate::diagnostics;
use crate::feishu::FeishuClient;
use crate::model::Notification;
use crate::monitor;
use crate::notify_config::parse_forward_notify;
use crate::paths::AppPaths;
use crate::platform;
use crate::secrets::{KeyringSecretStore, SecretStore};
use crate::settings::{AppConfig, FeishuConfig, ReceiverIdType, atomic_write, resolved_write_path};

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
    /// Reapply the integration after another tool switches config.toml.
    Sync,
    /// Restore the previous Codex notifier and remove codex-notify integration.
    Uninstall(UninstallArgs),
    /// Run the local terminal-error watcher.
    Watch(WatchArgs),
    #[command(hide = true)]
    Notify {
        /// Marks the self-contained notify command written by codex-notify.
        #[arg(long)]
        managed: bool,
        /// JSON command invoked before the Feishu notification.
        #[arg(long)]
        forward_notify: Option<String>,
        /// The single JSON argument supplied by Codex notify.
        event_json: String,
    },
    #[command(name = "prompt-hook", hide = true)]
    PromptHook,
    #[command(name = "stop-hook", hide = true)]
    StopHook,
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

#[derive(Debug, Args)]
struct WatchArgs {
    /// Scan once and exit instead of running the background loop.
    #[arg(long)]
    once: bool,
}

const CONFIG_RECONCILE_INTERVAL: Duration = Duration::from_secs(2);
const WATCHER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
const WATCHER_SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(100);
const WATCHER_LOCK_FILENAME: &str = "watcher-process.lock";
const WATCHER_STOP_FILENAME: &str = "watcher.stop";

pub fn run() -> ExitCode {
    let cli = Cli::parse();
    let result = match cli.command {
        Commands::Init(arguments) => init(arguments),
        Commands::Test => send_test(),
        Commands::Status(arguments) => status(arguments.json),
        Commands::Doctor(arguments) => doctor(arguments.json),
        Commands::Sync => sync(),
        Commands::Uninstall(arguments) => uninstall(arguments),
        Commands::Watch(arguments) => watch(arguments),
        Commands::Notify {
            managed,
            forward_notify,
            event_json,
        } => notify(event_json, managed, forward_notify),
        Commands::PromptHook => prompt_hook(),
        Commands::StopHook => stop_hook(),
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
    let original_app_config = read_optional_file(&paths.config)?;
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
    println!("  - {}", platform::watcher_location()?);
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
    if let Err(error) = clear_watcher_stop_request(&paths) {
        let _ = rollback_integration(&paths, &setup);
        let _ = restore_optional_file(&paths.config, original_app_config.as_deref());
        restore_feishu_secret(&secrets, &feishu, previous_secret.as_deref());
        return Err(error);
    }
    if let Err(error) = platform::install_watcher(&paths, &binary) {
        let _ = rollback_integration(&paths, &setup);
        let _ = restore_optional_file(&paths.config, original_app_config.as_deref());
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
    println!("Open /hooks in Codex and trust the new Stop hook before use.");

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

fn sync() -> Result<()> {
    let paths = AppPaths::discover()?;
    let mut config = configured(&paths)?;
    let result = reconcile_active_config(&paths, &mut config)?;
    if result.changed {
        println!(
            "Codex notify integration was synchronized ({}).",
            result.placement.as_str()
        );
        if let Some(path) = result.config_backup {
            println!("Config backup: {}", path.display());
        }
    } else {
        println!(
            "Codex notify integration is already synchronized ({}).",
            result.placement.as_str()
        );
    }
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
    let (notifier_status, notifier_mode) = match config {
        Some(config) => {
            match notify_integration_placement(&paths.codex_config(), &config.installation) {
                Ok(Some(placement)) => (true, placement.as_str()),
                Ok(None) => (false, "detached"),
                Err(_) => (false, "malformed"),
            }
        }
        None => (false, "not_configured"),
    };
    let hook_status = has_prompt_hook(&paths.codex_hooks())?;
    let stop_hook_status = has_stop_hook(&paths.codex_hooks())?;
    let watcher_status = platform::is_watcher_installed(paths)?;

    Ok(json!({
        "configured": config.is_some(),
        "app_data_directory": paths.root,
        "codex_home": paths.codex_home,
        "credential_store": secret_status,
        "codex_notify_installed": notifier_status,
        "notify_integration": notifier_mode,
        "prompt_hook_installed": hook_status,
        "stop_hook_installed": stop_hook_status,
        "background_watcher_installed": watcher_status,
        "watch_interval_seconds": monitor::WATCH_INTERVAL.as_secs(),
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
    let notifier_mode = data["notify_integration"].as_str().unwrap_or("unknown");
    let hook = data["prompt_hook_installed"].as_bool().unwrap_or(false);
    let stop_hook = data["stop_hook_installed"].as_bool().unwrap_or(false);
    let watcher = data["background_watcher_installed"]
        .as_bool()
        .unwrap_or(false);
    println!("Configured: {}", yes_no(configured));
    println!("Credential store: {secret}");
    println!(
        "Codex notify dispatcher: {} ({notifier_mode})",
        yes_no(notifier)
    );
    println!("UserPromptSubmit hook: {}", yes_no(hook));
    println!("Stop hook: {}", yes_no(stop_hook));
    println!("Background watcher: {}", yes_no(watcher));
    if include_guidance && configured && !notifier {
        match notifier_mode {
            "detached" => {
                println!("Notify repair: run codex-notify sync for the active config.toml.")
            }
            "malformed" => println!(
                "Notify repair: the active notify chain is malformed; inspect config.toml before retrying."
            ),
            _ => {}
        }
    }
    if include_guidance && hook {
        println!("Hook trust: inspect and trust both hooks with /hooks in Codex.");
        println!(
            "Error detection is best-effort because Codex transcripts are not a stable Hook API."
        );
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
    platform::uninstall_watcher(&paths)?;
    request_watcher_stop(&paths)?;
    wait_for_watcher_exit(&paths)?;

    let mut config_paths = config
        .installation
        .managed_config_paths
        .iter()
        .map(PathBuf::from)
        .collect::<Vec<_>>();
    config_paths.push(config_path.clone());
    config_paths.push(paths.codex_config());
    let mut resolved_paths = Vec::new();
    for path in config_paths {
        let resolved = resolved_write_path(&path)?;
        if !resolved_paths.contains(&resolved) {
            resolved_paths.push(resolved);
        }
    }

    let mut restored_configs = 0usize;
    for (index, path) in resolved_paths.iter().enumerate() {
        if !path.exists() {
            continue;
        }
        let _ = backup_file(&paths, path, &format!("uninstall-config-{index}"))?;
        if restore_notify_command(path, &config.installation)? == RestoreNotifyResult::Restored {
            restored_configs += 1;
        }
    }
    let _ = backup_file(&paths, &hooks_path, "uninstall-hooks")?;
    let _ = remove_prompt_hook(&hooks_path)?;
    let _ = remove_stop_hook(&hooks_path)?;
    remove_empty_created_codex_files(&config_path, &hooks_path, &config.installation)?;
    let _ = KeyringSecretStore.delete_feishu_secret(&config.feishu);
    let _ = fs::remove_file(&paths.config);
    if paths.state.exists() {
        remove_directory_tree(&paths.state)?;
    }
    println!(
        "codex-notify integration was removed; restored {restored_configs} managed Codex config file(s)."
    );
    Ok(())
}

fn notify(event_json: String, managed: bool, forward_notify: Option<String>) -> Result<()> {
    let paths = AppPaths::discover()?;
    let config = AppConfig::load(&paths)?;
    let embedded_previous = forward_notify
        .as_deref()
        .map(parse_forward_notify)
        .transpose()?;
    let previous = if managed {
        embedded_previous.as_ref()
    } else {
        embedded_previous.as_ref().or_else(|| {
            config
                .as_ref()
                .and_then(|config| config.installation.previous_notify.as_ref())
        })
    };
    if let Some(previous) = previous
        && run_previous_notifier(previous, &event_json).is_err()
    {
        diagnostics::record(&paths, "previous Codex notifier failed");
    }

    let Some(config) = config else {
        diagnostics::record(
            &paths,
            "notify skipped because codex-notify is not configured",
        );
        return Ok(());
    };

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
    if monitor::mark_turn_completed(&paths, &event.turn_id).is_err() {
        diagnostics::record(
            &paths,
            "could not cancel pending interruption after normal completion",
        );
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

fn stop_hook() -> Result<()> {
    let paths = AppPaths::discover()?;
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("could not read Codex Stop Hook input")?;
    match serde_json::from_str::<StopHookEvent>(&input) {
        Ok(event) if event.last_assistant_message.trim().is_empty() => {
            let transcript_path = (!event.transcript_path.trim().is_empty())
                .then(|| PathBuf::from(event.transcript_path));
            if monitor::record_stop_fallback(
                &paths,
                &event.turn_id,
                &event.session_id,
                &event.cwd,
                transcript_path.as_deref(),
                SystemTime::now(),
            )
            .is_err()
            {
                diagnostics::record(&paths, "could not record Codex Stop fallback");
            }
        }
        Ok(_) => {}
        Err(_) => diagnostics::record(&paths, "Codex Stop Hook received invalid JSON"),
    }
    println!("{{}}");
    Ok(())
}

fn watch(arguments: WatchArgs) -> Result<()> {
    let paths = AppPaths::discover()?;
    let mut config = configured(&paths)?;
    let _watcher_lease = (!arguments.once)
        .then(|| acquire_watcher_lease(&paths))
        .transpose()?;
    let mut next_monitor_scan = Instant::now();
    loop {
        if !arguments.once {
            if watcher_stop_requested(&paths) {
                return Ok(());
            }
            let Some(latest_config) = AppConfig::load(&paths)? else {
                return Ok(());
            };
            config = latest_config;
        }
        if let Err(error) = reconcile_active_config(&paths, &mut config) {
            if arguments.once {
                return Err(error);
            }
            diagnostics::record(
                &paths,
                &format!("notify integration reconciliation failed: {error:#}"),
            );
        }
        if arguments.once || Instant::now() >= next_monitor_scan {
            match watch_once(&paths, &config) {
                Ok(delivered) if arguments.once => {
                    println!(
                        "Watcher scan completed; delivered {delivered} interruption notification(s)."
                    );
                    return Ok(());
                }
                Ok(_) => {}
                Err(error) if arguments.once => return Err(error),
                Err(error) => {
                    diagnostics::record(&paths, &format!("watcher scan failed: {error:#}"))
                }
            }
            next_monitor_scan = Instant::now() + monitor::WATCH_INTERVAL;
        }
        thread::sleep(CONFIG_RECONCILE_INTERVAL);
    }
}

fn reconcile_active_config(
    paths: &AppPaths,
    config: &mut AppConfig,
) -> Result<NotifyReconcileResult> {
    let result = reconcile_notify_integration(paths, &config.installation)?;
    let mut metadata_changed = false;

    if config.installation.managed_notify != result.managed_notify {
        if let Some(program) = config.installation.managed_notify.first()
            && !config.installation.managed_binary_paths.contains(program)
        {
            config
                .installation
                .managed_binary_paths
                .push(program.clone());
        }
        config.installation.managed_notify = result.managed_notify.clone();
        metadata_changed = true;
    }
    if result.legacy_owned_before && config.installation.previous_notify != result.previous_notify {
        config.installation.previous_notify = result.previous_notify.clone();
        metadata_changed = true;
    }
    let managed_path = result.managed_config_path.to_string_lossy().into_owned();
    if !config
        .installation
        .managed_config_paths
        .contains(&managed_path)
    {
        config.installation.managed_config_paths.push(managed_path);
        metadata_changed = true;
    }
    if let Some(program) = result.managed_notify.first()
        && !config.installation.managed_binary_paths.contains(program)
    {
        config
            .installation
            .managed_binary_paths
            .push(program.clone());
        metadata_changed = true;
    }
    if metadata_changed {
        config.save(paths)?;
    }
    Ok(result)
}

fn watch_once(paths: &AppPaths, config: &AppConfig) -> Result<usize> {
    let (_, deliveries) = monitor::prepare_notifications(paths, SystemTime::now())?;
    if deliveries.is_empty() {
        return Ok(0);
    }
    let secret = match KeyringSecretStore.get_feishu_secret(&config.feishu) {
        Ok(secret) => secret,
        Err(error) => {
            for delivery in deliveries {
                let _ = monitor::settle_delivery(paths, &delivery.key, false);
            }
            return Err(error);
        }
    };
    let client = match FeishuClient::new() {
        Ok(client) => client,
        Err(error) => {
            for delivery in deliveries {
                let _ = monitor::settle_delivery(paths, &delivery.key, false);
            }
            return Err(error);
        }
    };

    let mut delivered = 0;
    for delivery in deliveries {
        match client.send(&config.feishu, &secret, &delivery.notification) {
            Ok(_) => {
                monitor::settle_delivery(paths, &delivery.key, true)?;
                delivered += 1;
            }
            Err(error) => {
                let _ = monitor::settle_delivery(paths, &delivery.key, false);
                diagnostics::record(
                    paths,
                    &format!("Feishu interruption notification failed: {error:#}"),
                );
            }
        }
    }
    Ok(delivered)
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

fn read_optional_file(path: &std::path::Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("could not read {}", path.display())),
    }
}

fn restore_optional_file(path: &std::path::Path, contents: Option<&[u8]>) -> Result<()> {
    match contents {
        Some(contents) => atomic_write(path, contents),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => {
                Err(error).with_context(|| format!("could not remove {}", path.display()))
            }
        },
    }
}

fn watcher_lock_path(paths: &AppPaths) -> PathBuf {
    paths.state.join(WATCHER_LOCK_FILENAME)
}

fn watcher_stop_path(paths: &AppPaths) -> PathBuf {
    paths.state.join(WATCHER_STOP_FILENAME)
}

fn acquire_watcher_lease(paths: &AppPaths) -> Result<File> {
    paths.ensure_directories()?;
    let path = watcher_lock_path(paths);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("could not open {}", path.display()))?;
    file.try_lock_exclusive().with_context(|| {
        format!(
            "another codex-notify watcher is already using {}",
            path.display()
        )
    })?;
    Ok(file)
}

fn watcher_stop_requested(paths: &AppPaths) -> bool {
    watcher_stop_path(paths).exists()
}

fn clear_watcher_stop_request(paths: &AppPaths) -> Result<()> {
    let path = watcher_stop_path(paths);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("could not remove {}", path.display())),
    }
}

fn request_watcher_stop(paths: &AppPaths) -> Result<()> {
    paths.ensure_directories()?;
    let path = watcher_stop_path(paths);
    atomic_write(&path, b"stop\n").with_context(|| {
        format!(
            "could not request watcher shutdown through {}",
            path.display()
        )
    })
}

fn is_lock_contended(error: &std::io::Error) -> bool {
    if error.kind() == std::io::ErrorKind::WouldBlock {
        return true;
    }

    #[cfg(windows)]
    {
        // `fs2` uses `LockFileEx` on Windows. A competing lock is reported as
        // `ERROR_LOCK_VIOLATION` instead of `ErrorKind::WouldBlock`.
        error.raw_os_error() == Some(33)
    }

    #[cfg(not(windows))]
    {
        false
    }
}

fn wait_for_watcher_exit(paths: &AppPaths) -> Result<()> {
    let deadline = Instant::now() + WATCHER_SHUTDOWN_TIMEOUT;
    let path = watcher_lock_path(paths);
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("could not open {}", path.display()))?;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => {
                let _ = FileExt::unlock(&file);
                return Ok(());
            }
            Err(error) if is_lock_contended(&error) => {
                if Instant::now() >= deadline {
                    bail!(
                        "the background watcher did not stop within {} seconds; retry uninstall",
                        WATCHER_SHUTDOWN_TIMEOUT.as_secs()
                    );
                }
                thread::sleep(WATCHER_SHUTDOWN_POLL_INTERVAL);
            }
            Err(error) => {
                return Err(error).with_context(|| format!("could not lock {}", path.display()));
            }
        }
    }
}

/// Remove an app-owned directory without relying on `remove_dir_all`.
///
/// Some Windows shared-folder drivers reject the recursive directory handle
/// flags used by `std::fs::remove_dir_all` with `ERROR_INVALID_PARAMETER`.
/// Walking the tree and removing each entry works on those filesystems while
/// retaining the same symlink-safe behavior for this app-owned state tree.
fn remove_directory_tree(path: &std::path::Path) -> Result<()> {
    for entry in fs::read_dir(path).with_context(|| format!("could not read {}", path.display()))? {
        let entry =
            entry.with_context(|| format!("could not read an entry in {}", path.display()))?;
        let entry_path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("could not inspect {}", entry_path.display()))?;
        if file_type.is_dir() && !file_type.is_symlink() {
            remove_directory_tree(&entry_path)?;
        } else {
            fs::remove_file(&entry_path)
                .with_context(|| format!("could not remove {}", entry_path.display()))?;
        }
    }
    fs::remove_dir(path).with_context(|| format!("could not remove {}", path.display()))
}

#[cfg(test)]
mod cli_tests {
    use super::{
        acquire_watcher_lease, remove_directory_tree, request_watcher_stop, wait_for_watcher_exit,
        watcher_stop_requested,
    };
    use crate::paths::AppPaths;
    use std::fs;
    use std::thread;
    use std::time::Duration;
    use tempfile::tempdir;

    fn paths() -> (tempfile::TempDir, tempfile::TempDir, AppPaths) {
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
        (app_home, codex_home, paths)
    }

    #[test]
    fn portable_directory_cleanup_removes_nested_state() {
        let parent = tempdir().expect("temporary parent");
        let state = parent.path().join("state");
        let nested = state.join("nested");
        fs::create_dir_all(&nested).expect("create nested state");
        fs::write(state.join("monitor.lock"), []).expect("write lock");
        fs::write(nested.join("turn.json"), b"{}").expect("write nested state");

        remove_directory_tree(&state).expect("remove state tree");

        assert!(!state.exists());
    }

    #[test]
    fn uninstall_handshake_waits_until_the_watcher_releases_its_lease() {
        let (_app_home, _codex_home, paths) = paths();
        let lease = acquire_watcher_lease(&paths).expect("watcher lease");
        let watcher_paths = paths.clone();
        let watcher = thread::spawn(move || {
            while !watcher_stop_requested(&watcher_paths) {
                thread::sleep(Duration::from_millis(10));
            }
            drop(lease);
        });

        request_watcher_stop(&paths).expect("request stop");
        wait_for_watcher_exit(&paths).expect("wait for watcher");
        watcher.join().expect("join watcher");
    }
}
