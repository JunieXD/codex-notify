use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Deserializer};
use serde_json::{Map, Value, json};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use toml_edit::{Array, DocumentMut, Item, Value as TomlValue};

use crate::model::Notification;
use crate::paths::AppPaths;
use crate::settings::{InstallationConfig, atomic_write};
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
    let title = find_thread_title(&paths.session_index(), &thread_id)
        .or_else(|| {
            state
                .as_ref()
                .and_then(|state| state.conversation_title_at_start.clone())
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
}

pub fn managed_notify_command(binary: &Path) -> Vec<String> {
    vec![binary.to_string_lossy().into_owned(), "notify".to_owned()]
}

pub fn read_notify_command(config_path: &Path) -> Result<Option<Vec<String>>> {
    let document = read_toml_document(config_path)?;
    let Some(item) = document.get("notify") else {
        return Ok(None);
    };
    let array = item
        .as_array()
        .context("Codex notify must be an array of command arguments")?;
    let mut command = Vec::with_capacity(array.len());
    for argument in array.iter() {
        let argument = argument
            .as_str()
            .context("Codex notify command arguments must be strings")?;
        command.push(argument.to_owned());
    }
    Ok((!command.is_empty()).then_some(command))
}

pub fn set_notify_command(config_path: &Path, command: &[String]) -> Result<()> {
    if command.is_empty() {
        bail!("cannot set an empty Codex notify command");
    }
    let mut document = read_toml_document(config_path)?;
    document["notify"] = command_item(command);
    atomic_write(config_path, document.to_string().as_bytes())
}

pub fn remove_notify_command(config_path: &Path) -> Result<()> {
    let mut document = read_toml_document(config_path)?;
    if document.remove("notify").is_some() {
        atomic_write(config_path, document.to_string().as_bytes())?;
    }
    Ok(())
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
        .context("Codex hooks.json must contain a JSON object")?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .context("Codex hooks.json hooks value must be a JSON object")?;
    let groups = hooks
        .entry(event_name)
        .or_insert_with(|| Value::Array(Vec::new()))
        .as_array_mut()
        .with_context(|| format!("Codex {event_name} hooks must be an array"))?;

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

fn remove_hook(hooks_path: &Path, event_name: &str, marker: &str) -> Result<bool> {
    if !hooks_path.exists() {
        return Ok(false);
    }
    let mut document = read_hooks_document(hooks_path)?;
    let mut changed = false;

    {
        let root = document
            .as_object_mut()
            .context("Codex hooks.json must contain a JSON object")?;
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
            "could not back up {} to {}",
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
    if active.as_deref() != Some(installation.managed_notify.as_slice()) {
        return Ok(RestoreNotifyResult::NotOwned);
    }

    match &installation.previous_notify {
        Some(command) => set_notify_command(config_path, command)?,
        None => remove_notify_command(config_path)?,
    }
    Ok(RestoreNotifyResult::Restored)
}

#[derive(Debug, Clone)]
pub struct IntegrationSetup {
    pub installation: InstallationConfig,
    pub config_backup: Option<PathBuf>,
    pub hooks_backup: Option<PathBuf>,
}

pub fn install_integration(
    paths: &AppPaths,
    binary: &Path,
    previous_installation: Option<&InstallationConfig>,
) -> Result<IntegrationSetup> {
    let config_path = paths.codex_config();
    let hooks_path = paths.codex_hooks();
    let active_notify = read_notify_command(&config_path)?;
    let managed_notify = managed_notify_command(binary);

    let previous_notify = match previous_installation {
        Some(installation)
            if active_notify.as_deref() == Some(installation.managed_notify.as_slice()) =>
        {
            installation.previous_notify.clone()
        }
        Some(_) => {
            bail!(
                "Codex notify changed since codex-notify was installed; refusing to overwrite it"
            );
        }
        None if active_notify.as_deref() == Some(managed_notify.as_slice()) => {
            bail!("Codex notify already points to codex-notify, but its prior command is unknown");
        }
        None => active_notify,
    };

    let original_config = read_optional_file(&config_path)?;
    let original_hooks = read_optional_file(&hooks_path)?;
    let config_backup = backup_file(paths, &config_path, "config")?;
    let hooks_backup = backup_file(paths, &hooks_path, "hooks")?;

    let result = (|| {
        set_notify_command(&config_path, &managed_notify)?;
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
        restore_original_file(&config_path, original_config.as_deref())?;
        restore_original_file(&hooks_path, original_hooks.as_deref())?;
        return Err(error);
    }

    Ok(IntegrationSetup {
        installation: InstallationConfig {
            previous_notify,
            managed_notify,
            codex_config_path: config_path.to_string_lossy().into_owned(),
            codex_hooks_path: hooks_path.to_string_lossy().into_owned(),
            prompt_hook_marker: PROMPT_HOOK_MARKER.to_owned(),
            stop_hook_marker: STOP_HOOK_MARKER.to_owned(),
        },
        config_backup,
        hooks_backup,
    })
}

pub fn rollback_integration(paths: &AppPaths, setup: &IntegrationSetup) -> Result<()> {
    restore_from_backup(&paths.codex_config(), setup.config_backup.as_deref())?;
    restore_from_backup(&paths.codex_hooks(), setup.hooks_backup.as_deref())
}

pub fn run_previous_notifier(command: &[String], event_json: &str) -> Result<()> {
    let (program, arguments) = command
        .split_first()
        .context("stored previous Codex notify command is empty")?;
    let status = Command::new(program)
        .args(arguments)
        .arg(event_json)
        .status()
        .with_context(|| format!("could not run previous Codex notify command '{program}'"))?;
    if !status.success() {
        return Err(anyhow!(
            "previous Codex notify command '{program}' exited with {status}"
        ));
    }
    Ok(())
}

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
            return Err(error).with_context(|| format!("could not read {}", path.display()));
        }
    };
    contents
        .parse::<DocumentMut>()
        .with_context(|| format!("could not parse {}", path.display()))
}

fn read_optional_file(path: &Path) -> Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("could not read {}", path.display())),
    }
}

fn restore_original_file(path: &Path, contents: Option<&[u8]>) -> Result<()> {
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

fn restore_from_backup(destination: &Path, backup: Option<&Path>) -> Result<()> {
    let contents =
        match backup {
            Some(backup) => Some(fs::read(backup).with_context(|| {
                format!("could not read integration backup {}", backup.display())
            })?),
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
            return Err(error).with_context(|| format!("could not read {}", path.display()));
        }
    };
    serde_json::from_slice(&contents).with_context(|| format!("could not parse {}", path.display()))
}

fn write_hooks_document(path: &Path, document: &Value) -> Result<()> {
    let contents =
        serde_json::to_vec_pretty(document).context("could not serialize Codex hooks")?;
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
        remove_prompt_hook, remove_stop_hook, rollback_integration, set_notify_command,
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
    fn known_ambient_turns_are_filtered() {
        assert!(is_internal_prompt(
            "Generate 0 to 3 hyperpersonalized suggestions for what this user can do with Codex"
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
            Some(vec!["/tmp/codex-notify".to_owned(), "notify".to_owned()])
        );
        assert!(has_prompt_hook(&paths.codex_hooks()).expect("has prompt hook"));
        assert!(has_stop_hook(&paths.codex_hooks()).expect("has Stop hook"));
        assert!(setup.config_backup.is_some());
        rollback_integration(&paths, &setup).expect("rollback");
        assert_eq!(
            fs::read_to_string(paths.codex_config()).expect("restored config"),
            "notify = [\"python3\", \"/tmp/previous-notify.py\"]\n"
        );
        assert!(!paths.codex_hooks().exists());
    }
}
