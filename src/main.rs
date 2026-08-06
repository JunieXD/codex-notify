use std::env;
use std::process::ExitCode;

const VERSION: &str = env!("CARGO_PKG_VERSION");

const HELP: &str = r#"codex-notify

Cross-platform local notifications for Codex. Feishu is the first channel.

Usage:
  codex-notify <command>

Commands:
  init       Configure Codex, Feishu, and the local error watcher.
  test       Send a test notification.
  status     Show the current installation status.
  doctor     Diagnose Codex, Feishu, and watcher configuration.
  uninstall  Remove only codex-notify-managed integration.
  watch      Run the local terminal-error watcher.

This initial release contains the project foundation. See docs/specification.md.
"#;

fn main() -> ExitCode {
    let command = env::args().nth(1);

    match command.as_deref() {
        None | Some("help") | Some("--help") | Some("-h") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some("--version") | Some("-V") | Some("version") => {
            println!("codex-notify {VERSION}");
            ExitCode::SUCCESS
        }
        Some(command)
            if matches!(
                command,
                "init" | "test" | "status" | "doctor" | "uninstall" | "watch"
            ) =>
        {
            eprintln!(
                "`{command}` is specified but not implemented yet. See docs/specification.md."
            );
            ExitCode::from(2)
        }
        Some(command) => {
            eprintln!("Unknown command: {command}\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}
