use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use toml_edit::{Array, DocumentMut, Item, Value as TomlValue};

use crate::model::Notification;
use crate::notify_config::{
    NotifyPlacement, managed_notify_command as build_managed_notify_command, managed_programs,
    plan_notify_integration, remove_notify_integration,
};
use crate::paths::AppPaths;
use crate::settings::{InstallationConfig, atomic_write, resolved_write_path};
use crate::state::{
    TurnState, elapsed_since, find_thread_title, load_turn_state, prune_turn_states,
    remove_turn_state, write_turn_state,
};

pub const PROMPT_HOOK_MARKER: &str = "codex-notify: record task context";
pub const STOP_HOOK_MARKER: &str = "codex-notify: record interruption fallback";
const INTERNAL_TURN_MARKERS: &[&str] = &[
    "generate 0 to 3 hyperpersonalized suggestions for what this user can do with codex",
    "codex ambient suggestions",
    "you will be presented with a user prompt, and your job is to provide a short title for a task",
];
const INTERNAL_TURN_SIGNATURES: &[&[&str]] = &[&[
    "fill the structured description field with a compact, search-oriented summary",
    "this is a keyword retrieval index, not a broad prose summary",
]];

#[derive(Debug, Clone, Deserialize)]
pub struct CompletionEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    #[serde(
        rename = "thread-id",
        alias = "thread_id",
        default,
        deserialize_with = "string_or_empty"
    )]
    pub thread_id: String,
    #[serde(
        rename = "turn-id",
        alias = "turn_id",
        default,
        deserialize_with = "string_or_empty"
    )]
    pub turn_id: String,
    #[serde(default, deserialize_with = "string_or_empty")]
    pub cwd: String,
    #[serde(
        rename = "input-messages",
        alias = "input_messages",
        default,
        deserialize_with = "string_list_or_empty"
    )]
    pub input_messages: Vec<String>,
    #[serde(
        rename = "last-assistant-message",
        alias = "last_assistant_message",
        default,
        deserialize_with = "string_or_empty"
    )]
    pub last_assistant_message: String,
}

impl CompletionEvent {
    pub fn is_completion(&self) -> bool {
        self.event_type == "agent-turn-complete"
    }

    pub fn task(&self) -> String {
        self.input_messages
            .iter()
            .rev()
            .find(|message| !message.trim().is_empty())
            .map(|message| message.trim().to_owned())
            .unwrap_or_else(|| "\u{672a}\u{547d}\u{540d}\u{4efb}\u{52a1}".to_owned())
    }

    pub fn is_internal(&self) -> bool {
        is_internal_prompt(&self.input_messages.join("\n"))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct PromptHookEvent {
    #[serde(default, deserialize_with = "string_or_empty")]
    pub turn_id: String,
    #[serde(default, deserialize_with = "string_or_empty")]
    pub session_id: String,
    #[serde(default, deserialize_with = "string_or_empty")]
    pub prompt: String,
    #[serde(default, deserialize_with = "string_or_empty")]
    pub cwd: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StopHookEvent {
    #[serde(default, deserialize_with = "string_or_empty")]
    pub turn_id: String,
    #[serde(default, deserialize_with = "string_or_empty")]
    pub session_id: String,
    #[serde(default, deserialize_with = "string_or_empty")]
    pub cwd: String,
    #[serde(default, deserialize_with = "string_or_empty")]
    pub transcript_path: String,
    #[serde(default, deserialize_with = "string_or_empty")]
    pub last_assistant_message: String,
}

pub fn record_prompt_context(paths: &AppPaths, event: &PromptHookEvent) -> Result<bool> {
    if event.turn_id.trim().is_empty() || is_internal_prompt(&event.prompt) {
        return Ok(false);
    }

    let title = find_thread_title(&paths.session_index(), &event.session_id);
    let state = TurnState::new(
        nonempty_or(&event.prompt, "\u{672a}\u{547d}\u{540d}\u{4efb}\u{52a1}"),
        event.cwd.trim(),
        event.session_id.trim(),
        title,
    );
    write_turn_state(&paths.state, &event.turn_id, &state)?;
    prune_turn_states(&paths.state, Duration::from_secs(24 * 60 * 60))?;
    Ok(true)
}

pub fn completion_notification(paths: &AppPaths, event: &CompletionEvent) -> Result<Notification> {
    let state = load_turn_state(&paths.state, &event.turn_id)?;
    let task = state
        .as_ref()
        .map(|state| state.prompt.trim())
        .filter(|task| !task.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| event.task());
    let thread_id = nonempty_or(
        &event.thread_id,
        state
            .as_ref()
            .map(|state| state.thread_id.as_str())
            .unwrap_or_default(),
    );
    let session_index = paths.session_index();
    let title = find_thread_title(&session_index, &thread_id)
        .or_else(|| {
            state
                .as_ref()
                .and_then(|state| state.conversation_title_at_start.clone())
                .filter(|title| !title.trim().is_empty())
        })
        .unwrap_or_else(|| "Codex \u{4f1a}\u{8bdd}".to_owned());
    let elapsed = state
        .as_ref()
        .and_then(|state| elapsed_since(state, SystemTime::now()));
    let details = nonempty_or(
        &event.last_assistant_message,
        "\u{4efb}\u{52a1}\u{5df2}\u{5b8c}\u{6210}\u{3002}",
    );

    let mut notification = Notification::completed(
        title,
        task,
        details,
        elapsed,
        nonempty_or(&event.turn_id, "unknown-turn"),
    );
    notification.workspace = nonempty_path(
        &event.cwd,
        state
            .as_ref()
            .map(|state| state.cwd.as_str())
            .unwrap_or_default(),
    );
    Ok(notification)
}

pub fn remove_completion_state(paths: &AppPaths, event: &CompletionEvent) -> Result<()> {
    remove_turn_state(&paths.state, &event.turn_id)
}

pub fn is_internal_prompt(prompt: &str) -> bool {
    let normalized = prompt.to_ascii_lowercase();
    INTERNAL_TURN_MARKERS
        .iter()
        .any(|marker| normalized.contains(marker))
        || INTERNAL_TURN_SIGNATURES
            .iter()
            .any(|signature| signature.iter().all(|marker| normalized.contains(marker)))
}

pub fn managed_notify_command(binary: &Path) -> Vec<String> {
    build_managed_notify_command(binary)
}

pub fn read_notify_command(config_path: &Path) -> Result<Option<Vec<String>>> {
    let document = read_toml_document(config_path)?;
    notify_command_from_document(&document)
}

fn notify_command_from_document(document: &DocumentMut) -> Result<Option<Vec<String>>> {
    let Some(item) = document.get("notify") else {
        return Ok(None);
    };
    let array = item.as_array().context("Codex notify 必须是命令参数数组")?;
    let mut command = Vec::with_capacity(array.len());
    for argument in array.iter() {
        let argument = argument
            .as_str()
            .context("Codex notify 命令参数必须是字符串")?;
        command.push(argument.to_owned());
    }
    Ok((!command.is_empty()).then_some(command))
}

fn notify_command_from_snapshot(
    config_path: &Path,
    contents: Option<&[u8]>,
) -> Result<Option<Vec<String>>> {
    let document = match contents {
        Some(contents) => std::str::from_utf8(contents)
            .with_context(|| format!("{} 不是有效的 UTF-8 文件", config_path.display()))?
            .parse::<DocumentMut>()
            .with_context(|| format!("无法解析配置文件 {}", config_path.display()))?,
        None => DocumentMut::new(),
    };
    notify_command_from_document(&document)
}

pub fn set_notify_command(config_path: &Path, command: &[String]) -> Result<()> {
    if command.is_empty() {
        bail!("无法设置空的 Codex notify 命令");
    }
    let mut document = read_toml_document(config_path)?;
    document["notify"] = command_item(command);
    atomic_write(config_path, document.to_string().as_bytes())
}

fn set_notify_command_if_unchanged(
    config_path: &Path,
    expected_contents: Option<&[u8]>,
    command: &[String],
) -> Result<bool> {
    if command.is_empty() {
        bail!("无法设置空的 Codex notify 命令");
    }
    let current_contents = read_optional_file(config_path)?;
    if current_contents.as_deref() != expected_contents {
        return Ok(false);
    }
    let mut document = match expected_contents {
        Some(contents) => std::str::from_utf8(contents)
            .with_context(|| format!("{} 不是有效的 UTF-8 文件", config_path.display()))?
            .parse::<DocumentMut>()
            .with_context(|| format!("无法解析配置文件 {}", config_path.display()))?,
        None => DocumentMut::new(),
    };
    document["notify"] = command_item(command);
    atomic_write(config_path, document.to_string().as_bytes())?;
    Ok(true)
}

pub fn remove_notify_command(config_path: &Path) -> Result<()> {
    let mut document = read_toml_document(config_path)?;
    if document.remove("notify").is_some() {
        atomic_write(config_path, document.to_string().as_bytes())?;
    }
    Ok(())
}

fn restore_notify_if_current(
    config_path: &Path,
    expected_current: &[String],
    previous: Option<&[String]>,
) -> Result<bool> {
    if read_notify_command(config_path)?.as_deref() != Some(expected_current) {
        return Ok(false);
    }
    match previous {
        Some(command) => set_notify_command(config_path, command)?,
        None => remove_notify_command(config_path)?,
    }
    Ok(true)
}

pub fn install_prompt_hook(hooks_path: &Path, binary: &Path) -> Result<bool> {
    install_hook(
        hooks_path,
        "UserPromptSubmit",
        PROMPT_HOOK_MARKER,
        prompt_hook_command(binary),
        prompt_hook_command_windows(binary),
        10,
    )
}

pub fn install_stop_hook(hooks_path: &Path, binary: &Path) -> Result<bool> {
    install_hook(
        hooks_path,
        "Stop",
        STOP_HOOK_MARKER,
        stop_hook_command(binary),
        stop_hook_command_windows(binary),
        10,
    )
}

fn install_hook(
    hooks_path: &Path,
    event_name: &str,
    marker: &str,
    command: String,
    command_windows: String,
    timeout: u64,
) -> Result<bool> {
    let mut document = read_hooks_document(hooks_path)?;
    let root = document
        .as_object_mut()
        .context("Codex hooks.json 顶层必须是 JSON 对象")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("Codex hooks.json 中的 hooks 必须是 JSON 对象")?;
    let groups = hooks
        .entry(event_name)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .with_context(|| format!("Codex {event_name} hooks 必须是数组"))?;

    if groups
        .iter()
        .any(|group| group_contains_marker(group, marker))
    {
        return Ok(false);
    }

    groups.push(json!({
        "hooks": [{
            "type": "command",
            "command": command,
            "commandWindows": command_windows,
            "timeout": timeout,
            "statusMessage": marker,
        }],
    }));
    write_hooks_document(hooks_path, &document)?;
    Ok(true)
}

pub fn has_prompt_hook(hooks_path: &Path) -> Result<bool> {
    has_hook(hooks_path, "UserPromptSubmit", PROMPT_HOOK_MARKER)
}

pub fn has_stop_hook(hooks_path: &Path) -> Result<bool> {
    has_hook(hooks_path, "Stop", STOP_HOOK_MARKER)
}

fn has_hook(hooks_path: &Path, event_name: &str, marker: &str) -> Result<bool> {
    let document = read_hooks_document(hooks_path)?;
    let groups = document
        .get("hooks")
        .and_then(Value::as_object)
        .and_then(|hooks| hooks.get(event_name))
        .and_then(Value::as_array);
    Ok(groups.is_some_and(|groups| {
        groups
            .iter()
            .any(|group| group_contains_marker(group, marker))
    }))
}

pub fn remove_prompt_hook(hooks_path: &Path) -> Result<bool> {
    remove_hook(hooks_path, "UserPromptSubmit", PROMPT_HOOK_MARKER)
}

pub fn remove_stop_hook(hooks_path: &Path) -> Result<bool> {
    remove_hook(hooks_path, "Stop", STOP_HOOK_MARKER)
}

pub fn remove_empty_created_codex_files(
    config_path: &Path,
    hooks_path: &Path,
    installation: &InstallationConfig,
) -> Result<()> {
    if installation.created_codex_config && config_path.exists() {
        let document = read_toml_document(config_path)?;
        if document.as_table().is_empty() {
            fs::remove_file(config_path)
                .with_context(|| format!("无法删除 {}", config_path.display()))?;
        }
    }
    if installation.created_codex_hooks && hooks_path.exists() {
        let document = read_hooks_document(hooks_path)?;
        let empty = document.as_object().is_some_and(|root| {
            root.is_empty()
                || (root.len() == 1
                    && root
                        .get("hooks")
                        .and_then(Value::as_object)
                        .is_some_and(Map::is_empty))
        });
        if empty {
            fs::remove_file(hooks_path)
                .with_context(|| format!("无法删除 {}", hooks_path.display()))?;
        }
    }
    Ok(())
}

fn remove_hook(hooks_path: &Path, event_name: &str, marker: &str) -> Result<bool> {
    if !hooks_path.exists() {
        return Ok(false);
    }
    let mut document = read_hooks_document(hooks_path)?;
    let mut changed = false;

    {
        let root = document
            .as_object_mut()
            .context("Codex hooks.json 顶层必须是 JSON 对象")?;
        let Some(hooks) = root.get_mut("hooks").and_then(Value::as_object_mut) else {
            return Ok(false);
        };
        let Some(groups) = hooks.get_mut(event_name).and_then(Value::as_array_mut) else {
            return Ok(false);
        };

        let old_groups = std::mem::take(groups);
        let mut retained_groups = Vec::with_capacity(old_groups.len());
        for mut group in old_groups {
            let mut remove_group = false;
            if let Some(handlers) = group.get_mut("hooks").and_then(Value::as_array_mut) {
                let old_handler_count = handlers.len();
                handlers.retain(|handler| !handler_has_marker(handler, marker));
                if handlers.len() != old_handler_count {
                    changed = true;
                }
                remove_group = handlers.is_empty();
            }
            if !remove_group {
                retained_groups.push(group);
            }
        }
        *groups = retained_groups;
        if groups.is_empty() {
            hooks.remove(event_name);
        }
    }

    if changed {
        write_hooks_document(hooks_path, &document)?;
    }
    Ok(changed)
}

pub fn backup_file(paths: &AppPaths, source: &Path, label: &str) -> Result<Option<PathBuf>> {
    if !source.exists() {
        return Ok(None);
    }
    paths.ensure_directories()?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let extension = source
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or("bak");
    let destination = paths
        .backups
        .join(format!("{label}-{timestamp}.{extension}"));
    fs::copy(source, &destination).with_context(|| {
        format!(
            "无法将 {} 备份到 {}",
            source.display(),
            destination.display()
        )
    })?;
    Ok(Some(destination))
}

pub fn restore_notify_command(
    config_path: &Path,
    installation: &InstallationConfig,
) -> Result<RestoreNotifyResult> {
    let active = read_notify_command(config_path)?;
    let programs = installation_managed_programs(installation);
    let Some(removal) =
        remove_notify_integration(active, &programs, installation.previous_notify.as_deref())?
    else {
        return Ok(RestoreNotifyResult::NotOwned);
    };

    match removal.restored_command {
        Some(command) => set_notify_command(config_path, &command)?,
        None => remove_notify_command(config_path)?,
    }
    Ok(RestoreNotifyResult::Restored)
}

pub fn notify_integration_placement(
    config_path: &Path,
    installation: &InstallationConfig,
) -> Result<Option<NotifyPlacement>> {
    let active = read_notify_command(config_path)?;
    let binary = installation
        .managed_notify
        .first()
        .context("缺少由 codex-notify 管理的程序路径")?;
    let canonical_managed = managed_notify_command(Path::new(binary));
    let programs = installation_managed_programs_with(installation, &canonical_managed);
    let plan = plan_notify_integration(
        active.clone(),
        &canonical_managed,
        &programs,
        installation.previous_notify.as_deref(),
    )?;
    Ok((active.as_deref() == Some(plan.active_command.as_slice())).then_some(plan.placement))
}

#[derive(Debug, Clone)]
pub struct NotifyReconcileResult {
    pub changed: bool,
    pub owned_before: bool,
    pub legacy_owned_before: bool,
    pub placement: NotifyPlacement,
    pub managed_notify: Vec<String>,
    pub previous_notify: Option<Vec<String>>,
    pub managed_config_path: PathBuf,
    pub config_backup: Option<PathBuf>,
}

pub fn reconcile_notify_integration(
    paths: &AppPaths,
    installation: &InstallationConfig,
) -> Result<NotifyReconcileResult> {
    let binary = installation
        .managed_notify
        .first()
        .context("缺少由 codex-notify 管理的程序路径")?;
    let canonical_managed = managed_notify_command(Path::new(binary));
    let programs = installation_managed_programs_with(installation, &canonical_managed);
    let config_path = paths.codex_config();
    let initial_managed_config_path = resolved_write_path(&config_path)?;
    let original_config = read_optional_file(&config_path)?;
    let active = notify_command_from_snapshot(&config_path, original_config.as_deref())?;
    let plan = plan_notify_integration(
        active.clone(),
        &canonical_managed,
        &programs,
        installation.previous_notify.as_deref(),
    )?;
    let changed = active.as_deref() != Some(plan.active_command.as_slice());
    let config_backup = if changed {
        let backup = backup_file(paths, &config_path, "reconcile-config")?;
        if !set_notify_command_if_unchanged(
            &config_path,
            original_config.as_deref(),
            &plan.active_command,
        )? {
            bail!("codex-notify 准备同步时 Codex 配置发生了变化，将在下次检查时重试");
        }
        backup
    } else {
        None
    };
    let managed_config_path =
        resolved_write_path(&config_path).unwrap_or(initial_managed_config_path);
    Ok(NotifyReconcileResult {
        changed,
        owned_before: plan.owned_before,
        legacy_owned_before: plan.legacy_owned_before,
        placement: plan.placement,
        managed_notify: canonical_managed,
        previous_notify: plan.previous_notify,
        managed_config_path,
        config_backup,
    })
}

#[derive(Debug, Clone)]
pub struct IntegrationSetup {
    pub installation: InstallationConfig,
    pub config_backup: Option<PathBuf>,
    pub hooks_backup: Option<PathBuf>,
    modified_config_path: PathBuf,
    notify_before: Option<Vec<String>>,
    notify_after: Vec<String>,
}

pub fn install_integration(
    paths: &AppPaths,
    binary: &Path,
    previous_installation: Option<&InstallationConfig>,
) -> Result<IntegrationSetup> {
    let config_path = paths.codex_config();
    let hooks_path = paths.codex_hooks();
    let original_config = read_optional_file(&config_path)?;
    let active_notify = notify_command_from_snapshot(&config_path, original_config.as_deref())?;
    let managed_notify = managed_notify_command(binary);
    let initial_resolved_config_path = resolved_write_path(&config_path)?;
    let historical_programs = previous_installation
        .map(installation_historical_programs)
        .unwrap_or_default();
    let programs = managed_programs(&managed_notify, &historical_programs);
    let plan = plan_notify_integration(
        active_notify.clone(),
        &managed_notify,
        &programs,
        previous_installation.and_then(|installation| installation.previous_notify.as_deref()),
    )?;

    let original_hooks = read_optional_file(&hooks_path)?;
    let created_codex_config = previous_installation
        .map(|installation| installation.created_codex_config)
        .unwrap_or(original_config.is_none());
    let created_codex_hooks = previous_installation
        .map(|installation| installation.created_codex_hooks)
        .unwrap_or(original_hooks.is_none());
    let config_backup = backup_file(paths, &config_path, "config")?;
    let hooks_backup = backup_file(paths, &hooks_path, "hooks")?;

    let mut notify_written = false;
    let result = (|| {
        if !set_notify_command_if_unchanged(
            &initial_resolved_config_path,
            original_config.as_deref(),
            &plan.active_command,
        )? {
            bail!("codex-notify 准备写入时 Codex 配置发生了变化，请重新运行 codex-notify init");
        }
        notify_written = true;
        if has_prompt_hook(&hooks_path)? {
            remove_prompt_hook(&hooks_path)?;
        }
        install_prompt_hook(&hooks_path, binary)?;
        if has_stop_hook(&hooks_path)? {
            remove_stop_hook(&hooks_path)?;
        }
        install_stop_hook(&hooks_path, binary)?;
        Ok(())
    })();

    if let Err(error) = result {
        if notify_written {
            let _ = restore_notify_if_current(
                &config_path,
                &plan.active_command,
                active_notify.as_deref(),
            );
            restore_original_file(&hooks_path, original_hooks.as_deref())?;
        }
        return Err(error);
    }

    let mut managed_binary_paths = historical_programs;
    if !managed_binary_paths.contains(&managed_notify[0]) {
        managed_binary_paths.push(managed_notify[0].clone());
    }
    let mut managed_config_paths = previous_installation
        .map(|installation| installation.managed_config_paths.clone())
        .unwrap_or_default();
    if let Some(legacy_path) = previous_installation
        .map(|installation| installation.codex_config_path.clone())
        .filter(|path| !path.trim().is_empty() && !managed_config_paths.contains(path))
    {
        managed_config_paths.push(legacy_path);
    }
    let resolved_config_path = initial_resolved_config_path.to_string_lossy().into_owned();
    if !managed_config_paths.contains(&resolved_config_path) {
        managed_config_paths.push(resolved_config_path.clone());
    }

    Ok(IntegrationSetup {
        installation: InstallationConfig {
            previous_notify: plan.previous_notify,
            managed_notify,
            managed_binary_paths,
            managed_config_paths,
            codex_config_path: config_path.to_string_lossy().into_owned(),
            codex_hooks_path: hooks_path.to_string_lossy().into_owned(),
            prompt_hook_marker: PROMPT_HOOK_MARKER.to_owned(),
            stop_hook_marker: STOP_HOOK_MARKER.to_owned(),
            created_codex_config,
            created_codex_hooks,
        },
        config_backup,
        hooks_backup,
        modified_config_path: initial_resolved_config_path,
        notify_before: active_notify,
        notify_after: plan.active_command,
    })
}

fn installation_historical_programs(installation: &InstallationConfig) -> Vec<String> {
    let mut programs = installation.managed_binary_paths.clone();
    if let Some(program) = installation.managed_notify.first()
        && !program.trim().is_empty()
        && !programs.contains(program)
    {
        programs.push(program.clone());
    }
    programs
}

fn installation_managed_programs(installation: &InstallationConfig) -> Vec<String> {
    installation_managed_programs_with(installation, &installation.managed_notify)
}

fn installation_managed_programs_with(
    installation: &InstallationConfig,
    canonical_managed: &[String],
) -> Vec<String> {
    managed_programs(
        canonical_managed,
        &installation_historical_programs(installation),
    )
}

pub fn rollback_integration(paths: &AppPaths, setup: &IntegrationSetup) -> Result<()> {
    let _ = restore_notify_if_current(
        &setup.modified_config_path,
        &setup.notify_after,
        setup.notify_before.as_deref(),
    )?;
    restore_from_backup(&paths.codex_hooks(), setup.hooks_backup.as_deref())?;
    remove_empty_created_codex_files(
        &setup.modified_config_path,
        &paths.codex_hooks(),
        &setup.installation,
    )
}

pub fn run_previous_notifier(command: &[String], event_json: &str) -> Result<()> {
    let (program, arguments) = command
        .split_first()
        .context("保存的原 Codex notify 命令为空")?;
    let status = Command::new(program)
        .args(arguments)
        .arg(event_json)
        .status()
        .with_context(|| format!("无法运行原 Codex notify 命令“{program}”"))?;
    if !status.success() {
        return Err(anyhow!(
            "原 Codex notify 命令“{program}”退出，状态为 {status}"
        ));
    }
    Ok(())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestoreNotifyResult {
    Restored,
    NotOwned,
}

fn read_toml_document(path: &Path) -> Result<DocumentMut> {
    let contents = match fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(DocumentMut::new());
        }
        Err(error) => {
            return Err(error).with_context(|| format!("无法读取 {}", path.display()));
        }
    };
    contents
        .parse::<DocumentMut>()
        .with_context(|| format!("无法解析配置文件 {}", path.display()))
}

fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("无法读取 {}", path.display())),
    }
}

fn restore_original_file(path: &Path, contents: Option<&[u8]>) -> Result<()> {
    match contents {
        Some(contents) => atomic_write(path, contents),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error).with_context(|| format!("无法删除 {}", path.display())),
        },
    }
}

fn restore_from_backup(destination: &Path, backup: Option<&Path>) -> Result<()> {
    let contents = match backup {
        Some(backup) => Some(
            fs::read(backup)
                .with_context(|| format!("无法读取接入配置备份 {}", backup.display()))?,
        ),
        None => None,
    };
    restore_original_file(destination, contents.as_deref())
}

fn command_item(command: &[String]) -> Item {
    let mut array = Array::new();
    for argument in command {
        array.push(argument.as_str());
    }
    Item::Value(TomlValue::Array(array))
}

fn read_hooks_document(path: &Path) -> Result<Value> {
    let contents = match fs::read(path) {
        Ok(contents) => contents,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(json!({ "hooks": {} }));
        }
        Err(error) => {
            return Err(error).with_context(|| format!("无法读取 {}", path.display()));
        }
    };
    serde_json::from_slice(&contents)
        .with_context(|| format!("无法解析 Hook 配置 {}", path.display()))
}

fn write_hooks_document(path: &Path, document: &Value) -> Result<()> {
    let contents = serde_json::to_vec_pretty(document).context("无法生成 Codex Hook 配置")?;
    atomic_write(path, &contents)
}

fn group_contains_marker(group: &Value, marker: &str) -> bool {
    group
        .get("hooks")
        .and_then(Value::as_array)
        .is_some_and(|handlers| {
            handlers
                .iter()
                .any(|handler| handler_has_marker(handler, marker))
        })
}

fn handler_has_marker(handler: &Value, marker: &str) -> bool {
    handler.get("statusMessage").and_then(Value::as_str) == Some(marker)
}

fn prompt_hook_command(binary: &Path) -> String {
    format!(
        "{} prompt-hook",
        shell_quote_posix(&binary.to_string_lossy())
    )
}

fn prompt_hook_command_windows(binary: &Path) -> String {
    let path = binary.to_string_lossy().replace('\'', "''");
    format!("& '{path}' prompt-hook")
}

fn stop_hook_command(binary: &Path) -> String {
    format!("{} stop-hook", shell_quote_posix(&binary.to_string_lossy()))
}

fn stop_hook_command_windows(binary: &Path) -> String {
    let path = binary.to_string_lossy().replace('\'', "''");
    format!("& '{path}' stop-hook")
}

fn shell_quote_posix(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

fn nonempty_or(value: &str, fallback: &str) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        fallback.to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn string_or_empty<'de, D>(deserializer: D) -> std::result::Result<String, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

fn string_list_or_empty<'de, D>(deserializer: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: Deserializer<'de>,
{
    Ok(Option::<Vec<String>>::deserialize(deserializer)?.unwrap_or_default())
}

fn nonempty_path(primary: &str, fallback: &str) -> Option<PathBuf> {
    let value = nonempty_or(primary, fallback);
    (value != fallback || !fallback.is_empty())
        .then(|| PathBuf::from(value))
        .filter(|path| !path.as_os_str().is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        CompletionEvent, PROMPT_HOOK_MARKER, PromptHookEvent, completion_notification,
        has_prompt_hook, has_stop_hook, install_integration, install_prompt_hook,
        install_stop_hook, is_internal_prompt, read_notify_command, record_prompt_context,
        remove_empty_created_codex_files, remove_prompt_hook, remove_stop_hook,
        restore_notify_command, rollback_integration, set_notify_command,
        set_notify_command_if_unchanged,
    };
    use crate::paths::AppPaths;
    use std::fs;
    use std::path::Path;
    use tempfile::tempdir;

    fn paths(app_home: &Path, codex_home: &Path) -> AppPaths {
        AppPaths {
            config: app_home.join("config.toml"),
            state: app_home.join("state"),
            logs: app_home.join("logs"),
            backups: app_home.join("backups"),
            root: app_home.to_path_buf(),
            codex_home: codex_home.to_path_buf(),
        }
    }

    #[test]
    fn notify_configuration_preserves_other_toml_content() {
        let directory = tempdir().expect("temporary directory");
        let config_path = directory.path().join("config.toml");
        fs::write(
            &config_path,
            "# keep this comment\nmodel = \"gpt-5\"\nnotify = [\"old\", \"notifier\"]\n",
        )
        .expect("write config");

        assert_eq!(
            read_notify_command(&config_path).expect("read notify"),
            Some(vec!["old".to_owned(), "notifier".to_owned()])
        );
        set_notify_command(
            &config_path,
            &["/Applications/codex-notify".to_owned(), "notify".to_owned()],
        )
        .expect("set notify");

        let contents = fs::read_to_string(&config_path).expect("read config");
        assert!(contents.contains("# keep this comment"));
        assert!(contents.contains("model = \"gpt-5\""));
        assert_eq!(
            read_notify_command(&config_path).expect("read notify"),
            Some(vec![
                "/Applications/codex-notify".to_owned(),
                "notify".to_owned()
            ])
        );
    }

    #[test]
    fn stale_notify_update_does_not_overwrite_a_switched_configuration() {
        let directory = tempdir().expect("temporary directory");
        let config_path = directory.path().join("config.toml");
        fs::write(
            &config_path,
            "model = \"profile-a\"\nnotify = [\"notifier-a\"]\n",
        )
        .expect("write profile A");
        let profile_a = fs::read(&config_path).expect("snapshot profile A");
        let profile_b = "model = \"profile-b\"\nnotify = [\"notifier-b\"]\n";
        fs::write(&config_path, profile_b).expect("switch to profile B");

        let written = set_notify_command_if_unchanged(
            &config_path,
            Some(&profile_a),
            &["codex-notify".to_owned(), "notify".to_owned()],
        )
        .expect("conditional update");

        assert!(!written);
        assert_eq!(
            fs::read_to_string(&config_path).expect("read active profile"),
            profile_b
        );
    }

    #[test]
    fn prompt_hook_merge_and_removal_leave_unrelated_hooks_intact() {
        let directory = tempdir().expect("temporary directory");
        let hooks_path = directory.path().join("hooks.json");
        fs::write(
            &hooks_path,
            r#"{
  "hooks": {
    "PreToolUse": [{"hooks": [{"type": "command", "command": "keep-me"}]}],
    "UserPromptSubmit": [{"hooks": [{"type": "command", "command": "other"}]}]
  }
}"#,
        )
        .expect("write hooks");

        assert!(install_prompt_hook(&hooks_path, Path::new("/tmp/codex-notify")).expect("install"));
        assert!(has_prompt_hook(&hooks_path).expect("has hook"));
        assert!(
            !install_prompt_hook(&hooks_path, Path::new("/tmp/codex-notify")).expect("idempotent")
        );
        assert!(remove_prompt_hook(&hooks_path).expect("remove"));

        let contents = fs::read_to_string(&hooks_path).expect("read hooks");
        assert!(contents.contains("keep-me"));
        assert!(contents.contains("other"));
        assert!(!contents.contains(PROMPT_HOOK_MARKER));
    }

    #[test]
    fn stop_hook_merge_and_removal_leave_unrelated_hooks_intact() {
        let directory = tempdir().expect("temporary directory");
        let hooks_path = directory.path().join("hooks.json");
        fs::write(
            &hooks_path,
            r#"{
  "hooks": {
    "Stop": [{"hooks": [{"type": "command", "command": "keep-stop"}]}]
  }
}"#,
        )
        .expect("write hooks");

        assert!(install_stop_hook(&hooks_path, Path::new("/tmp/codex-notify")).expect("install"));
        assert!(has_stop_hook(&hooks_path).expect("has hook"));
        assert!(remove_stop_hook(&hooks_path).expect("remove"));

        let contents = fs::read_to_string(&hooks_path).expect("read hooks");
        assert!(contents.contains("keep-stop"));
        assert!(!contents.contains("codex-notify: record interruption fallback"));
    }

    #[test]
    fn prompt_context_uses_session_title_without_using_the_task_as_title() {
        let app_home = tempdir().expect("temporary app home");
        let codex_home = tempdir().expect("temporary Codex home");
        let paths = paths(app_home.path(), codex_home.path());
        fs::write(
            paths.session_index(),
            "{\"id\":\"thread-1\",\"thread_name\":\"\u{786e}\u{8ba4}\u{4efb}\u{52a1}\u{5b8c}\u{6210}\u{901a}\u{77e5}\u{80fd}\u{529b}\"}\n",
        )
        .expect("write session index");
        let event = PromptHookEvent {
            turn_id: "turn-1".to_owned(),
            session_id: "thread-1".to_owned(),
            prompt: "\u{5b9e}\u{73b0}\u{98de}\u{4e66}\u{901a}\u{77e5}".to_owned(),
            cwd: "/workspace".to_owned(),
        };
        assert!(record_prompt_context(&paths, &event).expect("record prompt"));

        let completion = CompletionEvent {
            event_type: "agent-turn-complete".to_owned(),
            thread_id: "thread-1".to_owned(),
            turn_id: "turn-1".to_owned(),
            cwd: "/workspace".to_owned(),
            input_messages: vec!["fallback task".to_owned()],
            last_assistant_message: "done".to_owned(),
        };
        let notification = completion_notification(&paths, &completion).expect("notification");
        assert_eq!(
            notification.conversation_title,
            "\u{786e}\u{8ba4}\u{4efb}\u{52a1}\u{5b8c}\u{6210}\u{901a}\u{77e5}\u{80fd}\u{529b}"
        );
        assert_eq!(
            notification.task,
            "\u{5b9e}\u{73b0}\u{98de}\u{4e66}\u{901a}\u{77e5}"
        );
    }

    #[test]
    fn completion_returns_without_blocking_when_a_new_session_title_is_pending() {
        let app_home = tempdir().expect("temporary app home");
        let codex_home = tempdir().expect("temporary Codex home");
        let paths = paths(app_home.path(), codex_home.path());
        fs::write(paths.session_index(), "").expect("create session index");
        let prompt = PromptHookEvent {
            turn_id: "turn-new".to_owned(),
            session_id: "thread-new".to_owned(),
            prompt: "First task".to_owned(),
            cwd: "/workspace".to_owned(),
        };
        assert!(record_prompt_context(&paths, &prompt).expect("record prompt"));

        let completion = CompletionEvent {
            event_type: "agent-turn-complete".to_owned(),
            thread_id: "thread-new".to_owned(),
            turn_id: "turn-new".to_owned(),
            cwd: "/workspace".to_owned(),
            input_messages: vec!["Fallback task".to_owned()],
            last_assistant_message: "Done".to_owned(),
        };

        let notification = completion_notification(&paths, &completion).expect("notification");
        assert_eq!(notification.conversation_title, "Codex \u{4f1a}\u{8bdd}");
    }

    #[test]
    fn known_ambient_turns_are_filtered() {
        assert!(is_internal_prompt(
            "Generate 0 to 3 hyperpersonalized suggestions for what this user can do with Codex"
        ));
        assert!(is_internal_prompt(
            "Fill the structured description field with a compact, search-oriented summary. \
             This is a keyword retrieval index, not a broad prose summary."
        ));
        assert!(!is_internal_prompt(
            "Fill the structured description field with a compact, search-oriented summary."
        ));
        assert!(!is_internal_prompt("Implement the notification provider"));
    }

    #[test]
    fn nullable_codex_event_fields_fall_back_without_failing_dispatch() {
        let event: CompletionEvent = serde_json::from_str(
            r#"{
                "type": "agent-turn-complete",
                "thread-id": null,
                "turn-id": null,
                "cwd": null,
                "input-messages": null,
                "last-assistant-message": null
            }"#,
        )
        .expect("parse nullable event");
        assert!(event.thread_id.is_empty());
        assert!(event.turn_id.is_empty());
        assert!(event.input_messages.is_empty());
        assert!(event.last_assistant_message.is_empty());
    }

    #[test]
    fn integration_preserves_the_previous_notifier_and_adds_our_hook() {
        let app_home = tempdir().expect("temporary app home");
        let codex_home = tempdir().expect("temporary Codex home");
        let paths = paths(app_home.path(), codex_home.path());
        fs::write(
            paths.codex_config(),
            "notify = [\"python3\", \"/tmp/previous-notify.py\"]\n",
        )
        .expect("write config");

        let setup =
            install_integration(&paths, Path::new("/tmp/codex-notify"), None).expect("install");
        assert_eq!(
            setup.installation.previous_notify,
            Some(vec![
                "python3".to_owned(),
                "/tmp/previous-notify.py".to_owned()
            ])
        );
        assert_eq!(
            read_notify_command(&paths.codex_config()).expect("read notifier"),
            Some(vec![
                "/tmp/codex-notify".to_owned(),
                "notify".to_owned(),
                "--managed".to_owned(),
                "--forward-notify".to_owned(),
                "[\"python3\",\"/tmp/previous-notify.py\"]".to_owned(),
            ])
        );
        assert!(has_prompt_hook(&paths.codex_hooks()).expect("has prompt hook"));
        assert!(has_stop_hook(&paths.codex_hooks()).expect("has Stop hook"));
        assert!(setup.config_backup.is_some());
        assert!(!setup.installation.created_codex_config);
        assert!(setup.installation.created_codex_hooks);
        rollback_integration(&paths, &setup).expect("rollback");
        assert_eq!(
            fs::read_to_string(paths.codex_config()).expect("restored config"),
            "notify = [\"python3\", \"/tmp/previous-notify.py\"]\n"
        );
        assert!(!paths.codex_hooks().exists());
    }

    #[test]
    fn failed_upgrade_rollback_restores_the_previous_managed_integration_only() {
        let app_home = tempdir().expect("temporary app home");
        let codex_home = tempdir().expect("temporary Codex home");
        let paths = paths(app_home.path(), codex_home.path());
        let first = install_integration(&paths, Path::new("/tmp/codex-notify-old"), None)
            .expect("install old integration");
        let old_notify = read_notify_command(&paths.codex_config())
            .expect("read old notifier")
            .expect("old notifier");

        let historical_config = codex_home.path().join("profile-a.toml");
        set_notify_command(&historical_config, &old_notify).expect("write historical profile");
        let mut previous_installation = first.installation.clone();
        previous_installation
            .managed_config_paths
            .push(historical_config.to_string_lossy().into_owned());

        let upgrade = install_integration(
            &paths,
            Path::new("/tmp/codex-notify-new"),
            Some(&previous_installation),
        )
        .expect("install upgraded integration");
        assert_ne!(
            read_notify_command(&paths.codex_config()).expect("read upgraded notifier"),
            Some(old_notify.clone())
        );

        rollback_integration(&paths, &upgrade).expect("rollback upgrade");

        assert_eq!(
            read_notify_command(&paths.codex_config()).expect("read restored notifier"),
            Some(old_notify.clone())
        );
        assert_eq!(
            read_notify_command(&historical_config).expect("read historical notifier"),
            Some(old_notify)
        );
        let hooks = fs::read_to_string(paths.codex_hooks()).expect("read restored hooks");
        assert!(hooks.contains("/tmp/codex-notify-old"));
        assert!(!hooks.contains("/tmp/codex-notify-new"));
    }

    #[test]
    fn uninstall_removes_empty_codex_files_created_by_the_integration() {
        let app_home = tempdir().expect("temporary app home");
        let codex_home = tempdir().expect("temporary Codex home");
        let paths = paths(app_home.path(), codex_home.path());
        let setup =
            install_integration(&paths, Path::new("/tmp/codex-notify"), None).expect("install");
        assert!(setup.installation.created_codex_config);
        assert!(setup.installation.created_codex_hooks);

        restore_notify_command(&paths.codex_config(), &setup.installation).expect("restore notify");
        remove_prompt_hook(&paths.codex_hooks()).expect("remove prompt hook");
        remove_stop_hook(&paths.codex_hooks()).expect("remove stop hook");
        remove_empty_created_codex_files(
            &paths.codex_config(),
            &paths.codex_hooks(),
            &setup.installation,
        )
        .expect("remove empty created files");

        assert!(!paths.codex_config().exists());
        assert!(!paths.codex_hooks().exists());
    }

    #[test]
    fn uninstall_keeps_created_codex_files_after_the_user_adds_content() {
        let app_home = tempdir().expect("temporary app home");
        let codex_home = tempdir().expect("temporary Codex home");
        let paths = paths(app_home.path(), codex_home.path());
        let setup =
            install_integration(&paths, Path::new("/tmp/codex-notify"), None).expect("install");

        restore_notify_command(&paths.codex_config(), &setup.installation).expect("restore notify");
        fs::write(paths.codex_config(), "model = \"gpt-test\"\n").expect("add config");
        remove_prompt_hook(&paths.codex_hooks()).expect("remove prompt hook");
        remove_stop_hook(&paths.codex_hooks()).expect("remove stop hook");
        fs::write(
            paths.codex_hooks(),
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"type":"command","command":"keep"}]}]}}"#,
        )
        .expect("add hook");

        remove_empty_created_codex_files(
            &paths.codex_config(),
            &paths.codex_hooks(),
            &setup.installation,
        )
        .expect("preserve user content");

        assert!(paths.codex_config().exists());
        assert!(paths.codex_hooks().exists());
    }
}
