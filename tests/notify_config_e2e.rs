use codex_notify::codex::{
    PROMPT_HOOK_MARKER, STOP_HOOK_MARKER, managed_notify_command, read_notify_command,
    set_notify_command,
};
#[cfg(unix)]
use codex_notify::codex::{RestoreNotifyResult, restore_notify_command};
use codex_notify::paths::AppPaths;
use codex_notify::settings::{AppConfig, FeishuConfig, InstallationConfig, ReceiverIdType};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use tempfile::{TempDir, tempdir};

fn fixture() -> (TempDir, TempDir, AppPaths, PathBuf) {
    let app_home = tempdir().expect("temporary app home");
    let codex_home = tempdir().expect("temporary Codex home");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_codex-notify"));
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
            app_id: "cli_e2e_app".to_owned(),
            receiver_id_type: ReceiverIdType::Email,
            receiver_id: "test@example.com".to_owned(),
        },
        InstallationConfig {
            previous_notify: None,
            managed_notify: managed_notify_command(&binary),
            managed_binary_paths: vec![binary.to_string_lossy().into_owned()],
            managed_config_paths: Vec::new(),
            codex_config_path: paths.codex_config().to_string_lossy().into_owned(),
            codex_hooks_path: paths.codex_hooks().to_string_lossy().into_owned(),
            prompt_hook_marker: PROMPT_HOOK_MARKER.to_owned(),
            stop_hook_marker: STOP_HOOK_MARKER.to_owned(),
            created_codex_config: false,
            created_codex_hooks: false,
        },
    );
    config.save(&paths).expect("save app config");
    (app_home, codex_home, paths, binary)
}

fn run(binary: &Path, paths: &AppPaths, arguments: &[&str]) -> Output {
    Command::new(binary)
        .args(arguments)
        .env("CODEX_NOTIFY_HOME", &paths.root)
        .env("CODEX_NOTIFY_CODEX_HOME", &paths.codex_home)
        .output()
        .expect("run codex-notify")
}

fn assert_success(output: &Output) {
    assert!(
        output.status.success(),
        "command failed\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn json_command_argument(command: &[String], flag: &str) -> Option<Vec<String>> {
    let index = command.iter().position(|argument| argument == flag)?;
    serde_json::from_str(command.get(index + 1)?).ok()
}

fn mac_computer_use(previous: Option<Vec<String>>) -> Vec<String> {
    let mut command = vec![
        "/tmp/Codex Computer Use.app/SkyComputerUseClient".to_owned(),
        "turn-ended".to_owned(),
    ];
    if let Some(previous) = previous {
        command.push("--previous-notify".to_owned());
        command.push(serde_json::to_string(&previous).expect("serialize previous"));
    }
    command
}

#[test]
fn sync_and_watcher_reconcile_two_different_active_configs() {
    let (_app_home, _codex_home, paths, binary) = fixture();
    set_notify_command(&paths.codex_config(), &mac_computer_use(None))
        .expect("write Computer Use config");

    let first = run(&binary, &paths, &["sync"]);
    assert_success(&first);
    let profile_a = fs::read_to_string(paths.codex_config()).expect("read profile A");
    let active_a = read_notify_command(&paths.codex_config())
        .expect("read notify")
        .expect("active notify");
    let managed_a = json_command_argument(&active_a, "--previous-notify")
        .expect("Computer Use forwards to codex-notify");
    assert_eq!(managed_a, managed_notify_command(&binary));

    set_notify_command(
        &paths.codex_config(),
        &["profile-b-notifier".to_owned(), "--quiet".to_owned()],
    )
    .expect("switch to profile B");
    let second = run(&binary, &paths, &["watch", "--once"]);
    assert_success(&second);
    let active_b = read_notify_command(&paths.codex_config())
        .expect("read notify")
        .expect("active notify");
    assert_eq!(&active_b[..3], managed_notify_command(&binary).as_slice());
    assert_eq!(
        json_command_argument(&active_b, "--forward-notify"),
        Some(vec!["profile-b-notifier".to_owned(), "--quiet".to_owned()])
    );

    fs::write(paths.codex_config(), &profile_a).expect("switch back to profile A");
    let third = run(&binary, &paths, &["watch", "--once"]);
    assert_success(&third);
    assert_eq!(
        fs::read_to_string(paths.codex_config()).expect("read restored profile A"),
        profile_a
    );
}

#[cfg(unix)]
#[test]
fn watcher_follows_profile_symlinks_and_registers_each_target() {
    use std::os::unix::fs::symlink;

    let (_app_home, _codex_home, paths, binary) = fixture();
    let profile_a = paths.codex_home.join("profile-a.toml");
    let profile_b = paths.codex_home.join("profile-b.toml");
    set_notify_command(&profile_a, &mac_computer_use(None)).expect("write profile A");
    set_notify_command(&profile_b, &["profile-b-notifier".to_owned()]).expect("write profile B");
    symlink(&profile_a, paths.codex_config()).expect("activate profile A");

    assert_success(&run(&binary, &paths, &["sync"]));
    assert!(
        fs::symlink_metadata(paths.codex_config())
            .expect("profile symlink")
            .file_type()
            .is_symlink()
    );
    assert!(
        read_notify_command(&profile_a)
            .expect("read profile A")
            .is_some_and(|command| command.iter().any(|item| item == "--previous-notify"))
    );

    fs::remove_file(paths.codex_config()).expect("deactivate profile A");
    symlink(&profile_b, paths.codex_config()).expect("activate profile B");
    assert_success(&run(&binary, &paths, &["watch", "--once"]));
    assert!(
        fs::symlink_metadata(paths.codex_config())
            .expect("profile symlink")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        json_command_argument(
            &read_notify_command(&profile_b)
                .expect("read profile B")
                .expect("profile B notify"),
            "--forward-notify"
        ),
        Some(vec!["profile-b-notifier".to_owned()])
    );

    let config = AppConfig::load(&paths)
        .expect("load app config")
        .expect("configured");
    let profile_a = fs::canonicalize(profile_a)
        .expect("canonical profile A")
        .to_string_lossy()
        .into_owned();
    let profile_b = fs::canonicalize(profile_b)
        .expect("canonical profile B")
        .to_string_lossy()
        .into_owned();
    assert!(
        config
            .installation
            .managed_config_paths
            .contains(&profile_a)
    );
    assert!(
        config
            .installation
            .managed_config_paths
            .contains(&profile_b)
    );

    assert_eq!(
        restore_notify_command(Path::new(&profile_a), &config.installation)
            .expect("restore profile A"),
        RestoreNotifyResult::Restored
    );
    assert_eq!(
        restore_notify_command(Path::new(&profile_b), &config.installation)
            .expect("restore profile B"),
        RestoreNotifyResult::Restored
    );
    assert_eq!(
        read_notify_command(Path::new(&profile_a)).expect("read restored profile A"),
        Some(mac_computer_use(None))
    );
    assert_eq!(
        read_notify_command(Path::new(&profile_b)).expect("read restored profile B"),
        Some(vec!["profile-b-notifier".to_owned()])
    );
}

#[cfg(unix)]
#[test]
fn managed_notify_forwards_the_event_even_without_app_configuration() {
    use std::os::unix::fs::PermissionsExt;

    let app_home = tempdir().expect("temporary app home");
    let codex_home = tempdir().expect("temporary Codex home");
    let binary = PathBuf::from(env!("CARGO_BIN_EXE_codex-notify"));
    let capture = app_home.path().join("captured-event.json");
    let notifier = app_home.path().join("capture-notifier.sh");
    fs::write(
        &notifier,
        "#!/bin/sh\nprintf '%s' \"$1\" > \"$CAPTURE_PATH\"\n",
    )
    .expect("write notifier");
    let mut permissions = fs::metadata(&notifier)
        .expect("notifier metadata")
        .permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(&notifier, permissions).expect("make notifier executable");
    let forward = serde_json::to_string(&vec![notifier.to_string_lossy().into_owned()])
        .expect("serialize notifier");
    let event = r#"{"type":"agent-turn-complete","turn-id":"turn-e2e"}"#;

    let output = Command::new(&binary)
        .args(["notify", "--managed", "--forward-notify", &forward, event])
        .env("CODEX_NOTIFY_HOME", app_home.path())
        .env("CODEX_NOTIFY_CODEX_HOME", codex_home.path())
        .env("CAPTURE_PATH", &capture)
        .output()
        .expect("run managed notify");
    assert_success(&output);
    assert_eq!(
        fs::read_to_string(capture).expect("read captured event"),
        event
    );
}
