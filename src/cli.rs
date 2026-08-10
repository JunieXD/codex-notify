use anyhow::{Context, Result, bail};
use clap::error::{ContextKind, ErrorKind};
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use dialoguer::{Input, Password, Select, theme::ColorfulTheme};
use fs2::FileExt;
use serde_json::json;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
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
use crate::updater::{
    self, DEFAULT_REPOSITORY, PreparedRelease, executable_version, install_executable,
    remove_executable, replace_current_executable,
};

#[derive(Debug, Parser)]
#[command(name = "codex-notify", version, about = "为 Codex 提供本地飞书通知。")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Debug, Subcommand)]
enum Commands {
    /// 配置飞书通知并安全接入 Codex。
    Init(InitArgs),
    /// 使用当前配置发送一条飞书测试通知。
    Test,
    /// 查看本机配置和运行状态，不会显示密钥。
    Status(JsonOutput),
    /// 检查 Codex 与飞书配置并给出处理建议。
    Doctor(JsonOutput),
    /// 在其他工具切换 config.toml 后重新接入通知。
    Sync,
    /// 检查更新或安全升级到新版本。
    Update(UpdateArgs),
    /// 恢复原有 Codex 通知命令并移除 codex-notify。
    Uninstall(UninstallArgs),
    /// 运行本地任务异常监听器。
    Watch(WatchArgs),
    #[command(hide = true)]
    Notify {
        /// 标记由 codex-notify 写入的独立通知命令。
        #[arg(long)]
        managed: bool,
        /// 发送飞书通知前调用的 JSON 命令。
        #[arg(long)]
        forward_notify: Option<String>,
        /// Codex notify 传入的单个 JSON 参数。
        event_json: String,
    },
    #[command(name = "prompt-hook", hide = true)]
    PromptHook,
    #[command(name = "stop-hook", hide = true)]
    StopHook,
    #[command(name = "update-finalize", hide = true)]
    UpdateFinalize(UpdateFinalizeArgs),
    #[command(name = "install-prepared", hide = true)]
    InstallPrepared(InstallPreparedArgs),
}

#[derive(Debug, Args)]
struct InitArgs {
    /// 飞书 App ID；未提供时会交互询问。
    #[arg(long, value_name = "APP_ID")]
    app_id: Option<String>,
    /// 飞书 App Secret；直接写在命令中可能被终端历史记录保存。
    #[arg(long, value_name = "APP_SECRET")]
    app_secret: Option<String>,
    /// 飞书接收者 ID 类型：open_id、user_id、email 或 chat_id。
    #[arg(long, value_enum, hide_possible_values = true, value_name = "接收类型")]
    receiver_id_type: Option<ReceiverIdType>,
    /// 飞书接收者 ID。
    #[arg(long, value_name = "接收者_ID")]
    receiver_id: Option<String>,
    /// 供 Codex 调用的程序路径；默认使用当前程序。
    #[arg(long, value_name = "程序路径")]
    binary: Option<PathBuf>,
    /// 跳过确认提示。
    #[arg(short, long)]
    yes: bool,
    /// 配置完成后不发送飞书测试通知。
    #[arg(long)]
    skip_test: bool,
}

#[derive(Debug, Args)]
struct JsonOutput {
    /// 输出便于程序读取的 JSON。
    #[arg(long)]
    json: bool,
}

#[derive(Debug, Args)]
struct UninstallArgs {
    /// 跳过确认提示。
    #[arg(short, long)]
    yes: bool,
}

#[derive(Debug, Args)]
struct WatchArgs {
    /// 只检查一次并退出，不持续后台运行。
    #[arg(long)]
    once: bool,
}

#[derive(Debug, Args)]
struct UpdateArgs {
    /// 只检查是否有新版本，不执行升级。
    #[arg(long)]
    check: bool,
    /// 安装指定版本，例如 v0.4.0。
    #[arg(long, value_name = "版本")]
    version: Option<String>,
    /// 下载发行版的 GitHub 仓库；默认使用 JunieXD/codex-notify。
    #[arg(
        long,
        default_value = DEFAULT_REPOSITORY,
        hide_default_value = true,
        value_name = "仓库"
    )]
    repository: String,
    /// 允许重新安装当前版本或降级。
    #[arg(long)]
    force: bool,
    /// 跳过确认提示。
    #[arg(short, long)]
    yes: bool,
    /// 为端到端测试指定发行版下载目录。
    #[arg(long, hide = true)]
    download_base: Option<String>,
    /// 为回滚测试模拟替换后的失败。
    #[arg(long, hide = true)]
    fail_finalize_for_test: bool,
}

#[derive(Debug, Args)]
struct UpdateFinalizeArgs {
    #[arg(long)]
    binary: PathBuf,
    #[arg(long)]
    restart_watcher: bool,
    #[arg(long, hide = true)]
    fail_for_test: bool,
}

#[derive(Debug, Args)]
struct InstallPreparedArgs {
    #[arg(long)]
    target: PathBuf,
    #[arg(long)]
    expected_version: Option<String>,
    #[arg(long)]
    force: bool,
    #[arg(long, hide = true)]
    fail_finalize_for_test: bool,
}

const CONFIG_RECONCILE_INTERVAL: Duration = Duration::from_secs(2);
const WATCHER_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(15);
const WATCHER_SHUTDOWN_POLL_INTERVAL: Duration = Duration::from_millis(100);
const WATCHER_LOCK_FILENAME: &str = "watcher-process.lock";
const WATCHER_STOP_FILENAME: &str = "watcher.stop";
const UPDATE_LOCK_FILENAME: &str = ".codex-notify-update.lock";
const ROOT_HELP_TEMPLATE: &str = "{before-help}{about-with-newline}\n用法：{usage}\n\n命令：\n{subcommands}\n选项：\n{options}{after-help}";
const COMMAND_HELP_TEMPLATE: &str =
    "{before-help}{about-with-newline}\n用法：{usage}\n\n选项：\n{options}{after-help}";
const HOOK_TRUST_GUIDANCE: &str = "还差一步：信任两个用户 Hook\n\
如果你使用 ChatGPT App（原 Codex App）：\n\
  1. 打开“设置”，进入“钩子”。\n\
  2. 在“用户”区域找到 UserPromptSubmit 和 Stop，分别设为“信任”。\n\
如果你使用 Codex CLI：运行 /hooks，然后信任这两个 Hook。";

pub fn run() -> ExitCode {
    let cli = parse_cli();
    let result = match cli.command {
        Commands::Init(arguments) => init(arguments),
        Commands::Test => send_test(),
        Commands::Status(arguments) => status(arguments.json),
        Commands::Doctor(arguments) => doctor(arguments.json),
        Commands::Sync => sync(),
        Commands::Update(arguments) => update(arguments),
        Commands::Uninstall(arguments) => uninstall(arguments),
        Commands::Watch(arguments) => watch(arguments),
        Commands::Notify {
            managed,
            forward_notify,
            event_json,
        } => notify(event_json, managed, forward_notify),
        Commands::PromptHook => prompt_hook(),
        Commands::StopHook => stop_hook(),
        Commands::UpdateFinalize(arguments) => update_finalize(arguments),
        Commands::InstallPrepared(arguments) => install_prepared(arguments),
    };

    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("codex-notify：操作失败：{error:#}");
            ExitCode::from(1)
        }
    }
}

fn parse_cli() -> Cli {
    let command = localized_cli_command();
    let matches = match command.try_get_matches() {
        Ok(matches) => matches,
        Err(error)
            if matches!(
                error.kind(),
                ErrorKind::DisplayHelp
                    | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
                    | ErrorKind::DisplayVersion
            ) =>
        {
            error.exit()
        }
        Err(error) => exit_with_localized_argument_error(&error),
    };
    Cli::from_arg_matches(&matches).unwrap_or_else(|error| error.exit())
}

fn localized_cli_command() -> clap::Command {
    let mut command = Cli::command();
    localize_help(&mut command, true);
    command
}

fn exit_with_localized_argument_error(error: &clap::Error) -> ! {
    let invalid_subcommand = argument_error_value(error, ContextKind::InvalidSubcommand);
    let invalid_argument = argument_error_value(error, ContextKind::InvalidArg);
    let invalid_value = argument_error_value(error, ContextKind::InvalidValue);
    let message = match error.kind() {
        ErrorKind::UnknownArgument | ErrorKind::InvalidSubcommand => {
            if let Some(value) = invalid_subcommand {
                format!("无法识别命令“{value}”")
            } else if let Some(value) = invalid_argument {
                format!("无法识别参数“{value}”")
            } else {
                "包含无法识别的命令或参数".to_owned()
            }
        }
        ErrorKind::InvalidValue | ErrorKind::ValueValidation => match invalid_value {
            Some(value) => format!("“{value}”不是有效值"),
            None => "参数值无效".to_owned(),
        },
        ErrorKind::NoEquals => "这个选项需要使用“=”连接参数值".to_owned(),
        ErrorKind::TooManyValues => "提供的参数值过多".to_owned(),
        ErrorKind::TooFewValues => "提供的参数值不足".to_owned(),
        ErrorKind::WrongNumberOfValues => "参数值数量不正确".to_owned(),
        ErrorKind::ArgumentConflict => "部分参数不能同时使用".to_owned(),
        ErrorKind::MissingRequiredArgument => "缺少必填参数".to_owned(),
        ErrorKind::MissingSubcommand => "缺少要执行的命令".to_owned(),
        ErrorKind::Io | ErrorKind::Format => "无法读取命令行参数".to_owned(),
        _ => "命令行参数有误".to_owned(),
    };

    eprintln!("codex-notify：{message}。");
    if let Some(suggestion) = [
        ContextKind::SuggestedSubcommand,
        ContextKind::SuggestedArg,
        ContextKind::SuggestedValue,
        ContextKind::SuggestedCommand,
    ]
    .into_iter()
    .find_map(|kind| argument_error_value(error, kind))
    {
        eprintln!("你可能想输入：{suggestion}");
    }
    if let Some(values) = argument_error_value(error, ContextKind::ValidValue) {
        eprintln!("可选值：{}", values.replace(", ", "、"));
    }
    eprintln!("请运行 codex-notify --help 查看完整帮助。");
    std::process::exit(error.exit_code());
}

fn argument_error_value(error: &clap::Error, kind: ContextKind) -> Option<String> {
    error
        .get(kind)
        .map(ToString::to_string)
        .filter(|value| !value.trim().is_empty())
}

fn localize_help(command: &mut clap::Command, root: bool) {
    *command = command.clone().disable_help_subcommand(true);
    command.build();
    let template = if root {
        ROOT_HELP_TEMPLATE
    } else {
        COMMAND_HELP_TEMPLATE
    };
    let has_help = command
        .get_arguments()
        .any(|argument| matches!(argument.get_action(), clap::ArgAction::Help));
    let has_version = command
        .get_arguments()
        .any(|argument| matches!(argument.get_action(), clap::ArgAction::Version));
    let mut localized = command.clone().help_template(template);
    if let Some(usage) = localized_usage(command.get_name()) {
        localized = localized.override_usage(usage);
    }
    if has_help {
        localized = localized.mut_arg("help", |argument| argument.help("显示帮助信息"));
    }
    if has_version {
        localized = localized.mut_arg("version", |argument| argument.help("显示版本信息"));
    }
    *command = localized;
    for subcommand in command.get_subcommands_mut() {
        localize_help(subcommand, false);
    }
}

fn localized_usage(command_name: &str) -> Option<&'static str> {
    match command_name {
        "codex-notify" => Some("codex-notify <命令>"),
        "init" => Some("codex-notify init [选项]"),
        "test" => Some("codex-notify test"),
        "status" => Some("codex-notify status [选项]"),
        "doctor" => Some("codex-notify doctor [选项]"),
        "sync" => Some("codex-notify sync"),
        "update" => Some("codex-notify update [选项]"),
        "uninstall" => Some("codex-notify uninstall [选项]"),
        "watch" => Some("codex-notify watch [选项]"),
        _ => None,
    }
}

fn init(arguments: InitArgs) -> Result<()> {
    let paths = AppPaths::discover()?;
    let existing = AppConfig::load(&paths)?;
    let reconfiguring = existing.is_some();
    let original_app_config = read_optional_file(&paths.config)?;
    let theme = ColorfulTheme::default();
    let interactive = arguments.app_id.is_none()
        || arguments.app_secret.is_none()
        || arguments.receiver_id_type.is_none()
        || arguments.receiver_id.is_none();
    if interactive {
        if let Some(config) = existing.as_ref() {
            print_existing_configuration(&paths, config);
            if !arguments.yes
                && !choose_action(
                    &theme,
                    "请选择接下来的操作",
                    "重新配置",
                    "保留当前配置",
                    false,
                )?
            {
                println!("已保留当前配置，没有修改任何内容。");
                println!("如需检查运行状态，请执行 codex-notify doctor。");
                return Ok(());
            }
        }
        print_init_intro(reconfiguring);
    }
    if arguments.app_id.is_none() {
        print_app_id_help();
    }
    let app_id = input_value(
        arguments.app_id,
        existing
            .as_ref()
            .map(|config| config.feishu.app_id.as_str()),
        "请输入飞书 App ID",
        "App ID",
        validate_app_id,
        &theme,
    )?;
    let app_secret = secret_value(arguments.app_secret, &theme)?;
    let receiver_id_type = receiver_type_value(
        arguments.receiver_id_type,
        existing
            .as_ref()
            .map(|config| config.feishu.receiver_id_type),
        &theme,
    )?;
    let receiver_default = existing
        .as_ref()
        .filter(|config| config.feishu.receiver_id_type == receiver_id_type)
        .map(|config| config.feishu.receiver_id.as_str());
    if arguments.receiver_id.is_none() {
        print_receiver_help(receiver_id_type);
    }
    let receiver_id = input_value(
        arguments.receiver_id,
        receiver_default,
        receiver_prompt(receiver_id_type),
        "接收者",
        |value| validate_receiver_id(receiver_id_type, value),
        &theme,
    )?;
    println!("\n[4/4] 确认配置");
    println!("  App ID：{app_id}");
    println!(
        "  接收方式：{}（{}）",
        receiver_type_name(receiver_id_type),
        receiver_id_type.as_api_value()
    );
    println!("  接收者：{receiver_id}");
    println!("\n确认后将更新：");
    println!("  - Codex 通知配置：{}", paths.codex_config().display());
    println!("  - Codex Hook 配置：{}", paths.codex_hooks().display());
    println!("  - codex-notify 配置：{}", paths.config.display());
    println!("  - 后台监听：{}", platform::watcher_location()?);
    if reconfiguring {
        println!("当前飞书设置和 App Secret 会被新内容替换。");
    }
    println!("现有 notify 命令和其他 Hook 会保留；相关配置修改前会自动备份。");
    if !arguments.yes
        && !choose_action(
            &theme,
            "请选择接下来的操作",
            "写入配置并启动监听",
            "取消，不做修改",
            true,
        )?
    {
        println!("已取消，没有修改任何配置。");
        return Ok(());
    }

    let binary = resolve_binary(arguments.binary)?;
    let app_config_backup = if reconfiguring {
        backup_file(&paths, &paths.config, "codex-notify-config")?
    } else {
        None
    };
    let feishu = FeishuConfig {
        app_id,
        receiver_id_type,
        receiver_id,
    };
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
        eprintln!("提示：保留的原有通知程序似乎也会发送飞书消息，停用它之前可能收到重复提醒。");
    }
    if reconfiguring {
        println!("\n重新配置完成，codex-notify 已更新并重新接入 Codex。");
    } else {
        println!("\n配置完成，codex-notify 已接入 Codex。");
    }
    if let Some(path) = app_config_backup {
        println!("原 codex-notify 配置备份：{}", path.display());
    }
    if let Some(path) = setup.config_backup {
        println!("Codex 配置备份：{}", path.display());
    }
    if let Some(path) = setup.hooks_backup {
        println!("Hook 配置备份：{}", path.display());
    }
    println!("\n{HOOK_TRUST_GUIDANCE}");

    let should_test = !arguments.skip_test
        && (arguments.yes
            || choose_action(
                &theme,
                "是否发送一条飞书测试通知？",
                "发送测试通知",
                "暂不测试",
                true,
            )?);
    if should_test {
        match send_test_for(&config) {
            Ok(()) => println!("飞书测试通知已发送。"),
            Err(error) => {
                eprintln!(
                    "配置已经保存，但测试通知发送失败：{error:#}\n请检查飞书应用权限，然后运行 codex-notify doctor。"
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
    println!("飞书测试通知已发送，请检查接收端。");
    Ok(())
}

fn send_test_for(config: &AppConfig) -> Result<()> {
    let secret = KeyringSecretStore.get_feishu_secret(&config.feishu)?;
    let notification = Notification::completed(
        "codex-notify 测试通知",
        "检查飞书通知是否可以正常送达",
        "飞书连接、系统凭据和通知卡片均工作正常。",
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
            "Codex 通知接入已同步（{}）。",
            notify_placement_name(result.placement.as_str())
        );
        if let Some(path) = result.config_backup {
            println!("配置备份：{}", path.display());
        }
    } else {
        println!(
            "Codex 通知接入已经是最新状态（{}），无需修改。",
            notify_placement_name(result.placement.as_str())
        );
    }
    Ok(())
}

fn update(arguments: UpdateArgs) -> Result<()> {
    let current = updater::current_version()?;
    println!("当前版本：v{current}");
    println!("正在检查更新……");
    let release = updater::resolve_release(
        &arguments.repository,
        arguments.version.as_deref(),
        arguments.download_base.as_deref(),
    )?;
    let needed = updater::update_needed(&current, &release.version, arguments.force)?;

    if !needed {
        println!("你已经在使用最新版（v{current}）。");
        return Ok(());
    }
    if arguments.check {
        println!("发现新版本：v{current} → {}", release.tag);
        return Ok(());
    }
    let theme = ColorfulTheme::default();
    if !arguments.yes
        && !choose_action(
            &theme,
            &format!("发现 {}，请选择接下来的操作", release.tag),
            "立即升级",
            "暂不升级",
            true,
        )?
    {
        println!("已取消，当前版本没有变化。");
        return Ok(());
    }

    let paths = AppPaths::discover()?;
    let current_executable = resolve_binary(None)?;
    let _update_lease = acquire_update_lease(&current_executable)?;
    let staging_parent = current_executable
        .parent()
        .context("无法确定当前程序所在目录")?;
    println!("正在下载并校验 {}……", release.tag);
    let prepared = updater::prepare_release(release, staging_parent)?;
    let target_tag = prepared.info.tag.clone();
    apply_self_update(
        &paths,
        &current_executable,
        prepared,
        arguments.fail_finalize_for_test,
    )?;
    println!("升级完成：v{current} → {target_tag}。");
    Ok(())
}

fn apply_self_update(
    paths: &AppPaths,
    current_executable: &Path,
    prepared: PreparedRelease,
    fail_finalize_for_test: bool,
) -> Result<()> {
    let restart_watcher = platform::is_watcher_installed(paths)?;
    if let Err(error) = stop_watcher_for_update(paths) {
        return Err(with_recovery(
            error,
            resume_watcher_after_update(paths, current_executable, restart_watcher),
        ));
    }

    let backup = match prepared.backup_current_executable(current_executable) {
        Ok(backup) => backup,
        Err(error) => {
            return Err(with_recovery(
                error,
                resume_watcher_after_update(paths, current_executable, restart_watcher),
            ));
        }
    };
    if let Err(error) = replace_current_executable(&prepared.executable) {
        return Err(with_recovery(
            error,
            resume_watcher_after_update(paths, current_executable, restart_watcher),
        ));
    }

    let finalize_result =
        run_update_finalize(current_executable, restart_watcher, fail_finalize_for_test);
    if let Err(finalize_error) = finalize_result {
        let executable_recovery =
            install_executable(backup.path(), current_executable).context("无法恢复升级前的程序");
        let watcher_recovery =
            resume_watcher_after_update(paths, current_executable, restart_watcher);
        let recovery = combine_recovery_steps(executable_recovery, watcher_recovery);
        return Err(with_recovery(finalize_error, recovery));
    }
    Ok(())
}

fn install_prepared(arguments: InstallPreparedArgs) -> Result<()> {
    let source = resolve_binary(None)?;
    let source_version = updater::current_version()?;
    if let Some(expected) = arguments.expected_version.as_deref() {
        let expected = updater::parse_version(expected)?;
        if expected != source_version {
            bail!("下载的程序版本是 v{source_version}，但请求安装的是 v{expected}");
        }
    }

    let target = absolute_path(&arguments.target)?;
    if source == target {
        bail!("待安装程序与目标位置是同一个文件，无法继续");
    }
    let installed_version = target
        .exists()
        .then(|| executable_version(&target))
        .transpose();
    match installed_version.as_ref() {
        Ok(Some(installed)) => {
            if !updater::update_needed(installed, &source_version, arguments.force)? {
                println!("你已经在使用最新版（v{installed}）。");
                return Ok(());
            }
            println!("正在升级 codex-notify：v{installed} → v{source_version}……");
        }
        Ok(None) => println!("正在安装 codex-notify v{source_version}……"),
        Err(error) => {
            eprintln!("提示：现有程序无法报告版本，将尝试修复安装：{error:#}");
        }
    }

    let paths = AppPaths::discover()?;
    let target_parent = target.parent().context("无法确定安装目标所在目录")?;
    fs::create_dir_all(target_parent)
        .with_context(|| format!("无法创建目录 {}", target_parent.display()))?;
    let _update_lease = acquire_update_lease(&target)?;
    let existing_config = AppConfig::load(&paths)?;
    // A watcher launcher is user-global on every supported platform. It may
    // belong to another CODEX_NOTIFY_HOME, so it must not turn an otherwise
    // standalone first install into a coordinated upgrade.
    let coordinated_upgrade = target.exists() || existing_config.is_some();
    let restart_watcher = coordinated_upgrade && platform::is_watcher_installed(&paths)?;
    let previous_binary = existing_config
        .as_ref()
        .and_then(|config| config.installation.managed_notify.first())
        .map(PathBuf::from)
        .filter(|path| path.exists())
        .unwrap_or_else(|| target.clone());
    let backup_directory = tempfile::Builder::new()
        .prefix("codex-notify-installer-backup-")
        .tempdir_in(target_parent)
        .with_context(|| format!("无法在 {} 中创建安装回滚目录", target_parent.display()))?;
    let backup = target.exists().then(|| {
        let path = backup_directory.path().join(if cfg!(windows) {
            "codex-notify.previous.exe"
        } else {
            "codex-notify.previous"
        });
        fs::copy(&target, &path).with_context(|| format!("无法备份 {}", target.display()))?;
        Ok::<_, anyhow::Error>(path)
    });
    let backup = backup.transpose()?;

    if coordinated_upgrade {
        if let Err(error) = stop_watcher_for_update(&paths) {
            return Err(with_recovery(
                error,
                resume_watcher_after_update(&paths, &previous_binary, restart_watcher),
            ));
        }
    }
    if let Err(error) = install_executable(&source, &target) {
        let recovery = if coordinated_upgrade {
            resume_watcher_after_update(&paths, &previous_binary, restart_watcher)
        } else {
            Ok(())
        };
        return Err(with_recovery(error, recovery));
    }

    if let Err(error) = executable_version(&target).and_then(|installed| {
        if installed == source_version {
            Ok(())
        } else {
            bail!("安装后的程序报告版本 v{installed}，预期应为 v{source_version}")
        }
    }) {
        let recovery = restore_installer_update(
            &paths,
            &target,
            backup.as_deref(),
            &previous_binary,
            restart_watcher,
            coordinated_upgrade,
        );
        return Err(with_recovery(error, recovery));
    }

    if coordinated_upgrade {
        if let Err(error) =
            run_update_finalize(&target, restart_watcher, arguments.fail_finalize_for_test)
        {
            let recovery = restore_installer_update(
                &paths,
                &target,
                backup.as_deref(),
                &previous_binary,
                restart_watcher,
                true,
            );
            return Err(with_recovery(error, recovery));
        }
        println!("codex-notify 已安全升级到 v{source_version}。");
    } else {
        println!("codex-notify 已安装到 {}", target.display());
    }
    Ok(())
}

fn restore_installer_update(
    paths: &AppPaths,
    target: &Path,
    backup: Option<&Path>,
    previous_binary: &Path,
    restart_watcher: bool,
    coordinated_upgrade: bool,
) -> Result<()> {
    let executable_recovery = match backup {
        Some(backup) => install_executable(backup, target).context("无法恢复安装前的程序"),
        None => remove_executable(target).with_context(|| format!("无法删除 {}", target.display())),
    };
    let watcher_recovery = if coordinated_upgrade {
        resume_watcher_after_update(paths, previous_binary, restart_watcher)
    } else {
        Ok(())
    };
    combine_recovery_steps(executable_recovery, watcher_recovery)
}

fn update_finalize(arguments: UpdateFinalizeArgs) -> Result<()> {
    if arguments.fail_for_test {
        bail!("模拟升级收尾失败");
    }
    let paths = AppPaths::discover()?;
    let binary = fs::canonicalize(&arguments.binary)
        .with_context(|| format!("无法解析已安装程序路径 {}", arguments.binary.display()))?;
    refresh_existing_installation(&paths, &binary, arguments.restart_watcher)
}

fn refresh_existing_installation(
    paths: &AppPaths,
    binary: &Path,
    restart_watcher: bool,
) -> Result<()> {
    let Some(existing) = AppConfig::load(paths)? else {
        clear_watcher_stop_request(paths)?;
        if restart_watcher {
            platform::install_watcher(paths, binary)?;
        }
        return Ok(());
    };
    let original_app_config = read_optional_file(&paths.config)?;
    let setup = install_integration(paths, binary, Some(&existing.installation))?;
    let updated = AppConfig::new(existing.feishu, setup.installation.clone());
    let result = (|| {
        updated.save(paths)?;
        clear_watcher_stop_request(paths)?;
        if restart_watcher {
            platform::install_watcher(paths, binary)?;
        }
        Ok(())
    })();
    if let Err(error) = result {
        let recovery = (|| {
            rollback_integration(paths, &setup)?;
            restore_optional_file(&paths.config, original_app_config.as_deref())
        })();
        return Err(with_recovery(error, recovery));
    }
    Ok(())
}

fn run_update_finalize(
    executable: &Path,
    restart_watcher: bool,
    fail_for_test: bool,
) -> Result<()> {
    let mut command = Command::new(executable);
    command
        .arg("update-finalize")
        .arg("--binary")
        .arg(executable);
    if restart_watcher {
        command.arg("--restart-watcher");
    }
    if fail_for_test {
        command.arg("--fail-for-test");
    }
    let status = command
        .status()
        .with_context(|| format!("无法启动升级后的程序 {}", executable.display()))?;
    if status.success() {
        return Ok(());
    }
    bail!("升级后的程序未能完成收尾操作（{status}）")
}

fn absolute_path(path: &Path) -> Result<PathBuf> {
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()
        .context("无法确定当前目录")?
        .join(path))
}

fn with_recovery(error: anyhow::Error, recovery: Result<()>) -> anyhow::Error {
    match recovery {
        Ok(()) => error,
        Err(recovery_error) => {
            anyhow::anyhow!("{error:#}；恢复升级前的安装也失败了：{recovery_error:#}")
        }
    }
}

fn combine_recovery_steps(first: Result<()>, second: Result<()>) -> Result<()> {
    match (first, second) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(error), Ok(())) | (Ok(()), Err(error)) => Err(error),
        (Err(first), Err(second)) => {
            bail!("{first:#}；恢复后台监听也失败了：{second:#}")
        }
    }
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
            serde_json::to_string_pretty(data).expect("状态检查结果应当可以序列化")
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
    println!("codex-notify 状态");
    println!("-----------------");
    println!("配置状态：{}", if configured { "已完成" } else { "未配置" });
    println!("系统凭据：{}", credential_status_name(secret));
    println!(
        "Codex 通知接入：{}（{}）",
        if notifier { "正常" } else { "未生效" },
        notify_placement_name(notifier_mode)
    );
    println!("UserPromptSubmit Hook：{}", installed_status(hook));
    println!("Stop Hook：{}", installed_status(stop_hook));
    println!("后台监听：{}", installed_status(watcher));

    if !include_guidance {
        return;
    }

    println!("\n检查结果");
    println!("--------");
    let healthy = configured && secret == "present" && notifier && hook && stop_hook && watcher;
    if healthy {
        println!("基础配置正常。");
    }
    if !configured {
        println!("建议：尚未完成配置，请运行 codex-notify init。");
    } else if secret != "present" {
        println!("建议：无法读取 App Secret，请重新运行 codex-notify init 保存凭据。");
    }
    if configured && !notifier {
        match notifier_mode {
            "detached" => {
                println!("建议：当前 config.toml 已切换，请运行 codex-notify sync 重新接入通知。");
            }
            "malformed" => println!(
                "建议：当前 notify 配置格式异常，请检查 config.toml 后再运行 codex-notify sync。"
            ),
            _ => {}
        }
    }
    if configured && (!hook || !stop_hook) {
        println!("建议：缺少必要的 Hook，请重新运行 codex-notify init 进行修复。");
    }
    if configured && !watcher {
        println!("建议：后台监听尚未安装，请重新运行 codex-notify init 进行修复。");
    }
    if hook && stop_hook {
        println!(
            "Hook 信任：ChatGPT App 用户请前往“设置 → 钩子”，在“用户”区域信任这两个 Hook；Codex CLI 用户请运行 /hooks。"
        );
        println!("说明：任务异常识别依赖 Codex 本地记录，会尽力判断，但可能无法覆盖所有异常情况。");
    }
}

fn uninstall(arguments: UninstallArgs) -> Result<()> {
    let paths = AppPaths::discover()?;
    let config = configured(&paths)?;
    let theme = ColorfulTheme::default();
    if !arguments.yes
        && !choose_action(
            &theme,
            "卸载会恢复原有 Codex 通知命令，请选择",
            "确认卸载",
            "保留当前配置",
            false,
        )?
    {
        println!("已取消，没有修改任何配置。");
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
    println!("codex-notify 已移除，并恢复了 {restored_configs} 个由它管理的 Codex 配置文件。");
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
        diagnostics::record(&paths, "原有 Codex 通知命令执行失败");
    }

    let Some(config) = config else {
        diagnostics::record(&paths, "codex-notify 尚未配置，已跳过本次通知");
        return Ok(());
    };

    let event: CompletionEvent = match serde_json::from_str(&event_json) {
        Ok(event) => event,
        Err(_) => {
            diagnostics::record(&paths, "Codex notify 传入了无效的事件 JSON");
            return Ok(());
        }
    };
    if !event.is_completion() || event.is_internal() {
        return Ok(());
    }
    if monitor::mark_turn_completed(&paths, &event.turn_id).is_err() {
        diagnostics::record(&paths, "任务正常完成后未能取消待发送的异常通知");
    }

    let result: Result<()> = (|| {
        let notification = completion_notification(&paths, &event)?;
        let secret = KeyringSecretStore.get_feishu_secret(&config.feishu)?;
        FeishuClient::new()?.send(&config.feishu, &secret, &notification)?;
        Ok(())
    })();
    let _ = remove_completion_state(&paths, &event);
    if result.is_err() {
        diagnostics::record(&paths, "飞书任务完成通知发送失败");
    }
    Ok(())
}

fn prompt_hook() -> Result<()> {
    let paths = AppPaths::discover()?;
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("无法读取 Codex Hook 输入")?;
    match serde_json::from_str::<PromptHookEvent>(&input) {
        Ok(event) => {
            if record_prompt_context(&paths, &event).is_err() {
                diagnostics::record(&paths, "无法记录 Codex 任务上下文");
            }
        }
        Err(_) => diagnostics::record(&paths, "Codex UserPromptSubmit Hook 收到了无效 JSON"),
    }
    println!("{{}}");
    Ok(())
}

fn stop_hook() -> Result<()> {
    let paths = AppPaths::discover()?;
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("无法读取 Codex Stop Hook 输入")?;
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
                diagnostics::record(&paths, "无法记录 Codex Stop 后备事件");
            }
        }
        Ok(_) => {}
        Err(_) => diagnostics::record(&paths, "Codex Stop Hook 收到了无效 JSON"),
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
            diagnostics::record(&paths, &format!("通知接入自动同步失败：{error:#}"));
        }
        if arguments.once || Instant::now() >= next_monitor_scan {
            match watch_once(&paths, &config) {
                Ok(delivered) if arguments.once => {
                    println!("检查完成，本次发送了 {delivered} 条任务异常通知。");
                    return Ok(());
                }
                Ok(_) => {}
                Err(error) if arguments.once => return Err(error),
                Err(error) => diagnostics::record(&paths, &format!("后台监听检查失败：{error:#}")),
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
                diagnostics::record(paths, &format!("飞书任务异常通知发送失败：{error:#}"));
            }
        }
    }
    Ok(delivered)
}

fn configured(paths: &AppPaths) -> Result<AppConfig> {
    AppConfig::load(paths)?.context("codex-notify 尚未配置，请先运行 codex-notify init")
}

fn choose_action(
    theme: &ColorfulTheme,
    prompt: &str,
    confirm_label: &str,
    cancel_label: &str,
    default_confirm: bool,
) -> Result<bool> {
    let options = [confirm_label, cancel_label];
    let selection = Select::with_theme(theme)
        .with_prompt(format!("{prompt}（↑/↓ 切换，回车确认）"))
        .items(&options)
        .default(if default_confirm { 0 } else { 1 })
        .interact()
        .context("无法读取选择结果")?;
    Ok(selection == 0)
}

fn print_existing_configuration(paths: &AppPaths, config: &AppConfig) {
    println!(
        "\n{}",
        existing_configuration_summary(paths, &config.feishu)
    );
}

fn existing_configuration_summary(paths: &AppPaths, feishu: &FeishuConfig) -> String {
    format!(
        "检测到已有配置\n----------------\ncodex-notify 已经配置完成，无需重复初始化。\n  App ID：{}\n  接收方式：{}（{}）\n  接收者：{}\n  配置文件：{}\nApp Secret 已保存在系统凭据库中，这里不会显示。\n重新配置会替换以上飞书设置；Codex 原有通知命令和其他 Hook 不会被删除。",
        feishu.app_id,
        receiver_type_name(feishu.receiver_id_type),
        feishu.receiver_id_type.as_api_value(),
        feishu.receiver_id,
        paths.config.display()
    )
}

fn print_init_intro(reconfiguring: bool) {
    if reconfiguring {
        println!("\n重新配置 codex-notify 飞书通知");
    } else {
        println!("\ncodex-notify 飞书通知配置");
    }
    println!("--------------------------");
    println!("接下来会填写飞书应用凭证和通知接收者，完成最终确认前不会修改任何文件。");
    println!("请先打开飞书开放平台：https://open.feishu.cn/app");
    println!("选择企业自建应用后，可在“凭证与基础信息”中找到 App ID 和 App Secret。");
    println!("如果应用还没有机器人能力和消息权限，请先配置并发布应用。按 Ctrl+C 可随时退出。");
}

fn print_app_id_help() {
    println!("\n[1/4] 应用身份");
    println!("App ID 用于识别你的飞书应用，应以 cli_ 开头。");
}

fn input_value<F>(
    value: Option<String>,
    default: Option<&str>,
    prompt: &str,
    field_name: &str,
    validator: F,
    theme: &ColorfulTheme,
) -> Result<String>
where
    F: Fn(&str) -> std::result::Result<(), String>,
{
    let value = match value {
        Some(value) => value,
        None => {
            let mut input = Input::<String>::with_theme(theme)
                .with_prompt(prompt)
                .validate_with(|input: &String| validator(input.trim()));
            if let Some(default) = default.filter(|value| !value.trim().is_empty()) {
                input = input.default(default.to_owned());
            }
            input
                .interact_text()
                .with_context(|| format!("无法读取{field_name}"))?
        }
    };
    let value = value.trim();
    if let Err(error) = validator(value) {
        bail!("{error}");
    }
    Ok(value.to_owned())
}

fn validate_app_id(value: &str) -> std::result::Result<(), String> {
    if value.is_empty() {
        return Err("App ID 不能为空，请重新输入。".to_owned());
    }
    if !value.starts_with("cli_") {
        return Err("App ID 应以 cli_ 开头，请确认没有误填 App Secret。".to_owned());
    }
    if value.chars().any(char::is_whitespace) {
        return Err("App ID 不应包含空格，请重新输入。".to_owned());
    }
    Ok(())
}

fn secret_value(value: Option<String>, theme: &ColorfulTheme) -> Result<String> {
    let value = match value {
        Some(value) => value,
        None => {
            println!("\n[2/4] 应用密钥");
            println!("App Secret 与 App ID 位于同一页面。粘贴时终端不会显示字符，这是正常现象。");
            let value = Password::with_theme(theme)
                .with_prompt("请粘贴 App Secret（输入内容会隐藏）")
                .allow_empty_password(true)
                .validate_with(|input: &String| -> std::result::Result<(), &str> {
                    if input.trim().is_empty() {
                        Err("App Secret 不能为空，请重新粘贴。")
                    } else {
                        Ok(())
                    }
                })
                .interact()
                .context("无法读取 App Secret")?;
            println!("App Secret 已收到；稍后会安全保存到系统凭据库，不会写入配置文件。");
            value
        }
    };
    if value.trim().is_empty() {
        bail!("App Secret 不能为空");
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
    theme: &ColorfulTheme,
) -> Result<ReceiverIdType> {
    if let Some(value) = value {
        return Ok(value);
    }

    println!("\n[3/4] 消息接收者");
    println!("如果不清楚各种 ID 的区别，选择“邮箱”最容易使用。");
    let options = receiver_type_options();
    let selection = Select::with_theme(theme)
        .with_prompt("请选择接收方式（↑/↓ 切换，回车确认）")
        .items(&options)
        .default(receiver_type_index(
            default.unwrap_or(ReceiverIdType::Email),
        ))
        .interact()
        .context("无法读取接收方式")?;
    Ok(receiver_type_from_index(selection))
}

fn receiver_type_options() -> [&'static str; 4] {
    [
        "邮箱（email）— 填写飞书账号可识别的邮箱，最容易上手",
        "用户 Open ID（open_id）— 形如 ou_xxx，适合私聊",
        "用户 ID（user_id）— 企业内部定义的成员 ID",
        "群聊 ID（chat_id）— 形如 oc_xxx，发送到群聊",
    ]
}

fn receiver_type_index(value: ReceiverIdType) -> usize {
    match value {
        ReceiverIdType::Email => 0,
        ReceiverIdType::OpenId => 1,
        ReceiverIdType::UserId => 2,
        ReceiverIdType::ChatId => 3,
    }
}

fn receiver_type_from_index(index: usize) -> ReceiverIdType {
    match index {
        0 => ReceiverIdType::Email,
        1 => ReceiverIdType::OpenId,
        2 => ReceiverIdType::UserId,
        3 => ReceiverIdType::ChatId,
        _ => unreachable!("接收方式选项超出可用范围"),
    }
}

fn receiver_type_name(value: ReceiverIdType) -> &'static str {
    match value {
        ReceiverIdType::Email => "邮箱",
        ReceiverIdType::OpenId => "用户 Open ID",
        ReceiverIdType::UserId => "用户 ID",
        ReceiverIdType::ChatId => "群聊 ID",
    }
}

fn print_receiver_help(value: ReceiverIdType) {
    match value {
        ReceiverIdType::Email => {
            println!("填写接收人的飞书账号可识别邮箱，例如 name@example.com。");
        }
        ReceiverIdType::OpenId => {
            println!(
                "填写形如 ou_xxx 的 Open ID。可通过飞书开放平台“通过手机号或邮箱获取用户 ID”接口查询。"
            );
        }
        ReceiverIdType::UserId => {
            println!("填写企业通讯录中为成员设置的 User ID；它不是邮箱或 Open ID。");
        }
        ReceiverIdType::ChatId => {
            println!("填写形如 oc_xxx 的群聊 ID；机器人必须已经在该群中。");
        }
    }
}

fn receiver_prompt(value: ReceiverIdType) -> &'static str {
    match value {
        ReceiverIdType::Email => "请输入接收通知的飞书账号邮箱",
        ReceiverIdType::OpenId => "请输入接收人的 Open ID",
        ReceiverIdType::UserId => "请输入接收人的 User ID",
        ReceiverIdType::ChatId => "请输入接收通知的群聊 ID",
    }
}

fn validate_receiver_id(
    receiver_type: ReceiverIdType,
    value: &str,
) -> std::result::Result<(), String> {
    if value.is_empty() {
        return Err("接收者不能为空，请重新输入。".to_owned());
    }
    if value.chars().any(char::is_whitespace) {
        return Err("接收者不应包含空格，请重新输入。".to_owned());
    }
    match receiver_type {
        ReceiverIdType::Email => {
            let valid = value
                .split_once('@')
                .is_some_and(|(name, domain)| !name.is_empty() && domain.contains('.'));
            if !valid {
                return Err("邮箱格式不正确，请输入完整邮箱，例如 name@example.com。".to_owned());
            }
        }
        ReceiverIdType::OpenId if !value.starts_with("ou_") => {
            return Err("Open ID 应以 ou_ 开头；如果你想填写邮箱，请返回后选择“邮箱”。".to_owned());
        }
        ReceiverIdType::ChatId if !value.starts_with("oc_") => {
            return Err("群聊 ID 应以 oc_ 开头，请确认复制的是 chat_id。".to_owned());
        }
        _ => {}
    }
    Ok(())
}

fn resolve_binary(value: Option<PathBuf>) -> Result<PathBuf> {
    let path = match value {
        Some(path) => path,
        None => std::env::current_exe().context("无法确定当前程序路径")?,
    };
    fs::canonicalize(&path).with_context(|| format!("无法解析程序路径 {}", path.display()))
}

fn looks_like_feishu_notifier(command: &[String]) -> bool {
    command.iter().any(|argument| {
        let normalized = argument.to_ascii_lowercase();
        normalized.contains("feishu")
            || normalized.contains("lark")
            || normalized.contains("notify_dispatch")
    })
}

fn installed_status(value: bool) -> &'static str {
    if value { "已安装" } else { "未安装" }
}

fn credential_status_name(value: &str) -> &'static str {
    match value {
        "present" => "可用",
        "unavailable" => "不可用",
        "not_configured" => "未配置",
        _ => "状态未知",
    }
}

fn notify_placement_name(value: &str) -> &'static str {
    match value {
        "direct" => "直接接入",
        "via_computer_use" => "通过 Computer Use 接入",
        "detached" => "已与当前配置断开",
        "malformed" => "配置格式异常",
        "not_configured" => "尚未配置",
        _ => "状态未知",
    }
}

fn read_optional_file(path: &std::path::Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("无法读取 {}", path.display())),
    }
}

fn restore_optional_file(path: &std::path::Path, contents: Option<&[u8]>) -> Result<()> {
    match contents {
        Some(contents) => atomic_write(path, contents),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("无法删除 {}", path.display())),
        },
    }
}

fn watcher_lock_path(paths: &AppPaths) -> PathBuf {
    paths.state.join(WATCHER_LOCK_FILENAME)
}

fn watcher_stop_path(paths: &AppPaths) -> PathBuf {
    paths.state.join(WATCHER_STOP_FILENAME)
}

fn update_lock_path(executable: &Path) -> Result<PathBuf> {
    let parent = executable.parent().context("无法确定升级目标所在目录")?;
    Ok(parent.join(UPDATE_LOCK_FILENAME))
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
        .with_context(|| format!("无法打开 {}", path.display()))?;
    file.try_lock_exclusive()
        .with_context(|| format!("另一个 codex-notify 后台监听正在使用 {}", path.display()))?;
    Ok(file)
}

fn acquire_update_lease(executable: &Path) -> Result<File> {
    let path = update_lock_path(executable)?;
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .with_context(|| format!("无法打开 {}", path.display()))?;
    file.try_lock_exclusive().with_context(|| {
        format!(
            "另一个 codex-notify 升级正在进行，请等待它完成（{}）",
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
        Err(error) => Err(error).with_context(|| format!("无法删除 {}", path.display())),
    }
}

fn request_watcher_stop(paths: &AppPaths) -> Result<()> {
    paths.ensure_directories()?;
    let path = watcher_stop_path(paths);
    atomic_write(&path, b"stop\n")
        .with_context(|| format!("无法通过 {} 请求后台监听停止", path.display()))
}

fn stop_watcher_for_update(paths: &AppPaths) -> Result<()> {
    request_watcher_stop(paths)?;
    if let Err(error) = platform::stop_watcher(paths) {
        let _ = clear_watcher_stop_request(paths);
        return Err(error).context("升级前无法停止后台监听");
    }
    wait_for_watcher_exit(paths).context("升级前无法停止后台监听")
}

fn resume_watcher_after_update(
    paths: &AppPaths,
    binary: &Path,
    restart_watcher: bool,
) -> Result<()> {
    clear_watcher_stop_request(paths)?;
    if restart_watcher {
        platform::install_watcher(paths, binary)?;
    }
    Ok(())
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
        .with_context(|| format!("无法打开 {}", path.display()))?;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => {
                let _ = FileExt::unlock(&file);
                return Ok(());
            }
            Err(error) if is_lock_contended(&error) => {
                if Instant::now() >= deadline {
                    bail!(
                        "后台监听在 {} 秒内未停止，请稍后重试",
                        WATCHER_SHUTDOWN_TIMEOUT.as_secs()
                    );
                }
                thread::sleep(WATCHER_SHUTDOWN_POLL_INTERVAL);
            }
            Err(error) => {
                return Err(error).with_context(|| format!("无法锁定 {}", path.display()));
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
    for entry in fs::read_dir(path).with_context(|| format!("无法读取 {}", path.display()))? {
        let entry = entry.with_context(|| format!("无法读取 {} 中的项目", path.display()))?;
        let entry_path = entry.path();
        let file_type = entry
            .file_type()
            .with_context(|| format!("无法检查 {}", entry_path.display()))?;
        if file_type.is_dir() && !file_type.is_symlink() {
            remove_directory_tree(&entry_path)?;
        } else {
            fs::remove_file(&entry_path)
                .with_context(|| format!("无法删除 {}", entry_path.display()))?;
        }
    }
    fs::remove_dir(path).with_context(|| format!("无法删除 {}", path.display()))
}

#[cfg(test)]
mod cli_tests {
    use super::{
        HOOK_TRUST_GUIDANCE, acquire_update_lease, acquire_watcher_lease,
        existing_configuration_summary, localized_cli_command, receiver_type_from_index,
        receiver_type_index, refresh_existing_installation, remove_directory_tree,
        request_watcher_stop, validate_app_id, validate_receiver_id, wait_for_watcher_exit,
        watcher_stop_requested,
    };
    use crate::paths::AppPaths;
    use crate::settings::{AppConfig, FeishuConfig, ReceiverIdType};
    use clap::error::ErrorKind;
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
    fn receiver_menu_indices_round_trip_every_supported_type() {
        for receiver_type in [
            ReceiverIdType::Email,
            ReceiverIdType::OpenId,
            ReceiverIdType::UserId,
            ReceiverIdType::ChatId,
        ] {
            assert_eq!(
                receiver_type_from_index(receiver_type_index(receiver_type)),
                receiver_type
            );
        }
    }

    #[test]
    fn interactive_field_validation_catches_common_mixups() {
        assert!(validate_app_id("cli_example").is_ok());
        assert!(validate_app_id("secret-value").is_err());

        assert!(validate_receiver_id(ReceiverIdType::Email, "owner@example.com").is_ok());
        assert!(validate_receiver_id(ReceiverIdType::Email, "owner@example").is_err());
        assert!(validate_receiver_id(ReceiverIdType::OpenId, "ou_example").is_ok());
        assert!(validate_receiver_id(ReceiverIdType::OpenId, "owner@example.com").is_err());
        assert!(validate_receiver_id(ReceiverIdType::ChatId, "oc_example").is_ok());
        assert!(validate_receiver_id(ReceiverIdType::ChatId, "ou_example").is_err());
        assert!(validate_receiver_id(ReceiverIdType::UserId, "employee-001").is_ok());
    }

    #[test]
    fn hook_trust_guidance_names_the_app_location_and_both_hooks() {
        assert!(HOOK_TRUST_GUIDANCE.contains("ChatGPT App（原 Codex App）"));
        assert!(HOOK_TRUST_GUIDANCE.contains("“设置”，进入“钩子”"));
        assert!(HOOK_TRUST_GUIDANCE.contains("“用户”区域"));
        assert!(HOOK_TRUST_GUIDANCE.contains("UserPromptSubmit"));
        assert!(HOOK_TRUST_GUIDANCE.contains("Stop"));
    }

    #[test]
    fn command_help_uses_friendly_chinese_everywhere() {
        let mut command = localized_cli_command();
        let root_help = command.render_long_help().to_string();
        assert!(root_help.contains("为 Codex 提供本地飞书通知"));
        assert!(root_help.contains("用法："));
        assert!(root_help.contains("命令："));
        assert!(root_help.contains("显示帮助信息"));
        assert!(!root_help.contains("Usage:"));
        assert!(!root_help.contains("Commands:"));
        assert!(!root_help.contains("Print help"));

        let init_help = command
            .find_subcommand_mut("init")
            .expect("init 子命令")
            .render_long_help()
            .to_string();
        assert!(init_help.contains("飞书 App Secret"));
        assert!(init_help.contains("跳过确认提示"));
        assert!(!init_help.contains("Options:"));
        assert!(!init_help.contains("possible values"));

        for name in [
            "init",
            "test",
            "status",
            "doctor",
            "sync",
            "update",
            "uninstall",
            "watch",
        ] {
            let mut command = localized_cli_command();
            let help = command
                .find_subcommand_mut(name)
                .expect("公开子命令")
                .render_long_help()
                .to_string();
            for english_framework_text in [
                "Usage:",
                "Options:",
                "Print help",
                "[default:",
                "possible values",
            ] {
                assert!(
                    !help.contains(english_framework_text),
                    "{name} 帮助页仍包含英文：{english_framework_text}\n{help}"
                );
            }
        }

        let update_help = localized_cli_command()
            .try_get_matches_from(["codex-notify", "update", "--help"])
            .expect_err("--help 应显示帮助并退出");
        assert_eq!(
            update_help.kind(),
            ErrorKind::DisplayHelp,
            "{update_help:?}"
        );
    }

    #[test]
    fn existing_configuration_summary_prevents_accidental_reconfiguration() {
        let (_app_home, _codex_home, paths) = paths();
        let feishu = FeishuConfig {
            app_id: "cli_existing".to_owned(),
            receiver_id_type: ReceiverIdType::Email,
            receiver_id: "owner@example.com".to_owned(),
        };

        let summary = existing_configuration_summary(&paths, &feishu);

        assert!(summary.contains("检测到已有配置"));
        assert!(summary.contains("无需重复初始化"));
        assert!(summary.contains("重新配置会替换以上飞书设置"));
        assert!(summary.contains("原有通知命令和其他 Hook 不会被删除"));
        assert!(summary.contains("owner@example.com"));
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

    #[test]
    fn concurrent_updates_are_rejected_until_the_lease_is_released() {
        let (_app_home, _codex_home, paths) = paths();
        let executable = paths.root.join("bin").join("codex-notify");
        fs::create_dir_all(executable.parent().expect("executable parent"))
            .expect("create executable parent");
        let first = acquire_update_lease(&executable).expect("first update lease");

        let error = acquire_update_lease(&executable).expect_err("second update must be rejected");
        assert!(error.to_string().contains("另一个 codex-notify 升级"));

        drop(first);
        acquire_update_lease(&executable).expect("lease after release");
    }

    #[test]
    fn update_refresh_preserves_feishu_and_previous_notify_configuration() {
        let (_app_home, _codex_home, paths) = paths();
        paths.ensure_directories().expect("application directories");
        fs::write(
            paths.codex_config(),
            "notify = [\"existing-notifier\", \"--keep\"]\n",
        )
        .expect("initial Codex config");
        let old_binary = paths.root.join("old-codex-notify");
        let new_binary = paths.root.join("new-codex-notify");
        fs::write(&old_binary, b"old").expect("old binary marker");
        fs::write(&new_binary, b"new").expect("new binary marker");
        let setup = crate::codex::install_integration(&paths, &old_binary, None)
            .expect("initial integration");
        let feishu = FeishuConfig {
            app_id: "cli_update_test".to_owned(),
            receiver_id_type: ReceiverIdType::Email,
            receiver_id: "owner@example.com".to_owned(),
        };
        AppConfig::new(feishu.clone(), setup.installation)
            .save(&paths)
            .expect("initial app config");

        refresh_existing_installation(&paths, &new_binary, false).expect("refresh integration");

        let updated = AppConfig::load(&paths)
            .expect("load updated config")
            .expect("configured");
        assert_eq!(updated.feishu, feishu);
        assert_eq!(
            updated.installation.previous_notify,
            Some(vec!["existing-notifier".to_owned(), "--keep".to_owned()])
        );
        assert_eq!(
            updated.installation.managed_notify.first(),
            Some(&new_binary.to_string_lossy().into_owned())
        );
        assert!(
            updated
                .installation
                .managed_binary_paths
                .contains(&old_binary.to_string_lossy().into_owned())
        );
        let codex_config = fs::read_to_string(paths.codex_config()).expect("Codex config");
        assert!(codex_config.contains("existing-notifier"));
        assert!(codex_config.contains(new_binary.to_string_lossy().as_ref()));
        let hooks = fs::read_to_string(paths.codex_hooks()).expect("Codex hooks");
        let hooks: serde_json::Value = serde_json::from_str(&hooks).expect("parse Codex hooks");
        let expected_binary = new_binary.to_string_lossy();
        for event in ["UserPromptSubmit", "Stop"] {
            let handler = &hooks["hooks"][event][0]["hooks"][0];
            for command_field in ["command", "commandWindows"] {
                assert!(
                    handler[command_field]
                        .as_str()
                        .expect("hook command")
                        .contains(expected_binary.as_ref())
                );
            }
        }
    }
}
